use crate::autoenv_cmd;
use crate::config::MsvcupConfig;
use crate::install;
use crate::manifest::MsvcupDir;
use crate::packages::{ManifestUpdate, MsvcupPackageKind, get_packages};
use anyhow::Result;
use fs_err as fs;
use std::path::{Path, PathBuf};

pub async fn resolve_command(
    client: &reqwest::Client,
    msvcup_dir: &MsvcupDir,
    config_path: &str,
    out_dir: &str,
    manifest_update: ManifestUpdate,
) -> Result<()> {
    let config_path = Path::new(config_path);
    let config = MsvcupConfig::from_file(config_path)?;
    let msvcup_pkgs = config.msvcup_packages()?;
    let target_arch = config.target_arch();
    let lock_file_path = config.lock_file_path(config_path);
    let lock_file_str = lock_file_path.to_str().unwrap();

    // Step 1: Resolve packages and generate/update the lock file
    log::info!("resolving packages...");

    let try_no_update = match manifest_update {
        ManifestUpdate::Off => true,
        ManifestUpdate::Daily => unimplemented!("daily manifest update"),
        ManifestUpdate::Always => false,
    };

    let need_manifest_update = if try_no_update {
        if let Ok(content) = fs::read_to_string(&lock_file_path) {
            if crate::lockfile_parse::check_lock_file_pkgs(lock_file_str, &content, &msvcup_pkgs)
                .is_none()
            {
                log::info!("lock file is up-to-date");
                false
            } else {
                true
            }
        } else {
            true
        }
    } else {
        true
    };

    if need_manifest_update {
        let (vsman_path, vsman_content) = crate::manifest::read_vs_manifest(
            client,
            msvcup_dir,
            crate::channel_kind::ChannelKind::Release,
            manifest_update,
        )
        .await?;

        let pkgs = get_packages(vsman_path.to_str().unwrap(), &vsman_content)?;
        install::update_lock_file(&msvcup_pkgs, lock_file_str, &pkgs)?;
        log::info!("lock file updated: '{}'", lock_file_str);
    }

    // Step 2: Create output directory and place shim binaries + config
    fs::create_dir_all(out_dir)?;

    // Copy the config file to the output directory
    let out_config_path = Path::new(out_dir).join("msvcup.toml");
    update_file_from_file(config_path, &out_config_path)?;

    // Copy the lock file to the output directory
    let out_lock_name = lock_file_path.file_name().unwrap();
    let out_lock_path = Path::new(out_dir).join(out_lock_name);
    update_file_from_file(&lock_file_path, &out_lock_path)?;

    // If the lock file name in the config is not just the filename, update the config copy
    // to point to the lock file in the same directory
    let lock_file_basename = lock_file_path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    if config.msvcup.lock_file != lock_file_basename {
        let mut out_config = config;
        out_config.msvcup.lock_file = lock_file_basename;
        let toml_str = toml::to_string_pretty(&out_config)?;
        fs::write(&out_config_path, toml_str)?;
    }

    // Step 3: Place shim executables and msvcup binaries
    let (autoenv_exe, msvcup_exe) = find_binaries()?;

    // Place msvcup-autoenv.exe and msvcup.exe so `msvcup-autoenv install` can find msvcup
    let out_autoenv = Path::new(out_dir).join("msvcup-autoenv.exe");
    update_file_from_file(&autoenv_exe, &out_autoenv)?;
    let out_msvcup = Path::new(out_dir).join("msvcup.exe");
    update_file_from_file(&msvcup_exe, &out_msvcup)?;

    let has_msvc = msvcup_pkgs
        .iter()
        .any(|p| p.kind == MsvcupPackageKind::Msvc);
    let has_sdk = msvcup_pkgs.iter().any(|p| p.kind == MsvcupPackageKind::Sdk);

    if has_msvc {
        for tool in autoenv_cmd::MSVC_TOOLS {
            let dest = Path::new(out_dir).join(format!("{}.exe", tool.name));
            update_file_from_file(&autoenv_exe, &dest)?;
        }
    }
    if has_sdk {
        for tool in autoenv_cmd::SDK_TOOLS {
            let dest = Path::new(out_dir).join(format!("{}.exe", tool.name));
            update_file_from_file(&autoenv_exe, &dest)?;
        }
    }

    // Step 4: Generate toolchain.cmake
    let cmake = autoenv_cmd::generate_toolchain_cmake(target_arch, has_msvc, has_sdk);
    let cmake_path = Path::new(out_dir).join("toolchain.cmake");
    update_file(&cmake_path, cmake.as_bytes())?;

    log::info!("shims placed in '{}'", out_dir);
    log::info!(
        "run 'msvcup-autoenv install' in '{}' to install packages",
        out_dir
    );

    Ok(())
}

/// Find the msvcup-autoenv and msvcup binaries next to the current executable.
fn find_binaries() -> Result<(PathBuf, PathBuf)> {
    let current_exe = std::env::current_exe()?;
    let dir = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot determine directory of current executable"))?;

    let autoenv = find_binary_in_dir(dir, &["msvcup-autoenv.exe", "msvcup-autoenv"])
        .ok_or_else(|| {
            anyhow::anyhow!(
                "cannot find msvcup-autoenv binary in '{}'. Build it with: cargo build --bin msvcup-autoenv",
                dir.display()
            )
        })?;

    let msvcup = find_binary_in_dir(dir, &["msvcup.exe", "msvcup"]).ok_or_else(|| {
        anyhow::anyhow!(
            "cannot find msvcup binary in '{}'. Build it with: cargo build --bin msvcup",
            dir.display()
        )
    })?;

    Ok((autoenv, msvcup))
}

fn find_binary_in_dir(dir: &Path, candidates: &[&str]) -> Option<PathBuf> {
    for name in candidates {
        let path = dir.join(name);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

fn update_file_from_file(src: &Path, dest: &Path) -> Result<()> {
    let src_content = fs::read(src)?;
    let needs_update = match fs::read(dest) {
        Ok(existing) => existing != src_content,
        Err(_) => true,
    };
    if needs_update {
        log::info!("{}: updating...", dest.display());
        fs::copy(src, dest)?;
    } else {
        log::info!("{}: already up-to-date", dest.display());
    }
    Ok(())
}

fn update_file(path: &Path, content: &[u8]) -> Result<()> {
    let needs_update = match fs::read(path) {
        Ok(existing) => existing != content,
        Err(_) => true,
    };
    if needs_update {
        log::info!("{}: updating...", path.display());
        fs::write(path, content)?;
    } else {
        log::info!("{}: already up-to-date", path.display());
    }
    Ok(())
}
