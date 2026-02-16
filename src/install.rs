use crate::arch::Arch;
use crate::lock_file::LockFile;
use crate::lockfile_parse::{
    LockFilePayloadKind, check_lock_file_pkgs, parse_lock_file_payload, write_payload,
};
use crate::manifest::{MsvcupDir, fetch};
use crate::packages::{
    InstallPkgKind, LockFileUrlKind, ManifestUpdate, MsvcupPackage, MsvcupPackageKind, Packages,
    PayloadId, get_install_pkg, get_lock_file_url_kind, get_packages, identify_payload,
};
use crate::sha::Sha256;
use crate::util::{basename_from_url, insert_sorted};
use crate::zip_extract::{self, ZipKind};
use anyhow::{Context, Result, bail};
use std::cmp::Ordering;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

pub fn install_command(
    client: &reqwest::blocking::Client,
    msvcup_dir: &MsvcupDir,
    msvcup_pkgs: &[MsvcupPackage],
    lock_file_path: &str,
    manifest_update: ManifestUpdate,
    cache_dir: Option<&str>,
) -> Result<()> {
    if msvcup_pkgs.is_empty() {
        bail!("no packages were given to install, use 'list' to list the available packages");
    }

    let cache_dir = cache_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| msvcup_dir.path(&["cache"]));
    let cache_dir_str = cache_dir.to_str().unwrap();

    let try_no_update = match manifest_update {
        ManifestUpdate::Off => true,
        ManifestUpdate::Daily => unimplemented!("daily manifest update"),
        ManifestUpdate::Always => false,
    };

    if try_no_update {
        if let Ok(content) = fs::read_to_string(lock_file_path) {
            log::info!("lock file found: '{}'", lock_file_path);
            if let Some(mismatch) = check_lock_file_pkgs(lock_file_path, &content, msvcup_pkgs) {
                log::info!("{}", mismatch);
            } else {
                match install_from_lock_file(
                    client,
                    msvcup_pkgs,
                    msvcup_dir,
                    cache_dir_str,
                    lock_file_path,
                    &content,
                )? {
                    InstallResult::Success => return Ok(()),
                    InstallResult::VersionMismatch => {}
                }
            }
        } else {
            log::info!("lock file NOT found: '{}'", lock_file_path);
        }
    }

    // Read VS manifest and update lock file
    let (vsman_path, vsman_content) = crate::manifest::read_vs_manifest(
        client,
        msvcup_dir,
        crate::channel_kind::ChannelKind::Release,
        ManifestUpdate::Off,
    )?;

    let pkgs = get_packages(vsman_path.to_str().unwrap(), &vsman_content)?;

    update_lock_file(client, msvcup_pkgs, lock_file_path, &pkgs, cache_dir_str)?;

    let lock_file_content = fs::read_to_string(lock_file_path)
        .with_context(|| format!("reading lock file '{}' after update", lock_file_path))?;

    if let Some(mismatch) = check_lock_file_pkgs(lock_file_path, &lock_file_content, msvcup_pkgs) {
        bail!(
            "lock file '{}' still doesn't match after update: {}",
            lock_file_path,
            mismatch
        );
    }

    match install_from_lock_file(
        client,
        msvcup_pkgs,
        msvcup_dir,
        cache_dir_str,
        lock_file_path,
        &lock_file_content,
    )? {
        InstallResult::Success => Ok(()),
        InstallResult::VersionMismatch => bail!("lock file version mismatch even after update"),
    }
}

enum InstallResult {
    Success,
    VersionMismatch,
}

fn install_from_lock_file(
    client: &reqwest::blocking::Client,
    msvcup_pkgs: &[MsvcupPackage],
    msvcup_dir: &MsvcupDir,
    cache_dir: &str,
    lock_file_path: &str,
    lock_file_content: &str,
) -> Result<InstallResult> {
    let mut save_cab_lines: Vec<String> = Vec::new();

    for line in lock_file_content.lines() {
        if line.is_empty() {
            continue;
        }
        let parsed = parse_lock_file_payload(lock_file_path, 0, line)?;

        // Skip payloads for non-native architectures
        if let Some(host_arch_limit) = parsed.host_arch_limit()
            && Arch::native() != Some(host_arch_limit) {
                let name = basename_from_url(&parsed.url_decoded);
                log::info!(
                    "skipping payload '{}' arch {} != host arch {:?}",
                    name,
                    host_arch_limit,
                    Arch::native()
                );
                continue;
            }

        match &parsed.url_kind {
            LockFilePayloadKind::TopLevel(payload_msvcup_pkg) => {
                let cabs_content = save_cab_lines.join("\n");
                save_cab_lines.clear();

                let install_path = msvcup_dir.path(&[&payload_msvcup_pkg.pool_string()]);
                install_payload(
                    client,
                    &install_path,
                    lock_file_path,
                    cache_dir,
                    &parsed.url_decoded,
                    &parsed.sha256,
                    parsed.strip_root_dir(),
                    &cabs_content,
                )?;
            }
            LockFilePayloadKind::Cab(_) => {
                save_cab_lines.push(line.to_string());
            }
        }
    }

    // Finish packages (generate vcvars bat files)
    for msvcup_pkg in msvcup_pkgs {
        finish_package(msvcup_dir, msvcup_pkg)?;
    }

    Ok(InstallResult::Success)
}

fn fetch_payload(
    client: &reqwest::blocking::Client,
    _cache_dir: &str,
    sha256: &Sha256,
    url_decoded: &str,
    cache_path: &Path,
) -> Result<()> {
    let cache_lock_path = format!("{}.lock", cache_path.display());
    let _cache_lock = LockFile::lock(&cache_lock_path)?;

    if cache_path.exists() {
        log::info!("ALREADY FETCHED  | {} {}", url_decoded, sha256);
    } else {
        log::info!("FETCHING         | {} {}", url_decoded, sha256);
        let fetch_path = PathBuf::from(format!("{}.fetching", cache_path.display()));
        let actual_sha256 = fetch(client, url_decoded, &fetch_path)?;
        if actual_sha256 != *sha256 {
            log::error!(
                "SHA256 mismatch:\nexpected: {}\nactual  : {}",
                sha256,
                actual_sha256
            );
            std::process::exit(0xff);
        }
        fs::rename(&fetch_path, cache_path)?;
    }
    Ok(())
}

fn cache_entry_path(cache_dir: &str, sha256: &Sha256, name: &str) -> PathBuf {
    let basename = format!("{}-{}", sha256, name);
    PathBuf::from(cache_dir).join(basename)
}

#[allow(clippy::too_many_arguments)]
fn install_payload(
    client: &reqwest::blocking::Client,
    install_dir_path: &Path,
    lock_file_path: &str,
    cache_dir: &str,
    url_decoded: &str,
    sha256: &Sha256,
    strip_root_dir: bool,
    cabs: &str,
) -> Result<()> {
    let url_kind = get_lock_file_url_kind(url_decoded).ok_or_else(|| {
        anyhow::anyhow!(
            "unable to determine install kind from URL '{}'",
            url_decoded
        )
    })?;

    let cache_path = cache_entry_path(cache_dir, sha256, basename_from_url(url_decoded));

    let installed_basename = format!(
        "{}.files",
        cache_path.file_name().unwrap().to_str().unwrap()
    );
    let installed_manifest_path = install_dir_path.join("install").join(&installed_basename);

    if installed_manifest_path.exists() {
        log::info!(
            "ALREADY INSTALLED | {} {}",
            basename_from_url(url_decoded),
            sha256
        );
        return Ok(());
    }

    // Fetch cab files first
    for line in cabs.lines() {
        if line.is_empty() {
            continue;
        }
        let parsed = parse_lock_file_payload(lock_file_path, 0, line)?;
        let cab_cache_path = cache_entry_path(
            cache_dir,
            &parsed.sha256,
            basename_from_url(&parsed.url_decoded),
        );
        fetch_payload(
            client,
            cache_dir,
            &parsed.sha256,
            &parsed.url_decoded,
            &cab_cache_path,
        )?;
    }

    // Fetch the main payload
    fetch_payload(client, cache_dir, sha256, url_decoded, &cache_path)?;

    // Create install lock
    let install_lock_path = install_dir_path.join(".lock");
    fs::create_dir_all(install_dir_path)?;
    let _install_lock = LockFile::lock(install_lock_path.to_str().unwrap())?;

    let current_install_path = install_dir_path.join("install").join("current");

    // Handle previous interrupted install
    start_install(install_dir_path, &current_install_path)?;

    // Write install manifest
    let mut manifest_file = fs::File::create(&current_install_path)?;
    writeln!(
        manifest_file,
        "{}",
        cache_path.file_name().unwrap().to_str().unwrap()
    )?;

    match url_kind {
        LockFileUrlKind::Vsix => {
            zip_extract::extract_zip_to_dir(
                &cache_path,
                install_dir_path,
                ZipKind::Vsix,
                strip_root_dir,
                &mut manifest_file,
            )?;
        }
        LockFileUrlKind::Zip => {
            zip_extract::extract_zip_to_dir(
                &cache_path,
                install_dir_path,
                ZipKind::Zip,
                strip_root_dir,
                &mut manifest_file,
            )?;
        }
        LockFileUrlKind::Msi => {
            // MSI installation requires msiexec on Windows
            if cfg!(windows) {
                install_msi(
                    &cache_path,
                    install_dir_path,
                    lock_file_path,
                    cache_dir,
                    cabs,
                    url_decoded,
                    &mut manifest_file,
                )?;
            } else {
                log::warn!(
                    "MSI installation is only supported on Windows, skipping '{}'",
                    url_decoded
                );
            }
        }
        LockFileUrlKind::Cab => unreachable!(),
    }

    drop(manifest_file);
    end_install(&installed_manifest_path, &current_install_path)?;

    Ok(())
}

fn start_install(_install_dir_path: &Path, current_install_path: &Path) -> Result<()> {
    if let Ok(content) = fs::read_to_string(current_install_path) {
        log::info!("found previous install manifest, cleaning up...");
        let mut lines = content.lines();
        if let Some(_cache_basename) = lines.next() {
            for line in lines {
                if line.is_empty() {
                    continue;
                }
                if let Some(sub_path) = line.strip_prefix("new ") {
                    log::info!("removing file '{}'", sub_path);
                    let _ = fs::remove_file(sub_path);
                }
                // "add " lines: don't remove, file was added by another payload
            }
        }
        let _ = fs::remove_file(current_install_path);
    }

    if let Some(dir) = current_install_path.parent() {
        fs::create_dir_all(dir)?;
    }
    Ok(())
}

fn end_install(installed_manifest_path: &Path, current_install_path: &Path) -> Result<()> {
    let tmp_path = PathBuf::from(format!("{}.tmp", installed_manifest_path.display()));

    {
        let content = fs::read_to_string(current_install_path)?;
        let mut out = BufWriter::new(fs::File::create(&tmp_path)?);
        let mut lines = content.lines();
        let _cache_basename = lines.next(); // skip first line
        for line in lines {
            if line.is_empty() {
                continue;
            }
            if let Some(sub_path) = line.strip_prefix("new ") {
                writeln!(out, "{}", sub_path)?;
            } else if let Some(sub_path) = line.strip_prefix("add ") {
                writeln!(out, "{}", sub_path)?;
            }
        }
        out.flush()?;
    }

    fs::remove_file(current_install_path)?;
    if let Some(dir) = installed_manifest_path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::rename(&tmp_path, installed_manifest_path)?;

    Ok(())
}

#[cfg(windows)]
fn install_msi(
    msi_path: &Path,
    install_dir_path: &Path,
    lock_file_path: &str,
    cache_dir: &str,
    cabs: &str,
    url_decoded: &str,
    manifest_file: &mut fs::File,
) -> Result<()> {
    use std::process::Command;

    let staging_dir = install_dir_path.join(".msi-staging");
    let _ = fs::remove_dir_all(&staging_dir);

    let installer_path = staging_dir.join("installer");
    fs::create_dir_all(&installer_path)?;

    // Copy MSI file
    let msi_copy = installer_path.join(basename_from_url(url_decoded));
    fs::copy(msi_path, &msi_copy)?;

    // Copy cab files
    for line in cabs.lines() {
        if line.is_empty() {
            continue;
        }
        let parsed = parse_lock_file_payload(lock_file_path, 0, line)?;
        if let LockFilePayloadKind::Cab(cab_path) = &parsed.url_kind {
            let cab_cache_path = cache_entry_path(
                cache_dir,
                &parsed.sha256,
                basename_from_url(&parsed.url_decoded),
            );
            let dest = installer_path.join(cab_path.trim());
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&cab_cache_path, &dest)?;
        }
    }

    // Run msiexec
    let target_dir = staging_dir.join("target");
    let target_dir_arg = format!("TARGETDIR={}", target_dir.display());
    log::info!("running msiexec for '{}'...", msi_copy.display());

    let output = Command::new("msiexec.exe")
        .args([
            "/a",
            msi_copy.to_str().unwrap(),
            "/quiet",
            "/qn",
            &target_dir_arg,
        ])
        .output()?;

    if !output.status.success() {
        bail!(
            "msiexec for '{}' failed with exit code {:?}",
            msi_copy.display(),
            output.status.code()
        );
    }

    // Install files from staging to install dir
    install_dir_recursive(
        &target_dir,
        install_dir_path,
        basename_from_url(url_decoded),
        manifest_file,
    )?;

    // Clean up staging
    let _ = fs::remove_dir_all(&staging_dir);

    Ok(())
}

#[cfg(not(windows))]
fn install_msi(
    _msi_path: &Path,
    _install_dir_path: &Path,
    _lock_file_path: &str,
    _cache_dir: &str,
    _cabs: &str,
    _url_decoded: &str,
    _manifest_file: &mut fs::File,
) -> Result<()> {
    log::warn!("MSI installation is only supported on Windows");
    Ok(())
}

fn install_dir_recursive(
    source_dir: &Path,
    install_dir: &Path,
    root_exclude: &str,
    manifest_file: &mut fs::File,
) -> Result<()> {
    for entry in walkdir::WalkDir::new(source_dir).into_iter().flatten() {
        let rel_path = entry.path().strip_prefix(source_dir)?;
        if rel_path.to_str() == Some(root_exclude) {
            continue;
        }
        if entry.file_type().is_file() {
            let install_path = install_dir.join(rel_path);
            if install_path.exists() {
                writeln!(manifest_file, "add {}", install_path.display())?;
            } else {
                writeln!(manifest_file, "new {}", install_path.display())?;
                if let Some(parent) = install_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(entry.path(), &install_path)?;
            }
        }
    }
    Ok(())
}

fn finish_package(msvcup_dir: &MsvcupDir, msvcup_pkg: &MsvcupPackage) -> Result<()> {
    let finish_kind = match msvcup_pkg.kind {
        MsvcupPackageKind::Msvc => FinishKind::Msvc,
        MsvcupPackageKind::Sdk => FinishKind::Sdk,
        MsvcupPackageKind::Msbuild
        | MsvcupPackageKind::Diasdk
        | MsvcupPackageKind::Ninja
        | MsvcupPackageKind::Cmake => return Ok(()),
    };

    let install_path = msvcup_dir.path(&[&msvcup_pkg.pool_string()]);
    let install_version = query_install_version(finish_kind, &install_path)?;
    log::info!("{} install version '{}'", msvcup_pkg, install_version);

    // Generate vcvars bat files
    fs::create_dir_all(&install_path)?;
    for arch in Arch::ALL {
        let bat = generate_vcvars_bat(finish_kind, &install_version, arch);
        let basename = format!("vcvars-{}.bat", arch);
        let bat_path = install_path.join(&basename);
        update_file(&bat_path, bat.as_bytes())?;
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum FinishKind {
    Msvc,
    Sdk,
}

fn query_install_version(finish_kind: FinishKind, install_path: &Path) -> Result<String> {
    let query_path = match finish_kind {
        FinishKind::Msvc => install_path.join("VC").join("Tools").join("MSVC"),
        FinishKind::Sdk => install_path.join("Windows Kits").join("10").join("Include"),
    };

    let mut version_entry: Option<String> = None;
    for entry in fs::read_dir(&query_path)
        .with_context(|| format!("reading directory '{}'", query_path.display()))?
    {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if crate::util::is_valid_version(&name) {
            if version_entry.is_some() {
                bail!(
                    "directory '{}' has multiple version entries",
                    query_path.display()
                );
            }
            version_entry = Some(name);
        }
    }
    version_entry.ok_or_else(|| {
        anyhow::anyhow!(
            "directory '{}' did not contain any version subdirectories",
            query_path.display()
        )
    })
}

fn generate_vcvars_bat(
    finish_kind: FinishKind,
    install_version: &str,
    target_arch: Arch,
) -> String {
    let native_arch = Arch::native().unwrap_or(Arch::X64);
    match finish_kind {
        FinishKind::Msvc => format!(
            "set \"INCLUDE=%~dp0VC\\Tools\\MSVC\\{v}\\include;%INCLUDE%\"\n\
             set \"PATH=%~dp0VC\\Tools\\MSVC\\{v}\\bin\\Host{host}\\{target};%PATH%\"\n\
             set \"LIB=%~dp0VC\\Tools\\MSVC\\{v}\\lib\\{target};%LIB%\"\n",
            v = install_version,
            host = native_arch,
            target = target_arch,
        ),
        FinishKind::Sdk => format!(
            "set \"INCLUDE=%~dp0Windows Kits\\10\\Include\\{v}\\ucrt;\
             %~dp0Windows Kits\\10\\Include\\{v}\\shared;\
             %~dp0Windows Kits\\10\\Include\\{v}\\um;\
             %~dp0Windows Kits\\10\\Include\\{v}\\winrt;\
             %~dp0Windows Kits\\10\\Include\\{v}\\cppwinrt;\
             %INCLUDE%\"\n\
             set \"PATH=%~dp0Windows Kits\\10\\bin\\{v}\\{host};%PATH%\"\n\
             set \"LIB=%~dp0Windows Kits\\10\\Lib\\{v}\\ucrt\\{target};\
             %~dp0Windows Kits\\10\\Lib\\{v}\\um\\{target};%LIB%\"\n",
            v = install_version,
            host = native_arch,
            target = target_arch,
        ),
    }
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

pub fn update_lock_file(
    _client: &reqwest::blocking::Client,
    msvcup_pkgs: &[MsvcupPackage],
    lock_file_path: &str,
    pkgs: &Packages,
    _cache_dir: &str,
) -> Result<()> {
    // Collect install payloads
    let mut install_payloads: Vec<(MsvcupPackage, usize)> = Vec::new(); // (target, payload_index)

    for (pkg_index, pkg) in pkgs.packages.iter().enumerate() {
        match pkg.language {
            crate::packages::Language::Neutral | crate::packages::Language::EnUs => {}
            crate::packages::Language::Other => continue,
        }

        // Check if this package should be installed
        if let Some(install_pkg) = get_install_pkg(&pkg.id) {
            match install_pkg {
                InstallPkgKind::Msvc(pkg_version) => {
                    for msvcup_pkg in msvcup_pkgs {
                        if msvcup_pkg.kind == MsvcupPackageKind::Msvc
                            && msvcup_pkg.version == pkg_version
                        {
                            let range = pkgs.payload_range_from_pkg_index(pkg_index);
                            for pi in range {
                                insert_sorted(
                                    &mut install_payloads,
                                    (msvcup_pkg.clone(), pi),
                                    |a, b| match MsvcupPackage::order(&a.0, &b.0) {
                                        Ordering::Equal => a.1.cmp(&b.1),
                                        other => other,
                                    },
                                );
                            }
                            break;
                        }
                    }
                }
                InstallPkgKind::Msbuild(pkg_version) => {
                    for msvcup_pkg in msvcup_pkgs {
                        if msvcup_pkg.kind == MsvcupPackageKind::Msbuild
                            && msvcup_pkg.version == pkg_version
                        {
                            let range = pkgs.payload_range_from_pkg_index(pkg_index);
                            for pi in range {
                                insert_sorted(
                                    &mut install_payloads,
                                    (msvcup_pkg.clone(), pi),
                                    |a, b| match MsvcupPackage::order(&a.0, &b.0) {
                                        Ordering::Equal => a.1.cmp(&b.1),
                                        other => other,
                                    },
                                );
                            }
                            break;
                        }
                    }
                }
                InstallPkgKind::Diasdk => {
                    for msvcup_pkg in msvcup_pkgs {
                        if msvcup_pkg.kind == MsvcupPackageKind::Diasdk
                            && msvcup_pkg.version == pkg.version
                        {
                            let range = pkgs.payload_range_from_pkg_index(pkg_index);
                            for pi in range {
                                insert_sorted(
                                    &mut install_payloads,
                                    (msvcup_pkg.clone(), pi),
                                    |a, b| match MsvcupPackage::order(&a.0, &b.0) {
                                        Ordering::Equal => a.1.cmp(&b.1),
                                        other => other,
                                    },
                                );
                            }
                            break;
                        }
                    }
                }
                InstallPkgKind::Ninja(pkg_version) => {
                    for msvcup_pkg in msvcup_pkgs {
                        if msvcup_pkg.kind == MsvcupPackageKind::Ninja
                            && msvcup_pkg.version == pkg_version
                        {
                            let range = pkgs.payload_range_from_pkg_index(pkg_index);
                            for pi in range {
                                insert_sorted(
                                    &mut install_payloads,
                                    (msvcup_pkg.clone(), pi),
                                    |a, b| match MsvcupPackage::order(&a.0, &b.0) {
                                        Ordering::Equal => a.1.cmp(&b.1),
                                        other => other,
                                    },
                                );
                            }
                            break;
                        }
                    }
                }
                InstallPkgKind::Cmake(pkg_version) => {
                    for msvcup_pkg in msvcup_pkgs {
                        if msvcup_pkg.kind == MsvcupPackageKind::Cmake
                            && msvcup_pkg.version == pkg_version
                        {
                            let range = pkgs.payload_range_from_pkg_index(pkg_index);
                            for pi in range {
                                insert_sorted(
                                    &mut install_payloads,
                                    (msvcup_pkg.clone(), pi),
                                    |a, b| match MsvcupPackage::order(&a.0, &b.0) {
                                        Ordering::Equal => a.1.cmp(&b.1),
                                        other => other,
                                    },
                                );
                            }
                            break;
                        }
                    }
                }
            }
        }

        // Check for SDK payloads
        let payload_range = pkgs.payload_range_from_pkg_index(pkg_index);
        for pi in payload_range {
            let payload = &pkgs.payloads[pi];
            if identify_payload(&payload.file_name) == PayloadId::Sdk {
                for msvcup_pkg in msvcup_pkgs {
                    if msvcup_pkg.kind == MsvcupPackageKind::Sdk
                        && msvcup_pkg.version == pkg.version
                    {
                        insert_sorted(&mut install_payloads, (msvcup_pkg.clone(), pi), |a, b| {
                            match MsvcupPackage::order(&a.0, &b.0) {
                                Ordering::Equal => a.1.cmp(&b.1),
                                other => other,
                            }
                        });
                        break;
                    }
                }
            }
        }
    }

    log::warn!("TODO: add the dependencies for all the packages we've added");

    // Write lock file
    log::info!("{} payloads:", install_payloads.len());
    if let Some(dir) = Path::new(lock_file_path).parent() {
        fs::create_dir_all(dir)?;
    }
    let lock_file = fs::File::create(lock_file_path)?;
    let mut bw = BufWriter::new(lock_file);

    for (target, payload_index) in &install_payloads {
        let payload = &pkgs.payloads[*payload_index];
        let url_kind = get_lock_file_url_kind(&payload.url_decoded)
            .ok_or_else(|| anyhow::anyhow!("unable to determine payload kind from url"))?;

        // TODO: handle cab files for MSI payloads
        write_payload(
            &mut bw,
            Some(target),
            url_kind,
            &payload.url_decoded,
            &payload.sha256,
            &payload.file_name,
        )?;
    }
    bw.flush()?;

    Ok(())
}
