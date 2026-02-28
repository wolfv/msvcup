use crate::arch::Arch;
use crate::packages::{MsvcupPackage, MsvcupPackageKind};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct MsvcupConfig {
    pub msvcup: MsvcupSettings,
    pub packages: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MsvcupSettings {
    /// Cache directory for downloaded files
    pub cache_dir: Option<String>,
    /// Installation directory for extracted packages
    pub install_dir: Option<String>,
    /// Path to the lock file (relative to config file location)
    pub lock_file: String,
    /// Target architecture (x64, x86, arm64, arm)
    pub target_arch: String,
}

impl MsvcupConfig {
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = fs_err::read_to_string(path)?;
        let config: MsvcupConfig = toml::from_str(&content)
            .with_context(|| format!("parsing config file '{}'", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if Arch::from_str_exact(&self.msvcup.target_arch).is_none() {
            bail!(
                "invalid target_arch '{}', expected one of: x64, x86, arm, arm64",
                self.msvcup.target_arch
            );
        }
        for (name, version) in &self.packages {
            if MsvcupPackageKind::from_prefix(&format!("{}-{}", name, version)).is_none() {
                bail!(
                    "unknown package '{}', expected one of: msvc, sdk, msbuild, diasdk, ninja, cmake",
                    name
                );
            }
        }
        if self.packages.is_empty() {
            bail!("no packages specified in config");
        }
        Ok(())
    }

    pub fn target_arch(&self) -> Arch {
        Arch::from_str_exact(&self.msvcup.target_arch).unwrap()
    }

    pub fn msvcup_packages(&self) -> Result<Vec<MsvcupPackage>> {
        let mut pkgs = Vec::new();
        for (name, version) in &self.packages {
            let pkg_str = format!("{}-{}", name, version);
            let pkg = MsvcupPackage::from_string(&pkg_str)
                .map_err(|e| anyhow::anyhow!("invalid package '{}': {}", pkg_str, e))?;
            crate::util::insert_sorted(&mut pkgs, pkg, MsvcupPackage::order);
        }
        Ok(pkgs)
    }

    /// Resolve the lock file path relative to the config file's directory
    pub fn lock_file_path(&self, config_path: &Path) -> std::path::PathBuf {
        let config_dir = config_path.parent().unwrap_or(Path::new("."));
        config_dir.join(&self.msvcup.lock_file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse and validate a config from a TOML string (test helper).
    fn from_toml_str(content: &str) -> Result<MsvcupConfig> {
        let config: MsvcupConfig =
            toml::from_str(content).with_context(|| "parsing config TOML")?;
        config.validate()?;
        Ok(config)
    }

    fn valid_config_toml() -> &'static str {
        r#"
[msvcup]
lock_file = "msvc.lock"
target_arch = "x64"

[packages]
msvc = "14.43.34808"
sdk = "10.0.22621.7"
"#
    }

    #[test]
    fn parse_valid_config() {
        let config = from_toml_str(valid_config_toml()).unwrap();
        assert_eq!(config.msvcup.target_arch, "x64");
        assert_eq!(config.msvcup.lock_file, "msvc.lock");
        assert!(config.msvcup.cache_dir.is_none());
        assert!(config.msvcup.install_dir.is_none());
        assert_eq!(config.packages.len(), 2);
    }

    #[test]
    fn target_arch_returns_correct_arch() {
        let config = from_toml_str(valid_config_toml()).unwrap();
        assert_eq!(config.target_arch(), Arch::X64);
    }

    #[test]
    fn msvcup_packages_returns_sorted() {
        let config = from_toml_str(valid_config_toml()).unwrap();
        let pkgs = config.msvcup_packages().unwrap();
        assert_eq!(pkgs.len(), 2);
        // Msvc < Sdk in the ordering
        assert_eq!(pkgs[0].kind, MsvcupPackageKind::Msvc);
        assert_eq!(pkgs[1].kind, MsvcupPackageKind::Sdk);
    }

    #[test]
    fn reject_invalid_target_arch() {
        let toml = r#"
[msvcup]
lock_file = "msvc.lock"
target_arch = "riscv64"

[packages]
msvc = "14.43.34808"
"#;
        let err = from_toml_str(toml).unwrap_err();
        assert!(err.to_string().contains("invalid target_arch"));
    }

    #[test]
    fn reject_unknown_package() {
        let toml = r#"
[msvcup]
lock_file = "msvc.lock"
target_arch = "x64"

[packages]
unknown_pkg = "1.0"
"#;
        let err = from_toml_str(toml).unwrap_err();
        assert!(err.to_string().contains("unknown package"));
    }

    #[test]
    fn reject_empty_packages() {
        let toml = r#"
[msvcup]
lock_file = "msvc.lock"
target_arch = "x64"

[packages]
"#;
        let err = from_toml_str(toml).unwrap_err();
        assert!(err.to_string().contains("no packages"));
    }

    #[test]
    fn lock_file_path_relative_to_config() {
        let config = from_toml_str(valid_config_toml()).unwrap();
        let path = config.lock_file_path(Path::new("/some/dir/msvcup.toml"));
        assert_eq!(path, Path::new("/some/dir/msvc.lock"));
    }

    #[test]
    fn lock_file_path_config_in_current_dir() {
        let config = from_toml_str(valid_config_toml()).unwrap();
        let path = config.lock_file_path(Path::new("msvcup.toml"));
        assert_eq!(path, Path::new("msvc.lock"));
    }

    #[test]
    fn config_with_optional_fields() {
        let toml = r#"
[msvcup]
lock_file = "msvc.lock"
target_arch = "arm64"
cache_dir = "/tmp/cache"
install_dir = "/opt/msvc"

[packages]
msvc = "14.43.34808"
"#;
        let config = from_toml_str(toml).unwrap();
        assert_eq!(config.msvcup.cache_dir.as_deref(), Some("/tmp/cache"));
        assert_eq!(config.msvcup.install_dir.as_deref(), Some("/opt/msvc"));
        assert_eq!(config.target_arch(), Arch::Arm64);
    }

    #[test]
    fn config_from_file_nonexistent() {
        let result = MsvcupConfig::from_file(Path::new("/nonexistent/path/msvcup.toml"));
        assert!(result.is_err());
    }
}
