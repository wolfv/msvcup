use crate::packages::{LockFileUrlKind, MsvcupPackage, MsvcupPackageKind, get_lock_file_url_kind};
use crate::sha::Sha256;
use anyhow::{Result, bail};
use std::cmp::Ordering;

#[derive(Debug)]
pub struct LockFilePayload {
    pub url_decoded: String,
    pub sha256: Sha256,
    pub url_kind: LockFilePayloadKind,
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum LockFilePayloadKind {
    TopLevel(MsvcupPackage),
    Cab(String),
}

impl LockFilePayload {
    pub fn strip_root_dir(&self) -> bool {
        match &self.url_kind {
            LockFilePayloadKind::TopLevel(pkg) => matches!(pkg.kind, MsvcupPackageKind::Cmake),
            LockFilePayloadKind::Cab(_) => false,
        }
    }

    pub fn host_arch_limit(&self) -> Option<crate::arch::Arch> {
        match &self.url_kind {
            LockFilePayloadKind::TopLevel(pkg) => match pkg.kind {
                MsvcupPackageKind::Msvc
                | MsvcupPackageKind::Sdk
                | MsvcupPackageKind::Msbuild
                | MsvcupPackageKind::Diasdk => None,
                MsvcupPackageKind::Ninja | MsvcupPackageKind::Cmake => {
                    match crate::extra::parse_url(&self.url_decoded) {
                        crate::extra::ParseUrlResult::Ok { arch } => Some(arch),
                        crate::extra::ParseUrlResult::Unexpected { .. } => None,
                    }
                }
            },
            LockFilePayloadKind::Cab(_) => None,
        }
    }
}

pub fn parse_lock_file_payload(
    lock_file_path: &str,
    lineno: u32,
    line: &str,
) -> Result<LockFilePayload> {
    let msvcup_pkg_end = line.find('|').ok_or_else(|| {
        anyhow::anyhow!("{}:{}: line has no '|' separator", lock_file_path, lineno)
    })?;
    let msvcup_pkg_str = &line[..msvcup_pkg_end];
    let maybe_msvcup_pkg = if msvcup_pkg_str.is_empty() {
        None
    } else {
        Some(MsvcupPackage::from_string(msvcup_pkg_str).map_err(|e| {
            anyhow::anyhow!(
                "{}:{}: invalid msvcup pkg '{}': {}",
                lock_file_path,
                lineno,
                msvcup_pkg_str,
                e
            )
        })?)
    };

    let url_start = msvcup_pkg_end + 1;
    let url_end = line[url_start..]
        .find('|')
        .map(|i| url_start + i)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{}:{}: line has no second '|' separator",
                lock_file_path,
                lineno
            )
        })?;
    let url_decoded = &line[url_start..url_end];

    let url_kind = get_lock_file_url_kind(url_decoded).ok_or_else(|| {
        anyhow::anyhow!(
            "{}:{}: unable to determine payload kind from url '{}'",
            lock_file_path,
            lineno,
            url_decoded
        )
    })?;

    // Parse hash spec (either 64-char hex or an index into the URL)
    let hash_start = url_end + 1;
    let hash_end = line[hash_start..]
        .find(' ')
        .map(|i| hash_start + i)
        .unwrap_or(line.len());
    let hash_spec = &line[hash_start..hash_end];

    let sha256_hex = if hash_spec.len() == 64 {
        hash_spec.to_ascii_lowercase()
    } else {
        let hash_index: usize = hash_spec.parse().map_err(|_| {
            anyhow::anyhow!(
                "{}:{}: expected sha256 hash or unsigned integer, got '{}'",
                lock_file_path,
                lineno,
                hash_spec
            )
        })?;
        if hash_index + 64 > url_decoded.len() {
            bail!(
                "{}:{}: hash index {} out of bounds (url is {} chars)",
                lock_file_path,
                lineno,
                hash_index,
                url_decoded.len()
            );
        }
        url_decoded[hash_index..hash_index + 64].to_ascii_lowercase()
    };

    let sha256 = Sha256::parse_hex(&sha256_hex).ok_or_else(|| {
        anyhow::anyhow!(
            "{}:{}: invalid sha256 hash '{}'",
            lock_file_path,
            lineno,
            sha256_hex
        )
    })?;

    match url_kind {
        LockFileUrlKind::Vsix | LockFileUrlKind::Msi | LockFileUrlKind::Zip => {
            let msvcup_pkg = maybe_msvcup_pkg.ok_or_else(|| {
                anyhow::anyhow!("{}:{}: missing msvcup package", lock_file_path, lineno)
            })?;
            Ok(LockFilePayload {
                url_decoded: url_decoded.to_string(),
                sha256,
                url_kind: LockFilePayloadKind::TopLevel(msvcup_pkg),
            })
        }
        LockFileUrlKind::Cab => {
            if maybe_msvcup_pkg.is_some() {
                bail!(
                    "{}:{}: cab payloads should not have an associated msvcup package",
                    lock_file_path,
                    lineno
                );
            }
            let cab_path = if hash_end < line.len() {
                line[hash_end..].to_string()
            } else {
                bail!(
                    "{}:{}: missing ' PATH' after hash (required for .cab payloads)",
                    lock_file_path,
                    lineno
                );
            };
            Ok(LockFilePayload {
                url_decoded: url_decoded.to_string(),
                sha256,
                url_kind: LockFilePayloadKind::Cab(cab_path),
            })
        }
    }
}

/// Check if the lock file's packages match what we want to install
pub fn check_lock_file_pkgs(
    lock_file_path: &str,
    lock_file_content: &str,
    msvcup_pkgs: &[MsvcupPackage],
) -> Option<String> {
    assert!(!msvcup_pkgs.is_empty());
    let mut msvcup_pkg_index = 0;
    let mut msvcup_pkg_match_count = 0;

    for line in lock_file_content.lines() {
        if line.is_empty() {
            continue;
        }
        let parsed = match parse_lock_file_payload(lock_file_path, 0, line) {
            Ok(p) => p,
            Err(e) => return Some(format!("parse error: {}", e)),
        };
        while let LockFilePayloadKind::TopLevel(payload_pkg) = &parsed.url_kind {
            match MsvcupPackage::order(&msvcup_pkgs[msvcup_pkg_index], payload_pkg) {
                Ordering::Equal => {
                    msvcup_pkg_match_count += 1;
                    break;
                }
                Ordering::Less => {
                    if msvcup_pkg_match_count == 0 {
                        return Some(format!(
                            "lock file is missing package '{}'",
                            msvcup_pkgs[msvcup_pkg_index]
                        ));
                    }
                    if msvcup_pkg_index + 1 == msvcup_pkgs.len() {
                        return Some(format!("lock file has extra package '{}'", payload_pkg));
                    }
                    msvcup_pkg_index += 1;
                    msvcup_pkg_match_count = 0;
                    continue;
                }
                Ordering::Greater => {
                    return Some(format!("lock file has extra package '{}'", payload_pkg));
                }
            }
        }
    }

    if msvcup_pkg_index + 1 < msvcup_pkgs.len() || msvcup_pkg_match_count == 0 {
        return Some(format!(
            "lock file is missing package '{}'",
            msvcup_pkgs[msvcup_pkg_index]
        ));
    }
    None
}

/// Write a payload line to the lock file
pub fn write_payload(
    writer: &mut impl std::io::Write,
    maybe_target: Option<&MsvcupPackage>,
    url_kind: LockFileUrlKind,
    url: &str,
    sha256: &Sha256,
    file_name: &str,
) -> std::io::Result<()> {
    let target_str = match maybe_target {
        Some(t) => t.pool_string(),
        None => String::new(),
    };
    let sha_hex = sha256.to_hex();

    let (space, out_file_name) = match url_kind {
        LockFileUrlKind::Vsix | LockFileUrlKind::Msi | LockFileUrlKind::Zip => ("", ""),
        LockFileUrlKind::Cab => (" ", file_name),
    };

    // Check if the sha hex appears in the URL (for compact representation)
    if let Some(hash_index) = url.to_ascii_lowercase().find(&sha_hex) {
        writeln!(
            writer,
            "{}|{}|{}{}{}",
            target_str, url, hash_index, space, out_file_name
        )
    } else {
        let display_name = match url_kind {
            LockFileUrlKind::Cab => {
                // Use basename for cab files
                file_name
                    .rfind('\\')
                    .map(|i| &file_name[i + 1..])
                    .unwrap_or(file_name)
            }
            _ => "",
        };
        writeln!(
            writer,
            "{}|{}|{}{}{}",
            target_str, url, sha_hex, space, display_name
        )
    }
}
