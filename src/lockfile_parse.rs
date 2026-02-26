use crate::packages::{MsvcupPackage, MsvcupPackageKind};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// JSON lock file schema
#[derive(Debug, Serialize, Deserialize)]
pub struct LockFileJson {
    /// CAB files shared by MSI payloads: filename -> CabEntry
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub cabs: HashMap<String, CabEntry>,
    /// Top-level payloads grouped by package (e.g., "msvc-14.43.34808")
    pub packages: Vec<LockFilePackage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CabEntry {
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LockFilePackage {
    pub name: String,
    pub payloads: Vec<LockFilePayloadEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LockFilePayloadEntry {
    pub url: String,
    pub sha256: String,
}

/// Whether this package type requires stripping the root directory during extraction.
pub fn strip_root_dir(pkg_kind: MsvcupPackageKind) -> bool {
    matches!(pkg_kind, MsvcupPackageKind::Cmake)
}

/// If this package type is host-architecture-specific, parse the arch from the URL.
pub fn host_arch_limit(pkg_kind: MsvcupPackageKind, url: &str) -> Option<crate::arch::Arch> {
    match pkg_kind {
        MsvcupPackageKind::Msvc
        | MsvcupPackageKind::Sdk
        | MsvcupPackageKind::Msbuild
        | MsvcupPackageKind::Diasdk => None,
        MsvcupPackageKind::Ninja | MsvcupPackageKind::Cmake => match crate::extra::parse_url(url) {
            crate::extra::ParseUrlResult::Ok { arch } => Some(arch),
            crate::extra::ParseUrlResult::Unexpected { .. } => None,
        },
    }
}

pub fn parse_lock_file(lock_file_path: &str, content: &str) -> Result<LockFileJson> {
    serde_json::from_str(content)
        .map_err(|e| anyhow::anyhow!("{}: failed to parse JSON lock file: {}", lock_file_path, e))
}

/// Check if the lock file's packages match what we want to install.
/// Returns None if they match, Some(reason) if they don't.
pub fn check_lock_file_pkgs(
    _lock_file_path: &str,
    lock_file_content: &str,
    msvcup_pkgs: &[MsvcupPackage],
) -> Option<String> {
    assert!(!msvcup_pkgs.is_empty());

    let lock_file: LockFileJson = match serde_json::from_str(lock_file_content) {
        Ok(lf) => lf,
        Err(e) => return Some(format!("parse error: {}", e)),
    };

    let lock_pkg_names: Vec<&str> = lock_file.packages.iter().map(|p| p.name.as_str()).collect();

    for msvcup_pkg in msvcup_pkgs {
        let name = msvcup_pkg.pool_string();
        if !lock_pkg_names.contains(&name.as_str()) {
            return Some(format!("lock file is missing package '{}'", msvcup_pkg));
        }
    }

    for lock_pkg in &lock_file.packages {
        let found = msvcup_pkgs.iter().any(|p| p.pool_string() == lock_pkg.name);
        if !found {
            return Some(format!("lock file has extra package '{}'", lock_pkg.name));
        }
    }

    None
}
