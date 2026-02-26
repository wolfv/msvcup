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
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file '{}'", path.display()))?;
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
