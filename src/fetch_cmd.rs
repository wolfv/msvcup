use crate::lock_file::LockFile;
use crate::manifest::{MsvcupDir, fetch};
use crate::sha::Sha256;
use crate::util::basename_from_url;
use anyhow::{Result, bail};
use std::fs;
use std::path::PathBuf;

pub async fn fetch_command(
    client: &reqwest::Client,
    url: &str,
    cache_dir: Option<&str>,
) -> Result<()> {
    // Validate URL
    let _uri = url::Url::parse(url).map_err(|e| anyhow::anyhow!("invalid URL '{}': {}", url, e))?;

    // Validate it's a known package URL
    match crate::extra::parse_url(url) {
        crate::extra::ParseUrlResult::Ok(_) => {}
        crate::extra::ParseUrlResult::Unexpected { offset, what } => {
            bail!(
                "invalid package url '{}' expected {} at offset {} but got '{}'",
                url,
                what,
                offset,
                &url[offset..]
            );
        }
    }

    let msvcup_dir = MsvcupDir::new()?;
    let cache_dir = cache_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| msvcup_dir.path(&["cache"]));
    let cache_dir_str = cache_dir.to_str().unwrap();

    let cache_path = PathBuf::from(cache_dir_str).join("nohash");
    let cache_lock_path = format!("{}.lock", cache_path.display());

    let _cache_lock = LockFile::lock(&cache_lock_path)?;

    let sha256 = fetch(client, url, &cache_path, None).await?;

    // Move to proper cache location
    finish_cache_fetch(cache_dir_str, url, &sha256, &cache_path)?;

    println!("{}", sha256);

    Ok(())
}

fn finish_cache_fetch(
    cache_dir: &str,
    url: &str,
    sha256: &Sha256,
    cache_path: &PathBuf,
) -> Result<()> {
    let name = basename_from_url(url);
    let cache_basename = format!("{}-{}", sha256, name);
    let final_path = PathBuf::from(cache_dir).join(&cache_basename);

    if final_path.exists() {
        log::info!("{}: already exists", final_path.display());
        fs::remove_file(cache_path)?;
    } else {
        log::info!("{}: newly fetched", final_path.display());
        fs::create_dir_all(cache_dir)?;
        fs::rename(cache_path, &final_path)?;
    }
    Ok(())
}
