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
    if msvcup_pkgs.is_empty() {
        return Some("no packages to check against".to_string());
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arch::Arch;

    fn make_lock_json(packages: &[&str]) -> String {
        let pkgs: Vec<String> = packages
            .iter()
            .map(|name| format!(r#"{{"name": "{}", "payloads": []}}"#, name))
            .collect();
        format!(r#"{{"packages": [{}]}}"#, pkgs.join(","))
    }

    #[test]
    fn parse_lock_file_valid() {
        let json = r#"{
            "cabs": {
                "test.cab": {"url": "https://example.com/test.cab", "sha256": "abc123"}
            },
            "packages": [
                {
                    "name": "msvc-14.43.34808",
                    "payloads": [
                        {"url": "https://example.com/file.vsix", "sha256": "def456"}
                    ]
                }
            ]
        }"#;
        let result = parse_lock_file("test.lock", json).unwrap();
        assert_eq!(result.packages.len(), 1);
        assert_eq!(result.packages[0].name, "msvc-14.43.34808");
        assert_eq!(result.packages[0].payloads.len(), 1);
        assert_eq!(result.cabs.len(), 1);
    }

    #[test]
    fn parse_lock_file_no_cabs() {
        let json = r#"{"packages": []}"#;
        let result = parse_lock_file("test.lock", json).unwrap();
        assert!(result.cabs.is_empty());
        assert!(result.packages.is_empty());
    }

    #[test]
    fn parse_lock_file_invalid_json() {
        let result = parse_lock_file("test.lock", "not json");
        assert!(result.is_err());
    }

    #[test]
    fn check_lock_file_pkgs_matching() {
        let pkgs = vec![
            MsvcupPackage::new(MsvcupPackageKind::Msvc, "14.43.34808"),
            MsvcupPackage::new(MsvcupPackageKind::Sdk, "10.0.22621.7"),
        ];
        let json = make_lock_json(&["msvc-14.43.34808", "sdk-10.0.22621.7"]);
        assert!(check_lock_file_pkgs("test.lock", &json, &pkgs).is_none());
    }

    #[test]
    fn check_lock_file_pkgs_missing_package() {
        let pkgs = vec![
            MsvcupPackage::new(MsvcupPackageKind::Msvc, "14.43.34808"),
            MsvcupPackage::new(MsvcupPackageKind::Sdk, "10.0.22621.7"),
        ];
        let json = make_lock_json(&["msvc-14.43.34808"]);
        let result = check_lock_file_pkgs("test.lock", &json, &pkgs);
        assert!(result.is_some());
        assert!(result.unwrap().contains("missing"));
    }

    #[test]
    fn check_lock_file_pkgs_extra_package() {
        let pkgs = vec![MsvcupPackage::new(MsvcupPackageKind::Msvc, "14.43.34808")];
        let json = make_lock_json(&["msvc-14.43.34808", "sdk-10.0.22621.7"]);
        let result = check_lock_file_pkgs("test.lock", &json, &pkgs);
        assert!(result.is_some());
        assert!(result.unwrap().contains("extra"));
    }

    #[test]
    fn check_lock_file_pkgs_empty_input() {
        let json = make_lock_json(&[]);
        let result = check_lock_file_pkgs("test.lock", &json, &[]);
        assert!(result.is_some());
        assert!(result.unwrap().contains("no packages"));
    }

    #[test]
    fn check_lock_file_pkgs_invalid_json() {
        let pkgs = vec![MsvcupPackage::new(MsvcupPackageKind::Msvc, "14.43.34808")];
        let result = check_lock_file_pkgs("test.lock", "not json", &pkgs);
        assert!(result.is_some());
        assert!(result.unwrap().contains("parse error"));
    }

    #[test]
    fn strip_root_dir_only_cmake() {
        assert!(strip_root_dir(MsvcupPackageKind::Cmake));
        assert!(!strip_root_dir(MsvcupPackageKind::Msvc));
        assert!(!strip_root_dir(MsvcupPackageKind::Sdk));
        assert!(!strip_root_dir(MsvcupPackageKind::Msbuild));
        assert!(!strip_root_dir(MsvcupPackageKind::Diasdk));
        assert!(!strip_root_dir(MsvcupPackageKind::Ninja));
    }

    #[test]
    fn host_arch_limit_msvc_returns_none() {
        assert!(host_arch_limit(MsvcupPackageKind::Msvc, "anything").is_none());
        assert!(host_arch_limit(MsvcupPackageKind::Sdk, "anything").is_none());
        assert!(host_arch_limit(MsvcupPackageKind::Msbuild, "anything").is_none());
        assert!(host_arch_limit(MsvcupPackageKind::Diasdk, "anything").is_none());
    }

    #[test]
    fn host_arch_limit_ninja() {
        let url = "https://github.com/ninja-build/ninja/releases/download/v1.12.1/ninja-win.zip";
        assert_eq!(
            host_arch_limit(MsvcupPackageKind::Ninja, url),
            Some(Arch::X64)
        );

        let url_arm =
            "https://github.com/ninja-build/ninja/releases/download/v1.12.1/ninja-winarm64.zip";
        assert_eq!(
            host_arch_limit(MsvcupPackageKind::Ninja, url_arm),
            Some(Arch::Arm64)
        );
    }

    #[test]
    fn host_arch_limit_cmake() {
        let url = "https://github.com/Kitware/CMake/releases/download/v3.31.4/cmake-3.31.4-windows-x86_64.zip";
        assert_eq!(
            host_arch_limit(MsvcupPackageKind::Cmake, url),
            Some(Arch::X64)
        );
    }

    #[test]
    fn lockfile_json_serialization_roundtrip() {
        let lock_file = LockFileJson {
            cabs: HashMap::new(),
            packages: vec![LockFilePackage {
                name: "msvc-14.43.34808".to_string(),
                payloads: vec![LockFilePayloadEntry {
                    url: "https://example.com/file.vsix".to_string(),
                    sha256: "abc123".to_string(),
                }],
            }],
        };
        let json = serde_json::to_string(&lock_file).unwrap();
        let parsed: LockFileJson = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.packages.len(), 1);
        assert_eq!(parsed.packages[0].name, "msvc-14.43.34808");
    }
}
