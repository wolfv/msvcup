use crate::channel_kind::ChannelKind;
use crate::lock_file::LockFile;
use crate::packages::ManifestUpdate;
use crate::sha::{Sha256, Sha256Streaming};
use anyhow::{bail, Context, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The msvcup data directory
pub struct MsvcupDir {
    pub root_path: PathBuf,
}

impl MsvcupDir {
    pub fn new() -> Result<Self> {
        let root_path = if cfg!(windows) {
            PathBuf::from("C:\\msvcup")
        } else {
            dirs::data_dir()
                .ok_or_else(|| anyhow::anyhow!("unable to determine app data directory"))?
                .join("msvcup")
        };
        Ok(Self { root_path })
    }

    pub fn path(&self, parts: &[&str]) -> PathBuf {
        let mut p = self.root_path.clone();
        for part in parts {
            p.push(part);
        }
        p
    }
}

/// Read a file, returning None if it doesn't exist
fn read_file_opt(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading '{}'", path.display())),
    }
}

/// Fetch a URL to a file, returning the SHA256 hash
pub fn fetch(
    client: &reqwest::blocking::Client,
    url: &str,
    out_path: &Path,
) -> Result<Sha256> {
    log::info!("fetch: {}", url);

    let response = client
        .get(url)
        .send()
        .with_context(|| format!("fetching '{}'", url))?;

    if !response.status().is_success() {
        bail!(
            "fetch '{}': HTTP status {}",
            url,
            response.status()
        );
    }

    if let Some(dir) = out_path.parent() {
        fs::create_dir_all(dir)
            .with_context(|| format!("creating directory '{}'", dir.display()))?;
    }

    let mut file =
        fs::File::create(out_path).with_context(|| format!("creating '{}'", out_path.display()))?;
    let mut hasher = Sha256Streaming::new();

    let bytes = response.bytes().with_context(|| format!("reading response from '{}'", url))?;
    hasher.update(&bytes);
    file.write_all(&bytes)
        .with_context(|| format!("writing to '{}'", out_path.display()))?;

    Ok(hasher.finalize())
}

/// Fetch a URL, following redirects only to capture the redirect URL
pub fn resolve_redirect(
    _client: &reqwest::blocking::Client,
    url: &str,
    out_path: &Path,
) -> Result<()> {
    log::info!("resolving URL '{}'...", url);

    // Use a client that doesn't follow redirects
    let no_redirect_client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let response = no_redirect_client
        .get(url)
        .send()
        .with_context(|| format!("resolving '{}'", url))?;

    if response.status().is_redirection() {
        if let Some(location) = response.headers().get("location") {
            let redirect_url = location.to_str().with_context(|| "invalid redirect URL")?;
            if let Some(dir) = out_path.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::write(out_path, redirect_url)
                .with_context(|| format!("writing redirect URL to '{}'", out_path.display()))?;
            return Ok(());
        }
        bail!("redirect response missing Location header");
    }

    bail!(
        "GET '{}' HTTP status {} (expected redirect)",
        url,
        response.status()
    );
}

/// Read the VS manifest, fetching if necessary
pub fn read_vs_manifest(
    client: &reqwest::blocking::Client,
    msvcup_dir: &MsvcupDir,
    channel_kind: ChannelKind,
    update: ManifestUpdate,
) -> Result<(PathBuf, String)> {
    let subdir = channel_kind.subdir();
    let vsman_latest_path = msvcup_dir.path(&["manifest", subdir, "latest"]);
    let vsman_lock_path = msvcup_dir.path(&["manifest", subdir, ".lock"]);

    // First check with lock
    {
        let _lock = LockFile::lock(vsman_lock_path.to_str().unwrap())?;
        match update {
            ManifestUpdate::Off => {
                if let Some(content) = read_file_opt(&vsman_latest_path)? {
                    return Ok((vsman_latest_path, content));
                }
            }
            ManifestUpdate::Daily => bail!("daily manifest update not yet implemented"),
            ManifestUpdate::Always => {}
        }
    }

    // Read channel manifest (releases lock to avoid deadlock)
    let (chman_path, chman_content) =
        read_ch_manifest(client, msvcup_dir, channel_kind, update)?;

    // Re-acquire lock and check again
    {
        let _lock = LockFile::lock(vsman_lock_path.to_str().unwrap())?;
        match update {
            ManifestUpdate::Off => {
                if let Some(content) = read_file_opt(&vsman_latest_path)? {
                    return Ok((vsman_latest_path, content));
                }
            }
            ManifestUpdate::Daily => bail!("daily manifest update not yet implemented"),
            ManifestUpdate::Always => {
                // TODO: check if updated
            }
        }

        // Parse channel manifest to find VS manifest URL
        let payload = vs_manifest_payload_from_ch_manifest(channel_kind, &chman_path, &chman_content)?;
        let _sha256 = fetch(client, &payload.url, &vsman_latest_path)?;
        let content = read_file_opt(&vsman_latest_path)?
            .ok_or_else(|| anyhow::anyhow!("{} still doesn't exist", vsman_latest_path.display()))?;
        Ok((vsman_latest_path, content))
    }
}

/// Read the channel manifest
fn read_ch_manifest(
    client: &reqwest::blocking::Client,
    msvcup_dir: &MsvcupDir,
    channel_kind: ChannelKind,
    update: ManifestUpdate,
) -> Result<(PathBuf, String)> {
    let subdir = channel_kind.channel_subdir();
    let chman_latest_path = msvcup_dir.path(&["manifest", subdir, "latest"]);
    let chman_lock_path = msvcup_dir.path(&["manifest", subdir, ".lock"]);

    {
        let _lock = LockFile::lock(chman_lock_path.to_str().unwrap())?;
        match update {
            ManifestUpdate::Off => {
                if let Some(content) = read_file_opt(&chman_latest_path)? {
                    return Ok((chman_latest_path, content));
                }
            }
            ManifestUpdate::Daily => bail!("daily manifest update not yet implemented"),
            ManifestUpdate::Always => {}
        }
    }

    // Resolve the channel manifest URL
    let (_url_path, url_content) =
        resolve_ch_manifest_url(client, msvcup_dir, channel_kind, update)?;

    {
        let _lock = LockFile::lock(chman_lock_path.to_str().unwrap())?;
        match update {
            ManifestUpdate::Off => {
                if let Some(content) = read_file_opt(&chman_latest_path)? {
                    return Ok((chman_latest_path, content));
                }
            }
            ManifestUpdate::Daily => bail!("daily manifest update not yet implemented"),
            ManifestUpdate::Always => {}
        }

        let _sha256 = fetch(client, &url_content, &chman_latest_path)?;
        let content = read_file_opt(&chman_latest_path)?
            .ok_or_else(|| anyhow::anyhow!("{} still doesn't exist", chman_latest_path.display()))?;
        Ok((chman_latest_path, content))
    }
}

/// Resolve the channel manifest URL (follows redirect from aka.ms)
fn resolve_ch_manifest_url(
    client: &reqwest::blocking::Client,
    msvcup_dir: &MsvcupDir,
    channel_kind: ChannelKind,
    update: ManifestUpdate,
) -> Result<(PathBuf, String)> {
    let subdir = channel_kind.channel_url_subdir();
    let url_path = msvcup_dir.path(&["manifest", subdir, "latest"]);
    let url_lock_path = msvcup_dir.path(&["manifest", subdir, ".lock"]);

    let _lock = LockFile::lock(url_lock_path.to_str().unwrap())?;
    match update {
        ManifestUpdate::Off => {
            if let Some(content) = read_file_opt(&url_path)? {
                return Ok((url_path, content));
            }
        }
        ManifestUpdate::Daily => bail!("daily manifest update not yet implemented"),
        ManifestUpdate::Always => {}
    }

    resolve_redirect(client, channel_kind.https_url(), &url_path)?;
    let content = read_file_opt(&url_path)?
        .ok_or_else(|| anyhow::anyhow!("{} still doesn't exist", url_path.display()))?;
    Ok((url_path, content))
}

struct VsManifestPayload {
    url: String,
    sha256: Sha256,
    size: u64,
}

fn vs_manifest_payload_from_ch_manifest(
    channel_kind: ChannelKind,
    chman_path: &Path,
    chman_content: &str,
) -> Result<VsManifestPayload> {
    let parsed: serde_json::Value = serde_json::from_str(chman_content)
        .with_context(|| format!("parsing '{}'", chman_path.display()))?;

    let channel_items = parsed
        .get("channelItems")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{}: missing 'channelItems' array",
                chman_path.display()
            )
        })?;

    let vs_manifest_id = channel_kind.vs_manifest_channel_id();

    for item in channel_items {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if id == vs_manifest_id {
            let payloads = item
                .get("payloads")
                .and_then(|v| v.as_array())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "{}: channelItem '{}' missing 'payloads'",
                        chman_path.display(),
                        id
                    )
                })?;
            if payloads.len() != 1 {
                bail!(
                    "{}: channelItem '{}' has {} payloads instead of 1",
                    chman_path.display(),
                    id,
                    payloads.len()
                );
            }
            let payload = &payloads[0];
            let url = payload
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    anyhow::anyhow!("{}: payload missing 'url'", chman_path.display())
                })?;
            let sha256_str = payload
                .get("sha256")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    anyhow::anyhow!("{}: payload missing 'sha256'", chman_path.display())
                })?;
            let sha256_hex = sha256_str.to_ascii_lowercase();
            let sha256 = Sha256::parse_hex(&sha256_hex).ok_or_else(|| {
                anyhow::anyhow!("{}: invalid sha256 '{}'", chman_path.display(), sha256_str)
            })?;
            let size = payload
                .get("size")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| {
                    anyhow::anyhow!("{}: payload missing 'size'", chman_path.display())
                })?;

            let decoded_url = crate::util::alloc_url_percent_decoded(url);
            return Ok(VsManifestPayload {
                url: decoded_url,
                sha256,
                size,
            });
        }
    }

    bail!(
        "channel manifest '{}' is missing vs manifest id '{}'",
        chman_path.display(),
        vs_manifest_id
    );
}
