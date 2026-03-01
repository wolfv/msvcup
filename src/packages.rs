//! Package identification and manifest parsing.
//!
//! This module parses the Visual Studio channel manifest JSON into structured
//! package and payload data. It identifies which manifest entries correspond to
//! msvcup package types (MSVC, SDK, MSBuild, etc.) and provides the data needed
//! to download and install them.

use crate::arch::Arch;
use crate::sha::Sha256;
use crate::util::{
    alloc_url_percent_decoded, basename_from_url, order_dotted_numeric, scan_id_part,
    scan_id_version,
};
use anyhow::{Context, Result};
use std::cmp::Ordering;
use std::fmt;

/// The type of an msvcup package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MsvcupPackageKind {
    Msvc,
    Sdk,
    Msbuild,
    Diasdk,
    Ninja,
    Cmake,
}

impl MsvcupPackageKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Msvc => "msvc",
            Self::Sdk => "sdk",
            Self::Msbuild => "msbuild",
            Self::Diasdk => "diasdk",
            Self::Ninja => "ninja",
            Self::Cmake => "cmake",
        }
    }

    pub fn from_prefix(s: &str) -> Option<(MsvcupPackageKind, &str)> {
        if let Some(v) = s.strip_prefix("msvc-") {
            return Some((Self::Msvc, v));
        }
        if let Some(v) = s.strip_prefix("sdk-") {
            return Some((Self::Sdk, v));
        }
        if let Some(v) = s.strip_prefix("msbuild-") {
            return Some((Self::Msbuild, v));
        }
        if let Some(v) = s.strip_prefix("diasdk-") {
            return Some((Self::Diasdk, v));
        }
        if let Some(v) = s.strip_prefix("ninja-") {
            return Some((Self::Ninja, v));
        }
        if let Some(v) = s.strip_prefix("cmake-") {
            return Some((Self::Cmake, v));
        }
        None
    }
}

impl fmt::Display for MsvcupPackageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A user-facing package identifier like `msvc-14.44.17.14` or `sdk-10.0.22621.7`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MsvcupPackage {
    pub kind: MsvcupPackageKind,
    pub version: String,
}

impl MsvcupPackage {
    pub fn new(kind: MsvcupPackageKind, version: impl Into<String>) -> Self {
        Self {
            kind,
            version: version.into(),
        }
    }

    pub fn from_string(s: &str) -> Result<Self, MsvcupPackageParseError> {
        let (kind, version) =
            MsvcupPackageKind::from_prefix(s).ok_or(MsvcupPackageParseError::UnknownName)?;
        if !crate::util::is_valid_version(version) {
            return Err(MsvcupPackageParseError::InvalidVersion(version.to_string()));
        }
        Ok(Self {
            kind,
            version: version.to_string(),
        })
    }

    pub fn pool_string(&self) -> String {
        format!("{}", self)
    }

    pub fn order(lhs: &MsvcupPackage, rhs: &MsvcupPackage) -> Ordering {
        match lhs.kind.cmp(&rhs.kind) {
            Ordering::Equal => order_dotted_numeric(&lhs.version, &rhs.version),
            other => other,
        }
    }
}

impl fmt::Display for MsvcupPackage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.kind, self.version)
    }
}

#[derive(Debug)]
pub enum MsvcupPackageParseError {
    UnknownName,
    InvalidVersion(String),
}

impl fmt::Display for MsvcupPackageParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownName => write!(f, "unknown package name"),
            Self::InvalidVersion(v) => write!(f, "invalid version '{}'", v),
        }
    }
}

// --- Package identification (from VS manifest) ---

#[derive(Debug)]
#[allow(dead_code)]
pub enum PackageId<'a> {
    Unknown,
    Unexpected {
        offset: usize,
        expected: &'static str,
    },
    MsvcVersionSomething {
        build_version: &'a str,
        something: &'a str,
    },
    MsvcVersionToolsSomething {
        build_version: &'a str,
        something: &'a str,
    },
    MsvcVersionHostTarget {
        build_version: &'a str,
        host_arch: Arch,
        target_arch: Arch,
        name: &'a str,
    },
    Msbuild(&'a str),
    Diasdk,
    Ninja(&'a str),
    Cmake(&'a str),
}

pub fn identify_package(id: &str) -> PackageId<'_> {
    // MSBuild packages
    if id == "Microsoft.Build" || id == "Microsoft.Build.Arm64" {
        return PackageId::Msbuild("170");
    }
    let msbuild_prefix = "Microsoft.VisualStudio.VC.MSBuild.";
    if let Some(rest) = id.strip_prefix(msbuild_prefix) {
        let (version, _end) = scan_id_part(rest, 0);
        if version == "v170" {
            return PackageId::Msbuild("170");
        }
    }

    // DIA SDK
    if id == "Microsoft.VisualCpp.DIA.SDK" {
        return PackageId::Diasdk;
    }

    // MSVC packages
    let msvc_prefix = "Microsoft.VC.";
    if let Some(rest) = id.strip_prefix(msvc_prefix) {
        let (version, version_end) = scan_id_version(rest, 0);
        if version.is_empty() {
            return PackageId::Unexpected {
                offset: msvc_prefix.len(),
                expected: "version",
            };
        }
        let rest2 = &rest[version_end..];
        if rest2.is_empty() || !rest2.starts_with('.') {
            return PackageId::Unexpected {
                offset: msvc_prefix.len() + version_end,
                expected: "anything",
            };
        }
        let rest2 = &rest2[1..]; // skip '.'
        let (tools_part, tools_end) = scan_id_part(rest2, 0);
        if tools_part != "Tools" {
            return PackageId::MsvcVersionSomething {
                build_version: version,
                something: &rest[version_end..],
            };
        }
        let rest3 = &rest2[tools_end..];
        let (host_part, host_end) = scan_id_part(rest3, 0);
        if host_part.is_empty() {
            return PackageId::Unexpected {
                offset: msvc_prefix.len() + version_end + 1 + tools_end,
                expected: "anything",
            };
        }
        if !host_part.starts_with("Host") {
            return PackageId::MsvcVersionToolsSomething {
                build_version: version,
                something: &rest[version_end..],
            };
        }
        let host_arch_str = &host_part[4..];
        let host_arch = match Arch::from_str_ignore_case(host_arch_str) {
            Some(a) => a,
            None => {
                return PackageId::Unexpected {
                    offset: msvc_prefix.len() + version_end + 1 + tools_end + 4,
                    expected: "arch",
                };
            }
        };
        let rest4 = &rest3[host_end..];
        let (target_part, target_end) = scan_id_part(rest4, 0);
        if !target_part.starts_with("Target") {
            return PackageId::Unexpected {
                offset: msvc_prefix.len() + version_end + 1 + tools_end + host_end,
                expected: "target_arch",
            };
        }
        let target_arch_str = &target_part[6..];
        let target_arch = match Arch::from_str_ignore_case(target_arch_str) {
            Some(a) => a,
            None => {
                return PackageId::Unexpected {
                    offset: msvc_prefix.len() + version_end + 1 + tools_end + host_end + 6,
                    expected: "arch",
                };
            }
        };
        return PackageId::MsvcVersionHostTarget {
            build_version: version,
            host_arch,
            target_arch,
            name: &rest4[target_end..],
        };
    }

    // Ninja
    if let Some(rest) = id.strip_prefix("ninja-") {
        let (version, version_end) = scan_id_version(rest, 0);
        if version.is_empty() {
            return PackageId::Unexpected {
                offset: 6,
                expected: "version",
            };
        }
        if version_end != rest.len() {
            return PackageId::Unexpected {
                offset: 6 + version_end,
                expected: "end",
            };
        }
        return PackageId::Ninja(version);
    }

    // CMake
    if let Some(rest) = id.strip_prefix("cmake-") {
        let (version, version_end) = scan_id_version(rest, 0);
        if version.is_empty() {
            return PackageId::Unexpected {
                offset: 6,
                expected: "version",
            };
        }
        if version_end != rest.len() {
            return PackageId::Unexpected {
                offset: 6 + version_end,
                expected: "end",
            };
        }
        return PackageId::Cmake(version);
    }

    PackageId::Unknown
}

// --- Payload identification ---

#[derive(Debug, PartialEq, Eq)]
pub enum PayloadId {
    Unknown,
    Sdk,
}

pub fn identify_payload(payload_filename: &str, target_arch: Arch) -> PayloadId {
    if payload_filename.starts_with("Installers\\Universal CRT Headers Libraries and Sources-") {
        return PayloadId::Sdk;
    }
    // Arch-specific SDK payloads: "Windows SDK Desktop Headers <arch>-" / "... Libs <arch>-"
    if let Some(rest) = payload_filename.strip_prefix("Installers\\Windows SDK Desktop Headers ") {
        return if sdk_payload_arch_matches(rest, target_arch) {
            PayloadId::Sdk
        } else {
            PayloadId::Unknown
        };
    }
    if let Some(rest) = payload_filename.strip_prefix("Installers\\Windows SDK Desktop Libs ") {
        return if sdk_payload_arch_matches(rest, target_arch) {
            PayloadId::Sdk
        } else {
            PayloadId::Unknown
        };
    }
    if payload_filename.starts_with("Installers\\Windows SDK Signing Tools-") {
        return PayloadId::Sdk;
    }
    if payload_filename.starts_with("Installers\\Windows SDK for Windows Store Apps Headers-") {
        return PayloadId::Sdk;
    }
    if payload_filename.starts_with("Installers\\Windows SDK for Windows Store Apps Libs-") {
        return PayloadId::Sdk;
    }
    if payload_filename.starts_with("Installers\\Windows SDK for Windows Store Apps Tools-") {
        return PayloadId::Sdk;
    }
    PayloadId::Unknown
}

/// Check if an SDK payload's arch (parsed from the filename after the prefix) matches target_arch.
/// Filenames look like "arm64-x86_en-us.msi" or "x64-x86_en-us.msi".
fn sdk_payload_arch_matches(rest: &str, target_arch: Arch) -> bool {
    if let Some(dash_pos) = rest.find('-') {
        let arch_str = &rest[..dash_pos];
        match Arch::from_str_ignore_case(arch_str) {
            Some(arch) => arch == target_arch,
            None => true, // unknown arch pattern, include to be safe
        }
    } else {
        true // no dash found, include to be safe
    }
}

// --- Lock file URL kind ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockFileUrlKind {
    Vsix,
    Msi,
    Cab,
    Zip,
}

pub fn get_lock_file_url_kind(url: &str) -> Option<LockFileUrlKind> {
    if url.ends_with(".vsix") {
        Some(LockFileUrlKind::Vsix)
    } else if url.ends_with(".msi") {
        Some(LockFileUrlKind::Msi)
    } else if url.ends_with(".cab") {
        Some(LockFileUrlKind::Cab)
    } else if url.ends_with(".zip") {
        Some(LockFileUrlKind::Zip)
    } else {
        None
    }
}

// --- Language ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Neutral,
    EnUs,
    Other,
}

const OTHER_LANGUAGES: &[&str] = &[
    "cs-CZ", "de-DE", "es-ES", "fr-FR", "it-IT", "ja-JP", "ko-KR", "pl-PL", "pt-BR", "ru-RU",
    "tr-TR", "zh-CN", "zh-TW",
];

impl Language {
    pub fn from_str(s: &str) -> Language {
        if s == "neutral" {
            Language::Neutral
        } else if s.eq_ignore_ascii_case("en-US") {
            Language::EnUs
        } else if OTHER_LANGUAGES.contains(&s) {
            Language::Other
        } else {
            log::warn!("unknown language '{}'", s);
            Language::Other
        }
    }
}

// --- Package and Payload structs for parsed VS manifest ---

#[derive(Debug, Clone)]
pub struct Package {
    pub id: String,
    pub version: String,
    pub payloads_offset: usize,
    pub language: Language,
}

#[derive(Debug, Clone)]
pub struct Payload {
    pub url_decoded: String,
    pub sha256: Sha256,
    pub file_name: String,
}

impl Payload {
    pub fn name_decoded(&self) -> &str {
        basename_from_url(&self.url_decoded)
    }
}

#[derive(Debug)]
pub struct Packages {
    pub packages: Vec<Package>,
    pub payloads: Vec<Payload>,
}

impl Packages {
    pub fn payload_range_from_pkg_index(&self, pkg_index: usize) -> std::ops::Range<usize> {
        let start = self.packages[pkg_index].payloads_offset;
        let limit = if pkg_index == self.packages.len() - 1 {
            self.payloads.len()
        } else {
            self.packages[pkg_index + 1].payloads_offset
        };
        start..limit
    }

    pub fn payloads_from_pkg_index(&self, pkg_index: usize) -> &[Payload] {
        let range = self.payload_range_from_pkg_index(pkg_index);
        &self.payloads[range]
    }

    pub fn pkg_index_from_payload_index(&self, payload_index: usize) -> usize {
        assert!(!self.packages.is_empty());
        let mut min = 0;
        let mut max = self.packages.len() - 1;
        loop {
            if min == max {
                return min;
            }
            assert!(min < max);
            let remaining_pkg_count = max - min + 1;
            let min_range = self.payload_range_from_pkg_index(min);
            let max_range = self.payload_range_from_pkg_index(max);
            let remaining_payload_count = max_range.end - min_range.start;
            assert!(remaining_payload_count >= 1);
            let ratio = (payload_index - min_range.start) as f32 / remaining_payload_count as f32;
            let guess =
                ((ratio * remaining_pkg_count as f32) as usize).min(remaining_pkg_count - 1);
            let pkg_index = min + guess;
            let range = self.payload_range_from_pkg_index(pkg_index);
            if payload_index < range.start {
                max = pkg_index - 1;
            } else if payload_index < range.end {
                return pkg_index;
            } else {
                min = pkg_index + 1;
            }
        }
    }
}

/// Parse the VS manifest JSON into Packages
pub fn get_packages(vsman_path: &str, vsman_content: &str) -> Result<Packages> {
    let parsed: serde_json::Value =
        serde_json::from_str(vsman_content).with_context(|| format!("parsing '{}'", vsman_path))?;

    let packages_arr = parsed
        .get("packages")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("{}: missing 'packages' array", vsman_path))?;

    let mut out_packages = Vec::with_capacity(packages_arr.len());
    let mut out_payloads = Vec::new();

    for pkg_val in packages_arr {
        let pkg_obj = pkg_val
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("{}: package is not an object", vsman_path))?;

        let id = pkg_obj
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("{}: package missing 'id'", vsman_path))?;
        let version = pkg_obj
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("{}: package missing 'version'", vsman_path))?;

        let language = match pkg_obj.get("language").and_then(|v| v.as_str()) {
            Some(lang) => Language::from_str(lang),
            None => Language::Neutral,
        };

        let payloads_offset = out_payloads.len();

        if let Some(payloads_val) = pkg_obj.get("payloads")
            && let Some(payloads_arr) = payloads_val.as_array()
        {
            for payload_val in payloads_arr {
                let payload_obj = payload_val
                    .as_object()
                    .ok_or_else(|| anyhow::anyhow!("{}: payload is not an object", vsman_path))?;

                let file_name = payload_obj
                    .get("fileName")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("{}: payload missing 'fileName'", vsman_path))?;
                let sha256_str = payload_obj
                    .get("sha256")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("{}: payload missing 'sha256'", vsman_path))?;
                let sha256_hex = sha256_str.to_ascii_lowercase();
                let sha256 = Sha256::parse_hex(&sha256_hex).ok_or_else(|| {
                    anyhow::anyhow!("{}: invalid sha256 '{}'", vsman_path, sha256_str)
                })?;
                let url = payload_obj
                    .get("url")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("{}: payload missing 'url'", vsman_path))?;

                out_payloads.push(Payload {
                    url_decoded: alloc_url_percent_decoded(url),
                    sha256,
                    file_name: file_name.to_string(),
                });
            }
        }

        out_packages.push(Package {
            id: id.to_string(),
            version: version.to_string(),
            payloads_offset,
            language,
        });
    }

    Ok(Packages {
        packages: out_packages,
        payloads: out_payloads,
    })
}

/// Identify which packages should be installed based on the install request.
/// Filters MSVC packages by host and target architecture.
pub fn get_install_pkg(id: &str, host_arch: Arch, target_arch: Arch) -> Option<InstallPkgKind> {
    match identify_package(id) {
        PackageId::Unknown => None,
        PackageId::Unexpected { .. } => None,
        PackageId::MsvcVersionSomething {
            build_version,
            something,
        } => {
            let (crt, crt_end) = scan_id_part(something, 1); // skip leading '.'
            if crt != "CRT" {
                return None;
            }
            let rest = &something[crt_end + 1..]; // +1 to account for the '.' we skipped

            // Check for CRT.Headers.base
            if rest.starts_with("Headers.base") {
                // Actually, let's compute properly
            }
            // Simplified: parse more carefully
            let after_crt = &something[1 + crt.len()..]; // skip ".CRT"
            if let Some(after_dot) = after_crt.strip_prefix(".") {
                if after_dot == "Headers.base" {
                    // Arch-neutral, always include
                    return Some(InstallPkgKind::Msvc(build_version.to_string()));
                }
                // Check for Redist patterns: CRT.Redist.<arch>.base
                let (next_part, next_end) = scan_id_part(after_dot, 0);
                if next_part == "Redist" {
                    let rest2 = &after_dot[next_end..];
                    let (arch_part, arch_end) = scan_id_part(rest2, 0);
                    if let Some(arch) = Arch::from_str_ignore_case(arch_part) {
                        if arch != target_arch {
                            return None;
                        }
                        let final_rest = &rest2[arch_end..];
                        if final_rest == "base" {
                            return Some(InstallPkgKind::Msvc(build_version.to_string()));
                        }
                    }
                } else if let Some(arch) = Arch::from_str_ignore_case(next_part) {
                    // CRT.<arch>.Desktop.base, CRT.<arch>.Store.base, etc.
                    if arch != target_arch {
                        return None;
                    }
                    let final_rest = &after_dot[next_end..];
                    if final_rest == "Desktop.base"
                        || final_rest == "Desktop.debug.base"
                        || final_rest == "Store.base"
                    {
                        return Some(InstallPkgKind::Msvc(build_version.to_string()));
                    }
                }
            }
            None
        }
        PackageId::MsvcVersionToolsSomething { .. } => None,
        PackageId::MsvcVersionHostTarget {
            build_version,
            host_arch: pkg_host,
            target_arch: pkg_target,
            name,
        } => {
            if pkg_host != host_arch || pkg_target != target_arch {
                return None;
            }
            if name == "base" || name == "Res.base" {
                Some(InstallPkgKind::Msvc(build_version.to_string()))
            } else {
                None
            }
        }
        PackageId::Msbuild(version) => Some(InstallPkgKind::Msbuild(version.to_string())),
        PackageId::Diasdk => Some(InstallPkgKind::Diasdk),
        PackageId::Ninja(version) => Some(InstallPkgKind::Ninja(version.to_string())),
        PackageId::Cmake(version) => Some(InstallPkgKind::Cmake(version.to_string())),
    }
}

#[derive(Debug)]
pub enum InstallPkgKind {
    Msvc(String),
    Msbuild(String),
    Diasdk,
    Ninja(String),
    Cmake(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestUpdate {
    Off,
    Daily,
    Always,
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- MsvcupPackageKind tests ---

    #[test]
    fn package_kind_as_str() {
        assert_eq!(MsvcupPackageKind::Msvc.as_str(), "msvc");
        assert_eq!(MsvcupPackageKind::Sdk.as_str(), "sdk");
        assert_eq!(MsvcupPackageKind::Msbuild.as_str(), "msbuild");
        assert_eq!(MsvcupPackageKind::Diasdk.as_str(), "diasdk");
        assert_eq!(MsvcupPackageKind::Ninja.as_str(), "ninja");
        assert_eq!(MsvcupPackageKind::Cmake.as_str(), "cmake");
    }

    #[test]
    fn package_kind_from_prefix() {
        let (kind, version) = MsvcupPackageKind::from_prefix("msvc-14.30.17.6").unwrap();
        assert_eq!(kind, MsvcupPackageKind::Msvc);
        assert_eq!(version, "14.30.17.6");

        let (kind, version) = MsvcupPackageKind::from_prefix("sdk-10.0.22621.7").unwrap();
        assert_eq!(kind, MsvcupPackageKind::Sdk);
        assert_eq!(version, "10.0.22621.7");

        let (kind, _) = MsvcupPackageKind::from_prefix("ninja-1.12.1").unwrap();
        assert_eq!(kind, MsvcupPackageKind::Ninja);

        let (kind, _) = MsvcupPackageKind::from_prefix("cmake-3.31.4").unwrap();
        assert_eq!(kind, MsvcupPackageKind::Cmake);

        assert!(MsvcupPackageKind::from_prefix("unknown-1.0").is_none());
        assert!(MsvcupPackageKind::from_prefix("").is_none());
    }

    #[test]
    fn package_kind_display() {
        assert_eq!(format!("{}", MsvcupPackageKind::Msvc), "msvc");
        assert_eq!(format!("{}", MsvcupPackageKind::Sdk), "sdk");
    }

    // --- MsvcupPackage tests ---

    #[test]
    fn msvcup_package_from_string_valid() {
        let pkg = MsvcupPackage::from_string("msvc-14.30.17.6").unwrap();
        assert_eq!(pkg.kind, MsvcupPackageKind::Msvc);
        assert_eq!(pkg.version, "14.30.17.6");

        let pkg = MsvcupPackage::from_string("sdk-10.0.22621.7").unwrap();
        assert_eq!(pkg.kind, MsvcupPackageKind::Sdk);
        assert_eq!(pkg.version, "10.0.22621.7");
    }

    #[test]
    fn msvcup_package_from_string_invalid_name() {
        let err = MsvcupPackage::from_string("unknown-1.0").unwrap_err();
        assert!(matches!(err, MsvcupPackageParseError::UnknownName));
    }

    #[test]
    fn msvcup_package_from_string_invalid_version() {
        let err = MsvcupPackage::from_string("msvc-abc").unwrap_err();
        assert!(matches!(err, MsvcupPackageParseError::InvalidVersion(_)));
    }

    #[test]
    fn msvcup_package_display() {
        let pkg = MsvcupPackage::new(MsvcupPackageKind::Msvc, "14.30.17.6");
        assert_eq!(format!("{}", pkg), "msvc-14.30.17.6");
    }

    #[test]
    fn msvcup_package_pool_string() {
        let pkg = MsvcupPackage::new(MsvcupPackageKind::Sdk, "10.0.22621.7");
        assert_eq!(pkg.pool_string(), "sdk-10.0.22621.7");
    }

    #[test]
    fn msvcup_package_order_by_kind_first() {
        let msvc = MsvcupPackage::new(MsvcupPackageKind::Msvc, "14.30.17.6");
        let sdk = MsvcupPackage::new(MsvcupPackageKind::Sdk, "10.0.22621.7");
        assert_eq!(MsvcupPackage::order(&msvc, &sdk), Ordering::Less);
    }

    #[test]
    fn msvcup_package_order_by_version() {
        let a = MsvcupPackage::new(MsvcupPackageKind::Msvc, "14.30.17.6");
        let b = MsvcupPackage::new(MsvcupPackageKind::Msvc, "14.31.0.0");
        assert_eq!(MsvcupPackage::order(&a, &b), Ordering::Less);
        assert_eq!(MsvcupPackage::order(&b, &a), Ordering::Greater);
        assert_eq!(MsvcupPackage::order(&a, &a), Ordering::Equal);
    }

    // --- PackageId / identify_package tests ---

    #[test]
    fn identify_msvc_host_target() {
        let id = "Microsoft.VC.14.43.Tools.HostX64.TargetX64.base";
        match identify_package(id) {
            PackageId::MsvcVersionHostTarget {
                build_version,
                host_arch,
                target_arch,
                name,
            } => {
                assert_eq!(build_version, "14.43");
                assert_eq!(host_arch, Arch::X64);
                assert_eq!(target_arch, Arch::X64);
                assert_eq!(name, "base");
            }
            other => panic!("expected MsvcVersionHostTarget, got {:?}", other),
        }
    }

    #[test]
    fn identify_msvc_host_target_arm64() {
        let id = "Microsoft.VC.14.43.Tools.HostArm64.TargetArm64.base";
        match identify_package(id) {
            PackageId::MsvcVersionHostTarget {
                host_arch,
                target_arch,
                ..
            } => {
                assert_eq!(host_arch, Arch::Arm64);
                assert_eq!(target_arch, Arch::Arm64);
            }
            other => panic!("expected MsvcVersionHostTarget, got {:?}", other),
        }
    }

    #[test]
    fn identify_msbuild() {
        assert!(matches!(
            identify_package("Microsoft.Build"),
            PackageId::Msbuild("170")
        ));
        assert!(matches!(
            identify_package("Microsoft.Build.Arm64"),
            PackageId::Msbuild("170")
        ));
        assert!(matches!(
            identify_package("Microsoft.VisualStudio.VC.MSBuild.v170"),
            PackageId::Msbuild("170")
        ));
    }

    #[test]
    fn identify_diasdk() {
        assert!(matches!(
            identify_package("Microsoft.VisualCpp.DIA.SDK"),
            PackageId::Diasdk
        ));
    }

    #[test]
    fn identify_ninja() {
        match identify_package("ninja-1.12.1") {
            PackageId::Ninja(v) => assert_eq!(v, "1.12.1"),
            other => panic!("expected Ninja, got {:?}", other),
        }
    }

    #[test]
    fn identify_cmake() {
        match identify_package("cmake-3.31.4") {
            PackageId::Cmake(v) => assert_eq!(v, "3.31.4"),
            other => panic!("expected Cmake, got {:?}", other),
        }
    }

    #[test]
    fn identify_unknown() {
        assert!(matches!(
            identify_package("some.random.package"),
            PackageId::Unknown
        ));
        assert!(matches!(identify_package(""), PackageId::Unknown));
    }

    // --- PayloadId / identify_payload tests ---

    #[test]
    fn identify_sdk_payloads() {
        assert_eq!(
            identify_payload(
                "Installers\\Universal CRT Headers Libraries and Sources-x86_en-us.msi",
                Arch::X64
            ),
            PayloadId::Sdk
        );
        assert_eq!(
            identify_payload(
                "Installers\\Windows SDK Signing Tools-x86_en-us.msi",
                Arch::X64
            ),
            PayloadId::Sdk
        );
    }

    #[test]
    fn identify_sdk_arch_specific_headers() {
        assert_eq!(
            identify_payload(
                "Installers\\Windows SDK Desktop Headers x64-x86_en-us.msi",
                Arch::X64
            ),
            PayloadId::Sdk
        );
        assert_eq!(
            identify_payload(
                "Installers\\Windows SDK Desktop Headers arm64-x86_en-us.msi",
                Arch::X64
            ),
            PayloadId::Unknown
        );
    }

    #[test]
    fn identify_sdk_arch_specific_libs() {
        assert_eq!(
            identify_payload(
                "Installers\\Windows SDK Desktop Libs x64-x86_en-us.msi",
                Arch::X64
            ),
            PayloadId::Sdk
        );
        assert_eq!(
            identify_payload(
                "Installers\\Windows SDK Desktop Libs arm64-x86_en-us.msi",
                Arch::Arm64
            ),
            PayloadId::Sdk
        );
    }

    #[test]
    fn identify_unknown_payload() {
        assert_eq!(
            identify_payload("Installers\\Something else.msi", Arch::X64),
            PayloadId::Unknown
        );
    }

    // --- LockFileUrlKind tests ---

    #[test]
    fn lock_file_url_kind() {
        assert_eq!(
            get_lock_file_url_kind("https://example.com/file.vsix"),
            Some(LockFileUrlKind::Vsix)
        );
        assert_eq!(
            get_lock_file_url_kind("https://example.com/file.msi"),
            Some(LockFileUrlKind::Msi)
        );
        assert_eq!(
            get_lock_file_url_kind("https://example.com/file.cab"),
            Some(LockFileUrlKind::Cab)
        );
        assert_eq!(
            get_lock_file_url_kind("https://example.com/file.zip"),
            Some(LockFileUrlKind::Zip)
        );
        assert_eq!(get_lock_file_url_kind("https://example.com/file.exe"), None);
        assert_eq!(get_lock_file_url_kind(""), None);
    }

    // --- Language tests ---

    #[test]
    fn language_from_str() {
        assert_eq!(Language::from_str("neutral"), Language::Neutral);
        assert_eq!(Language::from_str("en-US"), Language::EnUs);
        assert_eq!(Language::from_str("En-Us"), Language::EnUs);
        assert_eq!(Language::from_str("fr-FR"), Language::Other);
        assert_eq!(Language::from_str("zh-CN"), Language::Other);
    }

    // --- get_install_pkg tests ---

    #[test]
    fn get_install_pkg_msvc_matching_arch() {
        let result = get_install_pkg(
            "Microsoft.VC.14.43.Tools.HostX64.TargetX64.base",
            Arch::X64,
            Arch::X64,
        );
        assert!(result.is_some());
        match result.unwrap() {
            InstallPkgKind::Msvc(v) => assert_eq!(v, "14.43"),
            other => panic!("expected Msvc, got {:?}", other),
        }
    }

    #[test]
    fn get_install_pkg_msvc_wrong_host() {
        let result = get_install_pkg(
            "Microsoft.VC.14.43.Tools.HostArm64.TargetX64.base",
            Arch::X64,
            Arch::X64,
        );
        assert!(result.is_none());
    }

    #[test]
    fn get_install_pkg_msvc_wrong_target() {
        let result = get_install_pkg(
            "Microsoft.VC.14.43.Tools.HostX64.TargetArm64.base",
            Arch::X64,
            Arch::X64,
        );
        assert!(result.is_none());
    }

    #[test]
    fn get_install_pkg_msbuild() {
        let result = get_install_pkg("Microsoft.Build", Arch::X64, Arch::X64);
        assert!(matches!(result, Some(InstallPkgKind::Msbuild(_))));
    }

    #[test]
    fn get_install_pkg_diasdk() {
        let result = get_install_pkg("Microsoft.VisualCpp.DIA.SDK", Arch::X64, Arch::X64);
        assert!(matches!(result, Some(InstallPkgKind::Diasdk)));
    }

    #[test]
    fn get_install_pkg_unknown() {
        let result = get_install_pkg("some.random.package", Arch::X64, Arch::X64);
        assert!(result.is_none());
    }

    // --- MsvcupPackageParseError Display ---

    #[test]
    fn parse_error_display() {
        let err = MsvcupPackageParseError::UnknownName;
        assert_eq!(format!("{}", err), "unknown package name");

        let err = MsvcupPackageParseError::InvalidVersion("abc".to_string());
        assert_eq!(format!("{}", err), "invalid version 'abc'");
    }
}
