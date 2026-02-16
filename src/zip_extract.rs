use anyhow::{Context, Result};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Extract a ZIP/VSIX file to an install directory, writing an install manifest
pub fn extract_zip_to_dir(
    cache_path: &Path,
    install_dir_path: &Path,
    kind: ZipKind,
    strip_root_dir: bool,
    installing_manifest: &mut fs::File,
) -> Result<()> {
    let file = fs::File::open(cache_path)
        .with_context(|| format!("opening '{}'", cache_path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).with_context(|| format!("reading ZIP '{}'", cache_path.display()))?;

    let prefix = match kind {
        ZipKind::Vsix => "Contents/",
        ZipKind::Zip => "",
    };

    let mut last_root_dir: Option<String> = None;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let raw_name = entry.name().to_string();

        // Normalize separators
        let filename = raw_name.replace('\\', "/");

        if filename.is_empty() || filename.starts_with('/') {
            continue;
        }

        // Check for . and .. components
        for part in filename.split('/') {
            if part == "." || part == ".." {
                anyhow::bail!("ZIP filename contains '.' or '..' component: '{}'", filename);
            }
        }

        // Skip entries not in the expected prefix
        if !filename.starts_with(prefix) {
            continue;
        }

        // Skip directories
        if filename.ends_with('/') {
            continue;
        }

        // Remove prefix, then URL percent-decode
        let sub_path_encoded = &filename[prefix.len()..];
        let sub_path_decoded =
            percent_encoding::percent_decode_str(sub_path_encoded).decode_utf8_lossy();
        let sub_path_decoded = sub_path_decoded.as_ref();

        // Strip root directory if requested
        let sub_path = if strip_root_dir {
            let sep_pos = sub_path_decoded.find('/').ok_or_else(|| {
                anyhow::anyhow!("no root dir to strip from '{}'", sub_path_decoded)
            })?;
            let root_dir = &sub_path_decoded[..sep_pos];
            if let Some(ref last) = last_root_dir {
                if last != root_dir {
                    anyhow::bail!(
                        "root dir changed from '{}' to '{}', cannot strip",
                        last,
                        root_dir
                    );
                }
            }
            last_root_dir = Some(root_dir.to_string());
            &sub_path_decoded[sep_pos..]
        } else {
            sub_path_decoded
        };

        let install_path = install_dir_path.join(
            sub_path
                .strip_prefix('/')
                .unwrap_or(sub_path)
                .replace('/', std::path::MAIN_SEPARATOR_STR),
        );

        // Check if file already exists
        if install_path.exists() {
            writeln!(installing_manifest, "add {}", install_path.display())?;
        } else {
            writeln!(installing_manifest, "new {}", install_path.display())?;
            if let Some(parent) = install_path.parent() {
                fs::create_dir_all(parent)?;
            }
        }

        let mut outfile = fs::File::create(&install_path)
            .with_context(|| format!("creating '{}'", install_path.display()))?;
        io::copy(&mut entry, &mut outfile)?;
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub enum ZipKind {
    Vsix,
    Zip,
}
