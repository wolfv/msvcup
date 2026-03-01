//! ZIP and VSIX archive extraction.
//!
//! Extracts ZIP files (including VSIX packages, which are ZIP files with a
//! specific directory structure). Handles root directory stripping, VSIX prefix
//! removal, percent-encoded filenames, and path traversal prevention.

use anyhow::{Context, Result};
use fs_err as fs;
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
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("reading ZIP '{}'", cache_path.display()))?;

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
                anyhow::bail!(
                    "ZIP filename contains '.' or '..' component: '{}'",
                    filename
                );
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
            if let Some(ref last) = last_root_dir
                && last != root_dir
            {
                anyhow::bail!(
                    "root dir changed from '{}' to '{}', cannot strip",
                    last,
                    root_dir
                );
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;

    /// Create a ZIP file in a unique temp directory with the given entries.
    fn create_test_zip(test_name: &str, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("msvcup_test_zipsrc_{}", test_name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("test.zip");
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip_writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            zip_writer.start_file(*name, options).unwrap();
            zip_writer.write_all(data).unwrap();
        }
        zip_writer.finish().unwrap();
        zip_path
    }

    #[test]
    fn extract_plain_zip() {
        let zip_path = create_test_zip(
            "plain",
            &[("file1.txt", b"hello"), ("subdir/file2.txt", b"world")],
        );

        let install_dir = std::env::temp_dir().join("msvcup_test_zip_plain");
        let _ = std::fs::remove_dir_all(&install_dir);
        std::fs::create_dir_all(&install_dir).unwrap();

        let manifest_path = install_dir.join("manifest.txt");
        let mut manifest_file = fs::File::create(&manifest_path).unwrap();

        extract_zip_to_dir(
            &zip_path,
            &install_dir,
            ZipKind::Zip,
            false,
            &mut manifest_file,
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(install_dir.join("file1.txt")).unwrap(),
            "hello"
        );
        assert_eq!(
            std::fs::read_to_string(install_dir.join("subdir").join("file2.txt")).unwrap(),
            "world"
        );

        let _ = std::fs::remove_dir_all(&install_dir);
        let _ = std::fs::remove_dir_all(zip_path.parent().unwrap());
    }

    #[test]
    fn extract_vsix_strips_contents_prefix() {
        let zip_path = create_test_zip(
            "vsix",
            &[
                ("Contents/lib/mylib.dll", b"dll data"),
                ("[Content_Types].xml", b"xml"),
            ],
        );

        let install_dir = std::env::temp_dir().join("msvcup_test_zip_vsix");
        let _ = std::fs::remove_dir_all(&install_dir);
        std::fs::create_dir_all(&install_dir).unwrap();

        let manifest_path = install_dir.join("manifest.txt");
        let mut manifest_file = fs::File::create(&manifest_path).unwrap();

        extract_zip_to_dir(
            &zip_path,
            &install_dir,
            ZipKind::Vsix,
            false,
            &mut manifest_file,
        )
        .unwrap();

        // "Contents/" prefix is stripped for VSIX
        assert_eq!(
            std::fs::read_to_string(install_dir.join("lib").join("mylib.dll")).unwrap(),
            "dll data"
        );
        // Non-Contents files are skipped
        assert!(!install_dir.join("[Content_Types].xml").exists());

        let _ = std::fs::remove_dir_all(&install_dir);
        let _ = std::fs::remove_dir_all(zip_path.parent().unwrap());
    }

    #[test]
    fn extract_zip_with_strip_root_dir() {
        let zip_path = create_test_zip(
            "strip_root",
            &[
                ("cmake-3.31.4/bin/cmake.exe", b"cmake binary"),
                ("cmake-3.31.4/share/info.txt", b"info"),
            ],
        );

        let install_dir = std::env::temp_dir().join("msvcup_test_zip_strip");
        let _ = std::fs::remove_dir_all(&install_dir);
        std::fs::create_dir_all(&install_dir).unwrap();

        let manifest_path = install_dir.join("manifest.txt");
        let mut manifest_file = fs::File::create(&manifest_path).unwrap();

        extract_zip_to_dir(
            &zip_path,
            &install_dir,
            ZipKind::Zip,
            true, // strip root
            &mut manifest_file,
        )
        .unwrap();

        // Root "cmake-3.31.4/" should be stripped
        assert_eq!(
            std::fs::read_to_string(install_dir.join("bin").join("cmake.exe")).unwrap(),
            "cmake binary"
        );
        assert_eq!(
            std::fs::read_to_string(install_dir.join("share").join("info.txt")).unwrap(),
            "info"
        );

        let _ = std::fs::remove_dir_all(&install_dir);
        let _ = std::fs::remove_dir_all(zip_path.parent().unwrap());
    }

    #[test]
    fn extract_zip_rejects_dotdot() {
        let zip_path = create_test_zip("dotdot", &[("safe/../../escape.txt", b"bad")]);

        let install_dir = std::env::temp_dir().join("msvcup_test_zip_dotdot");
        let _ = std::fs::remove_dir_all(&install_dir);
        std::fs::create_dir_all(&install_dir).unwrap();

        let manifest_path = install_dir.join("manifest.txt");
        let mut manifest_file = fs::File::create(&manifest_path).unwrap();

        let result = extract_zip_to_dir(
            &zip_path,
            &install_dir,
            ZipKind::Zip,
            false,
            &mut manifest_file,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains(".."));

        let _ = std::fs::remove_dir_all(&install_dir);
        let _ = std::fs::remove_dir_all(zip_path.parent().unwrap());
    }

    #[test]
    fn extract_zip_manifest_tracking() {
        let zip_path = create_test_zip("manifest_track", &[("new_file.txt", b"data")]);

        let install_dir = std::env::temp_dir().join("msvcup_test_zip_manifest");
        let _ = std::fs::remove_dir_all(&install_dir);
        std::fs::create_dir_all(&install_dir).unwrap();

        let manifest_path = install_dir.join("manifest.txt");
        let mut manifest_file = fs::File::create(&manifest_path).unwrap();

        extract_zip_to_dir(
            &zip_path,
            &install_dir,
            ZipKind::Zip,
            false,
            &mut manifest_file,
        )
        .unwrap();

        drop(manifest_file);
        let manifest = std::fs::read_to_string(&manifest_path).unwrap();
        assert!(manifest.contains("new"));
        assert!(manifest.contains("new_file.txt"));

        // Extract again - file now exists, should be "add"
        let mut manifest_file2 = fs::File::create(&manifest_path).unwrap();
        extract_zip_to_dir(
            &zip_path,
            &install_dir,
            ZipKind::Zip,
            false,
            &mut manifest_file2,
        )
        .unwrap();

        drop(manifest_file2);
        let manifest2 = std::fs::read_to_string(&manifest_path).unwrap();
        assert!(manifest2.contains("add"));

        let _ = std::fs::remove_dir_all(&install_dir);
        let _ = std::fs::remove_dir_all(zip_path.parent().unwrap());
    }

    #[test]
    fn extract_zip_percent_encoded_filenames() {
        let zip_path = create_test_zip("pct_encode", &[("dir/file%20name.txt", b"encoded")]);

        let install_dir = std::env::temp_dir().join("msvcup_test_zip_pct");
        let _ = std::fs::remove_dir_all(&install_dir);
        std::fs::create_dir_all(&install_dir).unwrap();

        let manifest_path = install_dir.join("manifest.txt");
        let mut manifest_file = fs::File::create(&manifest_path).unwrap();

        extract_zip_to_dir(
            &zip_path,
            &install_dir,
            ZipKind::Zip,
            false,
            &mut manifest_file,
        )
        .unwrap();

        // Percent-encoded name should be decoded
        assert!(install_dir.join("dir").join("file name.txt").exists());

        let _ = std::fs::remove_dir_all(&install_dir);
        let _ = std::fs::remove_dir_all(zip_path.parent().unwrap());
    }

    #[test]
    fn extract_zip_strip_root_dir_inconsistent_roots_fails() {
        let zip_path = create_test_zip(
            "strip_incon",
            &[("root1/file1.txt", b"a"), ("root2/file2.txt", b"b")],
        );

        let install_dir = std::env::temp_dir().join("msvcup_test_zip_strip_fail");
        let _ = std::fs::remove_dir_all(&install_dir);
        std::fs::create_dir_all(&install_dir).unwrap();

        let manifest_path = install_dir.join("manifest.txt");
        let mut manifest_file = fs::File::create(&manifest_path).unwrap();

        let result = extract_zip_to_dir(
            &zip_path,
            &install_dir,
            ZipKind::Zip,
            true,
            &mut manifest_file,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("root dir changed"));

        let _ = std::fs::remove_dir_all(&install_dir);
        let _ = std::fs::remove_dir_all(zip_path.parent().unwrap());
    }
}
