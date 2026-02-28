use crate::arch::Arch;
use crate::lock_file::LockFile;
use crate::lockfile_parse::{
    CabEntry, LockFileJson, LockFilePackage, LockFilePayloadEntry, check_lock_file_pkgs,
    parse_lock_file,
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
use fs_err as fs;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use tokio::sync::Semaphore;

/// Max concurrent downloads
const MAX_CONCURRENT_DOWNLOADS: usize = 8;

/// Max concurrent extractions (CPU/IO-bound), based on available CPU cores.
fn max_concurrent_extractions() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

#[allow(clippy::too_many_arguments)]
pub async fn install_command(
    client: &reqwest::Client,
    msvcup_dir: &MsvcupDir,
    msvcup_pkgs: &[MsvcupPackage],
    lock_file_path: &str,
    manifest_update: ManifestUpdate,
    cache_dir: Option<&str>,
    target_arch: Arch,
    mp: &MultiProgress,
) -> Result<()> {
    if msvcup_pkgs.is_empty() {
        bail!("no packages were given to install, use 'list' to list the available packages");
    }

    let cache_dir = cache_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| msvcup_dir.path(&["cache"]));
    let cache_dir_str = cache_dir.to_str().unwrap();

    let try_no_update = match manifest_update {
        ManifestUpdate::Off | ManifestUpdate::Daily => true,
        ManifestUpdate::Always => false,
    };

    if try_no_update {
        if let Ok(content) = fs::read_to_string(lock_file_path) {
            log::debug!("lock file found: '{}'", lock_file_path);
            if let Some(mismatch) = check_lock_file_pkgs(lock_file_path, &content, msvcup_pkgs) {
                log::debug!("{}", mismatch);
            } else {
                install_from_lock_file(
                    client,
                    msvcup_pkgs,
                    msvcup_dir,
                    cache_dir_str,
                    lock_file_path,
                    &content,
                    mp,
                )
                .await?;
                return Ok(());
            }
        } else {
            log::debug!("lock file NOT found: '{}'", lock_file_path);
        }
    }

    // Read VS manifest and update lock file
    let (vsman_path, vsman_content) = crate::manifest::read_vs_manifest(
        client,
        msvcup_dir,
        crate::channel_kind::ChannelKind::Release,
        ManifestUpdate::Off,
    )
    .await?;

    let pkgs = get_packages(vsman_path.to_str().unwrap(), &vsman_content)?;

    update_lock_file(msvcup_pkgs, lock_file_path, &pkgs, target_arch)?;

    let lock_file_content = fs::read_to_string(lock_file_path)
        .with_context(|| format!("reading lock file '{}' after update", lock_file_path))?;

    if let Some(mismatch) = check_lock_file_pkgs(lock_file_path, &lock_file_content, msvcup_pkgs) {
        bail!(
            "lock file '{}' still doesn't match after update: {}",
            lock_file_path,
            mismatch
        );
    }

    install_from_lock_file(
        client,
        msvcup_pkgs,
        msvcup_dir,
        cache_dir_str,
        lock_file_path,
        &lock_file_content,
        mp,
    )
    .await
}

async fn install_from_lock_file(
    client: &reqwest::Client,
    msvcup_pkgs: &[MsvcupPackage],
    msvcup_dir: &MsvcupDir,
    cache_dir: &str,
    lock_file_path: &str,
    lock_file_content: &str,
    mp: &MultiProgress,
) -> Result<()> {
    let lock_file = parse_lock_file(lock_file_path, lock_file_content)?;

    // --- Build cab info lookup from lock file ---
    let cab_info: HashMap<String, (String, Sha256)> = {
        let mut m = HashMap::new();
        for (cab_filename, cab_entry) in &lock_file.cabs {
            let sha256 = Sha256::parse_hex(&cab_entry.sha256).ok_or_else(|| {
                anyhow::anyhow!(
                    "invalid sha256 for cab '{}': '{}'",
                    cab_filename,
                    cab_entry.sha256
                )
            })?;
            m.insert(cab_filename.clone(), (cab_entry.url.clone(), sha256));
        }
        m
    };
    let cab_info = std::sync::Arc::new(cab_info);

    // --- Collect install entries (payloads to download and extract) ---
    let mut install_entries: Vec<(MsvcupPackage, String, Sha256)> = Vec::new();
    for lock_pkg in &lock_file.packages {
        let msvcup_pkg = MsvcupPackage::from_string(&lock_pkg.name)
            .map_err(|e| anyhow::anyhow!("invalid package name '{}': {}", lock_pkg.name, e))?;

        for entry in &lock_pkg.payloads {
            let sha256 = Sha256::parse_hex(&entry.sha256).ok_or_else(|| {
                anyhow::anyhow!(
                    "invalid sha256 for payload '{}': '{}'",
                    entry.url,
                    entry.sha256
                )
            })?;

            // Skip payloads for non-native architectures
            if let Some(arch) = crate::lockfile_parse::host_arch_limit(msvcup_pkg.kind, &entry.url)
                && Arch::native() != Some(arch)
            {
                continue;
            }

            install_entries.push((msvcup_pkg.clone(), entry.url.clone(), sha256));
        }
    }

    // --- Pipelined install: download → (read MSI → fetch CABs) → extract ---
    // Each payload is processed as a single async task. VSIX/ZIP payloads are
    // downloaded and immediately extracted. MSI payloads are downloaded, their
    // Media table is read to discover needed CABs, those CABs are fetched,
    // and then the MSI is extracted.

    let install_start = std::time::Instant::now();
    log::debug!("{} payloads to install", install_entries.len());

    let total = install_entries.len() as u64;
    let pb = mp.add(ProgressBar::new(total));
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{prefix} [{bar:30.cyan/blue}] {pos}/{len} {msg}")
            .expect("valid template")
            .progress_chars("=> "),
    );
    pb.set_prefix("Installing");
    pb.set_message("");

    let download_sem = std::sync::Arc::new(Semaphore::new(MAX_CONCURRENT_DOWNLOADS));
    let extract_sem = std::sync::Arc::new(Semaphore::new(max_concurrent_extractions()));
    let mut handles = Vec::new();

    for (msvcup_pkg, url, sha256) in install_entries {
        let client = client.clone();
        let mp = mp.clone();
        let pb = pb.clone();
        let download_sem = download_sem.clone();
        let extract_sem = extract_sem.clone();
        let cab_info = cab_info.clone();
        let install_path = msvcup_dir.path(&[&msvcup_pkg.pool_string()]);
        let cache_dir = cache_dir.to_string();
        let strip_root_dir = crate::lockfile_parse::strip_root_dir(msvcup_pkg.kind);
        let payload_name = basename_from_url(&url).to_string();

        handles.push(tokio::spawn(async move {
            let t_start = std::time::Instant::now();
            let name = basename_from_url(&url);
            let cache_path = cache_entry_path(&cache_dir, &sha256, name);

            // Step 1: Download the payload
            {
                let _permit = download_sem.acquire().await.unwrap();
                fetch_payload_async(&client, &sha256, &url, &cache_path, &mp).await?;
            }
            let t_download = t_start.elapsed();
            log::debug!("{}: downloaded in {:.1?}", payload_name, t_download);

            // Step 2: For MSIs, discover needed CABs and fetch them
            if get_lock_file_url_kind(&url) == Some(LockFileUrlKind::Msi) {
                let msi_cache_path = cache_path.clone();
                let cab_names = tokio::task::spawn_blocking(move || {
                    crate::msi_extract::read_msi_cab_names(&msi_cache_path)
                })
                .await
                .unwrap()
                .with_context(|| format!("reading cab names from '{}'", payload_name))?;

                let needed: Vec<_> = cab_names
                    .iter()
                    .filter(|c| !c.starts_with('#'))
                    .filter_map(|c| cab_info.get(c.as_str()))
                    .collect();
                log::debug!(
                    "{}: {} CABs needed (of {} in Media table)",
                    payload_name,
                    needed.len(),
                    cab_names.len()
                );

                let mut cab_handles = Vec::new();
                for (cab_url, cab_sha256) in needed {
                    let client = client.clone();
                    let mp = mp.clone();
                    let download_sem = download_sem.clone();
                    let cab_url = cab_url.clone();
                    let cab_sha256 = *cab_sha256;
                    let cache_dir = cache_dir.clone();
                    cab_handles.push(tokio::spawn(async move {
                        let _permit = download_sem.acquire().await.unwrap();
                        let cab_cache_name = basename_from_url(&cab_url);
                        let cab_cache_path =
                            cache_entry_path(&cache_dir, &cab_sha256, cab_cache_name);
                        fetch_payload_async(&client, &cab_sha256, &cab_url, &cab_cache_path, &mp)
                            .await
                    }));
                }
                for h in cab_handles {
                    h.await.unwrap()?;
                }
                log::debug!(
                    "{}: CABs fetched in {:.1?}",
                    payload_name,
                    t_start.elapsed() - t_download
                );
            }

            // Step 3: Extract
            let t_before_extract = std::time::Instant::now();
            {
                let _permit = extract_sem.acquire().await.unwrap();
                let t_extract_start = std::time::Instant::now();
                tokio::task::spawn_blocking(move || {
                    install_payload(
                        &install_path,
                        &cache_dir,
                        &url,
                        &sha256,
                        strip_root_dir,
                        &cab_info,
                    )
                })
                .await
                .unwrap()
                .with_context(|| format!("installing payload '{}'", payload_name))?;
                log::debug!(
                    "{}: extracted in {:.1?} (waited {:.1?} for slot)",
                    payload_name,
                    t_extract_start.elapsed(),
                    t_before_extract.elapsed() - t_extract_start.elapsed()
                );
            }

            log::debug!("{}: total {:.1?}", payload_name, t_start.elapsed());
            pb.inc(1);
            Ok::<(), anyhow::Error>(())
        }));
    }

    for handle in handles {
        handle.await.unwrap()?;
    }
    pb.finish_and_clear();
    log::debug!("install completed in {:.1?}", install_start.elapsed());

    // Finish packages (generate vcvars bat files and env JSON)
    for msvcup_pkg in msvcup_pkgs {
        finish_package(msvcup_dir, msvcup_pkg)?;
    }

    Ok(())
}

async fn fetch_payload_async(
    client: &reqwest::Client,
    sha256: &Sha256,
    url_decoded: &str,
    cache_path: &Path,
    mp: &MultiProgress,
) -> Result<()> {
    let cache_lock_path = format!("{}.lock", cache_path.display());
    let _cache_lock = LockFile::lock(&cache_lock_path)?;

    if cache_path.exists() {
        log::debug!("ALREADY FETCHED  | {} {}", url_decoded, sha256);
    } else {
        log::debug!("FETCHING         | {} {}", url_decoded, sha256);
        let fetch_path = PathBuf::from(format!("{}.fetching", cache_path.display()));
        let actual_sha256 = fetch(client, url_decoded, &fetch_path, Some(mp)).await?;
        if actual_sha256 != *sha256 {
            bail!(
                "SHA256 mismatch for '{}':\nexpected: {}\nactual  : {}",
                url_decoded,
                sha256,
                actual_sha256
            );
        }
        fs::rename(&fetch_path, cache_path)?;
    }
    Ok(())
}

fn cache_entry_path(cache_dir: &str, sha256: &Sha256, name: &str) -> PathBuf {
    let basename = format!("{}-{}", sha256, name);
    PathBuf::from(cache_dir).join(basename)
}

fn install_payload(
    install_dir_path: &Path,
    cache_dir: &str,
    url_decoded: &str,
    sha256: &Sha256,
    strip_root_dir: bool,
    cab_info: &HashMap<String, (String, Sha256)>,
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
    let install_meta_dir = install_dir_path.join("install");
    let installed_manifest_path = install_meta_dir.join(&installed_basename);

    if installed_manifest_path.exists() {
        log::debug!(
            "ALREADY INSTALLED | {} {}",
            basename_from_url(url_decoded),
            sha256
        );
        return Ok(());
    }

    fs::create_dir_all(install_dir_path)?;
    fs::create_dir_all(&install_meta_dir)?;

    // Use a per-payload temp manifest file to avoid races with the shared "current" file.
    // Each payload writes to its own unique temp file based on the hash.
    let pending_path = install_meta_dir.join(format!("{}.pending", installed_basename));

    // Clean up any leftover pending file from a previous interrupted install
    clean_up_pending(&pending_path)?;

    // Write install manifest
    let mut manifest_file = fs::File::create(&pending_path)?;
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
            install_msi(
                &cache_path,
                install_dir_path,
                cache_dir,
                cab_info,
                &mut manifest_file,
            )?;
        }
        LockFileUrlKind::Cab => unreachable!(),
    }

    drop(manifest_file);
    finalize_manifest(&installed_manifest_path, &pending_path)?;

    Ok(())
}

/// Clean up a pending manifest from a previous interrupted install.
/// Removes any files that were newly created by the interrupted payload.
fn clean_up_pending(pending_path: &Path) -> Result<()> {
    if let Ok(content) = fs::read_to_string(pending_path) {
        log::debug!(
            "found interrupted install manifest '{}', cleaning up...",
            pending_path.display()
        );
        let mut lines = content.lines();
        let _cache_basename = lines.next(); // skip first line (cache basename)
        for line in lines {
            if line.is_empty() {
                continue;
            }
            if let Some(sub_path) = line.strip_prefix("new ") {
                log::debug!("removing file '{}'", sub_path);
                let _ = fs::remove_file(sub_path);
            }
            // "add " lines: don't remove, file was added by another payload
        }
        let _ = fs::remove_file(pending_path);
    }
    Ok(())
}

/// Finalize installation by converting the pending manifest into the installed manifest.
/// Strips the cache basename header and the "new "/"add " prefixes, writing just the file paths.
fn finalize_manifest(installed_manifest_path: &Path, pending_path: &Path) -> Result<()> {
    let content = fs::read_to_string(pending_path).with_context(|| {
        format!(
            "reading pending install manifest '{}'",
            pending_path.display()
        )
    })?;

    let tmp_path = PathBuf::from(format!("{}.tmp", installed_manifest_path.display()));
    {
        let mut out = BufWriter::new(
            fs::File::create(&tmp_path)
                .with_context(|| format!("creating tmp manifest '{}'", tmp_path.display()))?,
        );
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

    fs::remove_file(pending_path)
        .with_context(|| format!("removing pending manifest '{}'", pending_path.display()))?;
    fs::rename(&tmp_path, installed_manifest_path).with_context(|| {
        format!(
            "renaming '{}' to '{}'",
            tmp_path.display(),
            installed_manifest_path.display()
        )
    })?;

    Ok(())
}

fn install_msi(
    msi_path: &Path,
    install_dir_path: &Path,
    cache_dir: &str,
    cab_info: &HashMap<String, (String, Sha256)>,
    manifest_file: &mut fs::File,
) -> Result<()> {
    let msi_name = msi_path.file_name().unwrap_or_default().to_string_lossy();
    log::debug!(
        "installing MSI '{}' from '{}'",
        msi_name,
        msi_path.display()
    );

    if !msi_path.exists() {
        bail!("MSI file not found at '{}'", msi_path.display());
    }

    // Read the MSI's Media table to find which external cabs it needs
    let cab_names = crate::msi_extract::read_msi_cab_names(msi_path)
        .with_context(|| format!("reading cab names from MSI '{}'", msi_path.display()))?;
    log::debug!(
        "  Media table has {} cab entries: {:?}",
        cab_names.len(),
        cab_names
    );

    // Stage only the needed CAB files (already pre-fetched in Pass 2)
    // Use a unique staging dir per MSI to avoid races when multiple MSIs
    // for the same package are extracted in parallel.
    let msi_stem = msi_path.file_stem().unwrap_or_default().to_string_lossy();
    let staging_dir = install_dir_path.join(format!(".msi-staging-{}", msi_stem));
    let _ = fs::remove_dir_all(&staging_dir);
    fs::create_dir_all(&staging_dir)?;

    let mut staged_count = 0u32;
    for cab_name in &cab_names {
        if cab_name.starts_with('#') {
            log::debug!(
                "  cab '{}': embedded (will be extracted from MSI stream)",
                cab_name
            );
            continue;
        }
        if let Some((url, sha256)) = cab_info.get(cab_name.as_str()) {
            let name = basename_from_url(url);
            let cab_cache_path = cache_entry_path(cache_dir, sha256, name);
            if !cab_cache_path.exists() {
                bail!(
                    "CAB '{}' not found in cache at '{}' (should have been pre-fetched)",
                    cab_name,
                    cab_cache_path.display()
                );
            }
            let dest = staging_dir.join(cab_name);
            if fs::hard_link(&cab_cache_path, &dest).is_err() {
                fs::copy(&cab_cache_path, &dest)?;
            }
            staged_count += 1;
            log::debug!("  cab '{}': staged from lock file", cab_name);
        } else {
            log::warn!(
                "  cab '{}': NOT in lock file ({} cabs available)",
                cab_name,
                cab_info.len()
            );
        }
    }
    log::debug!(
        "  staged {} external cab(s) for '{}'",
        staged_count,
        msi_name
    );

    crate::msi_extract::extract_msi(msi_path, install_dir_path, &staging_dir, manifest_file)
        .with_context(|| format!("extracting MSI '{}'", msi_name))?;

    let _ = fs::remove_dir_all(&staging_dir);
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
    log::debug!("{} install version '{}'", msvcup_pkg, install_version);

    // Generate vcvars bat files and env JSON files
    fs::create_dir_all(&install_path)?;
    for arch in Arch::ALL {
        let bat = generate_vcvars_bat(finish_kind, &install_version, arch);
        let basename = format!("vcvars-{}.bat", arch);
        let bat_path = install_path.join(&basename);
        crate::util::update_file(&bat_path, bat.as_bytes())?;

        let env_json = generate_env_json(finish_kind, &install_version, arch, &install_path);
        let json_basename = format!("env-{}.json", arch);
        let json_path = install_path.join(&json_basename);
        crate::util::update_file(&json_path, env_json.as_bytes())?;
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

/// Generate a JSON file with resolved environment variable entries for a given arch.
/// The JSON maps env var names to arrays of absolute path entries to prepend.
fn generate_env_json(
    finish_kind: FinishKind,
    install_version: &str,
    target_arch: Arch,
    install_path: &Path,
) -> String {
    let native_arch = Arch::native().unwrap_or(Arch::X64);
    let root = install_path.to_string_lossy();

    let mut env: HashMap<String, Vec<String>> = HashMap::new();

    match finish_kind {
        FinishKind::Msvc => {
            env.insert(
                "INCLUDE".to_string(),
                vec![format!(
                    "{}\\VC\\Tools\\MSVC\\{}\\include",
                    root, install_version
                )],
            );
            env.insert(
                "PATH".to_string(),
                vec![format!(
                    "{}\\VC\\Tools\\MSVC\\{}\\bin\\Host{}\\{}",
                    root, install_version, native_arch, target_arch
                )],
            );
            env.insert(
                "LIB".to_string(),
                vec![format!(
                    "{}\\VC\\Tools\\MSVC\\{}\\lib\\{}",
                    root, install_version, target_arch
                )],
            );
        }
        FinishKind::Sdk => {
            env.insert(
                "INCLUDE".to_string(),
                vec![
                    format!(
                        "{}\\Windows Kits\\10\\Include\\{}\\ucrt",
                        root, install_version
                    ),
                    format!(
                        "{}\\Windows Kits\\10\\Include\\{}\\shared",
                        root, install_version
                    ),
                    format!(
                        "{}\\Windows Kits\\10\\Include\\{}\\um",
                        root, install_version
                    ),
                    format!(
                        "{}\\Windows Kits\\10\\Include\\{}\\winrt",
                        root, install_version
                    ),
                    format!(
                        "{}\\Windows Kits\\10\\Include\\{}\\cppwinrt",
                        root, install_version
                    ),
                ],
            );
            env.insert(
                "PATH".to_string(),
                vec![format!(
                    "{}\\Windows Kits\\10\\bin\\{}\\{}",
                    root, install_version, native_arch
                )],
            );
            env.insert(
                "LIB".to_string(),
                vec![
                    format!(
                        "{}\\Windows Kits\\10\\Lib\\{}\\ucrt\\{}",
                        root, install_version, target_arch
                    ),
                    format!(
                        "{}\\Windows Kits\\10\\Lib\\{}\\um\\{}",
                        root, install_version, target_arch
                    ),
                ],
            );
        }
    }

    serde_json::to_string_pretty(&env).unwrap()
}

pub fn update_lock_file(
    msvcup_pkgs: &[MsvcupPackage],
    lock_file_path: &str,
    pkgs: &Packages,
    target_arch: Arch,
) -> Result<()> {
    let host_arch = Arch::native().unwrap_or(Arch::X64);
    // Collect install payloads
    let mut install_payloads: Vec<(MsvcupPackage, usize)> = Vec::new(); // (target, payload_index)

    for (pkg_index, pkg) in pkgs.packages.iter().enumerate() {
        match pkg.language {
            crate::packages::Language::Neutral | crate::packages::Language::EnUs => {}
            crate::packages::Language::Other => continue,
        }

        // Check if this package should be installed
        if let Some(install_pkg) = get_install_pkg(&pkg.id, host_arch, target_arch) {
            let (target_kind, target_version) = match &install_pkg {
                InstallPkgKind::Msvc(v) => (MsvcupPackageKind::Msvc, v.as_str()),
                InstallPkgKind::Msbuild(v) => (MsvcupPackageKind::Msbuild, v.as_str()),
                InstallPkgKind::Diasdk => (MsvcupPackageKind::Diasdk, pkg.version.as_str()),
                InstallPkgKind::Ninja(v) => (MsvcupPackageKind::Ninja, v.as_str()),
                InstallPkgKind::Cmake(v) => (MsvcupPackageKind::Cmake, v.as_str()),
            };

            if let Some(msvcup_pkg) = msvcup_pkgs
                .iter()
                .find(|p| p.kind == target_kind && p.version == target_version)
            {
                let range = pkgs.payload_range_from_pkg_index(pkg_index);
                for pi in range {
                    insert_sorted(&mut install_payloads, (msvcup_pkg.clone(), pi), |a, b| {
                        match MsvcupPackage::order(&a.0, &b.0) {
                            Ordering::Equal => a.1.cmp(&b.1),
                            other => other,
                        }
                    });
                }
            }
        }

        // Check for SDK payloads
        let payload_range = pkgs.payload_range_from_pkg_index(pkg_index);
        for pi in payload_range {
            let payload = &pkgs.payloads[pi];
            if identify_payload(&payload.file_name, target_arch) == PayloadId::Sdk {
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

    // Verify every requested package has at least one payload
    for msvcup_pkg in msvcup_pkgs {
        let has_payload = install_payloads.iter().any(|(pkg, _)| pkg == msvcup_pkg);
        if !has_payload {
            bail!(
                "package '{}' not found in the VS manifest. \
                 Run 'msvcup list' to see available versions.",
                msvcup_pkg
            );
        }
    }

    // Collect unique cab payloads for MSI payloads from the VS manifest.
    // Each VS manifest package lists MSIs and CABs as sibling payloads.
    let mut cabs: HashMap<String, CabEntry> = HashMap::new();
    let mut seen_pkg_indices: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for (_, payload_index) in &install_payloads {
        let payload = &pkgs.payloads[*payload_index];
        if get_lock_file_url_kind(&payload.url_decoded) != Some(LockFileUrlKind::Msi) {
            continue;
        }
        let pkg_index = pkgs.pkg_index_from_payload_index(*payload_index);
        if !seen_pkg_indices.insert(pkg_index) {
            continue;
        }
        let pkg_payload_range = pkgs.payload_range_from_pkg_index(pkg_index);
        for pi in pkg_payload_range {
            let sibling = &pkgs.payloads[pi];
            if sibling.file_name.ends_with(".cab") {
                let cab_filename = sibling
                    .file_name
                    .rfind('\\')
                    .map(|i| &sibling.file_name[i + 1..])
                    .unwrap_or(&sibling.file_name);
                cabs.entry(cab_filename.to_string())
                    .or_insert_with(|| CabEntry {
                        url: sibling.url_decoded.clone(),
                        sha256: sibling.sha256.to_hex(),
                    });
            }
        }
    }

    // Build JSON packages list
    let mut json_packages: Vec<LockFilePackage> = Vec::new();
    let mut current_pkg_name: Option<String> = None;
    let mut current_payloads: Vec<LockFilePayloadEntry> = Vec::new();

    for (target, payload_index) in &install_payloads {
        let payload = &pkgs.payloads[*payload_index];
        let pkg_name = target.pool_string();

        if current_pkg_name.as_deref() != Some(&pkg_name) {
            if let Some(name) = current_pkg_name.take() {
                json_packages.push(LockFilePackage {
                    name,
                    payloads: std::mem::take(&mut current_payloads),
                });
            }
            current_pkg_name = Some(pkg_name);
        }

        current_payloads.push(LockFilePayloadEntry {
            url: payload.url_decoded.clone(),
            sha256: payload.sha256.to_hex(),
        });
    }
    if let Some(name) = current_pkg_name {
        json_packages.push(LockFilePackage {
            name,
            payloads: current_payloads,
        });
    }

    let lock_file_json = LockFileJson {
        cabs,
        packages: json_packages,
    };

    log::debug!("{} payloads:", install_payloads.len());
    if let Some(dir) = Path::new(lock_file_path).parent() {
        fs::create_dir_all(dir)?;
    }
    let json_str = serde_json::to_string_pretty(&lock_file_json)?;
    fs::write(lock_file_path, json_str)?;

    Ok(())
}
