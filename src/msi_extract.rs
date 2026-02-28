use anyhow::{Context, Result};
use fs_err as fs;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::Path;

/// Extract files from an MSI package to a target directory.
///
/// This reads the MSI database tables (File, Component, Directory, Media)
/// to determine file paths, then extracts files from CAB archives
/// (either embedded in the MSI or external) to their correct locations.
///
/// `cab_dir` is the directory containing external .cab files referenced by the MSI.
pub fn extract_msi(
    msi_path: &Path,
    install_dir: &Path,
    cab_dir: &Path,
    manifest_file: &mut fs::File,
) -> Result<()> {
    let msi_name = msi_path.file_name().unwrap_or_default().to_string_lossy();
    let mut package = msi::open(msi_path)
        .with_context(|| format!("opening MSI file '{}'", msi_path.display()))?;

    // Parse the Directory table: directory_id -> (parent_id, default_dir)
    let directory_table = read_directory_table(&mut package)?;

    // Parse the Component table: component_id -> directory_id
    let component_table = read_component_table(&mut package)?;

    // Parse the File table: file_key -> (file_name, component_id)
    let file_table = read_file_table(&mut package)?;

    // Parse the Media table to find CAB file names
    let media_entries = read_media_table(&mut package)?;

    log::debug!(
        "  [{}] tables: {} dirs, {} components, {} files, {} media entries",
        msi_name,
        directory_table.len(),
        component_table.len(),
        file_table.len(),
        media_entries.len(),
    );

    let mut extracted_count = 0u32;

    // Try external CABs first (referenced in Media table)
    let mut found_external = false;
    for media in &media_entries {
        if media.cabinet.is_empty() {
            continue;
        }
        let cab_name = &media.cabinet;
        let cab_path = cab_dir.join(cab_name);
        if cab_path.exists() {
            log::debug!("  [{}] extracting external CAB '{}'", msi_name, cab_name);
            let cab_file = fs::File::open(&cab_path)
                .with_context(|| format!("opening CAB file '{}'", cab_path.display()))?;
            let count = extract_cab(
                cab_file,
                install_dir,
                &file_table,
                &component_table,
                &directory_table,
                manifest_file,
            )
            .with_context(|| format!("extracting CAB '{}'", cab_path.display()))?;
            log::debug!(
                "  [{}] extracted {} files from '{}'",
                msi_name,
                count,
                cab_name
            );
            extracted_count += count;
            found_external = true;
        } else {
            log::debug!(
                "  [{}] external CAB '{}' not found at '{}'",
                msi_name,
                cab_name,
                cab_path.display()
            );
        }
    }

    if found_external {
        log::debug!(
            "  [{}] done: {} files from external CAB(s)",
            msi_name,
            extracted_count
        );
        return Ok(());
    }

    // Fall back to embedded CAB streams
    let stream_names: Vec<String> = package.streams().map(|s| s.to_string()).collect();
    log::debug!(
        "  [{}] no external CABs found, checking {} streams for embedded CABs",
        msi_name,
        stream_names.len()
    );
    for media in &media_entries {
        if media.cabinet.is_empty() {
            continue;
        }
        let stream_name = if media.cabinet.starts_with('#') {
            &media.cabinet[1..]
        } else {
            continue;
        };

        if stream_names.iter().any(|s| s == stream_name) {
            log::debug!(
                "  [{}] extracting embedded CAB stream '{}'",
                msi_name,
                stream_name
            );
            let mut reader = package
                .read_stream(stream_name)
                .with_context(|| format!("reading embedded stream '{}'", stream_name))?;
            let mut cab_data = Vec::new();
            reader.read_to_end(&mut cab_data)?;

            let cursor = io::Cursor::new(cab_data);
            let count = extract_cab(
                cursor,
                install_dir,
                &file_table,
                &component_table,
                &directory_table,
                manifest_file,
            )
            .with_context(|| format!("extracting embedded CAB '{}'", stream_name))?;
            log::debug!(
                "  [{}] extracted {} files from embedded '{}'",
                msi_name,
                count,
                stream_name
            );
            extracted_count += count;
        }
    }

    if extracted_count == 0 {
        // Try any stream that looks like a CAB (check for MSCF signature)
        for name in &stream_names {
            let mut reader = match package.read_stream(name) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let mut sig = [0u8; 4];
            if reader.read_exact(&mut sig).is_err() {
                continue;
            }
            if &sig != b"MSCF" {
                continue;
            }

            log::debug!("  [{}] found CAB signature in stream '{}'", msi_name, name);
            let mut cab_data = sig.to_vec();
            reader.read_to_end(&mut cab_data)?;

            let cursor = io::Cursor::new(cab_data);
            let count = extract_cab(
                cursor,
                install_dir,
                &file_table,
                &component_table,
                &directory_table,
                manifest_file,
            )?;
            log::debug!(
                "  [{}] extracted {} files from stream '{}'",
                msi_name,
                count,
                name
            );
            extracted_count += count;
        }
    }

    if extracted_count == 0 {
        if file_table.is_empty() {
            log::debug!(
                "  [{}] no files in File table, nothing to extract (metadata-only MSI)",
                msi_name
            );
        } else {
            log::warn!(
                "  [{}] File table has {} entries but no CAB files found (neither external nor embedded)",
                msi_name,
                file_table.len()
            );
        }
    } else {
        log::debug!(
            "  [{}] done: extracted {} files total",
            msi_name,
            extracted_count
        );
    }
    Ok(())
}

struct FileEntry {
    /// The long filename (after '|' separator if present)
    file_name: String,
    /// Reference to component
    component: String,
}

struct MediaEntry {
    cabinet: String,
}

/// Read the File table from the MSI database.
/// Returns a map from file key to FileEntry.
fn read_file_table(
    package: &mut msi::Package<std::fs::File>,
) -> Result<HashMap<String, FileEntry>> {
    let mut map = HashMap::new();
    if !package.has_table("File") {
        return Ok(map);
    }

    let query = msi::Select::table("File").columns(&["File", "FileName", "Component_"]);
    let rows = package.select_rows(query).context("querying File table")?;

    for row in rows {
        let file_key = row["File"].as_str().unwrap_or_default().to_string();
        let file_name = row["FileName"].as_str().unwrap_or_default().to_string();
        let component = row["Component_"].as_str().unwrap_or_default().to_string();

        if file_key.is_empty() {
            continue;
        }

        map.insert(
            file_key,
            FileEntry {
                file_name,
                component,
            },
        );
    }

    Ok(map)
}

/// Read the Component table from the MSI database.
/// Returns a map from component ID to directory ID.
fn read_component_table(
    package: &mut msi::Package<std::fs::File>,
) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    if !package.has_table("Component") {
        return Ok(map);
    }

    let query = msi::Select::table("Component").columns(&["Component", "Directory_"]);
    let rows = package
        .select_rows(query)
        .context("querying Component table")?;

    for row in rows {
        let component = row["Component"].as_str().unwrap_or_default().to_string();
        let directory = row["Directory_"].as_str().unwrap_or_default().to_string();
        if !component.is_empty() {
            map.insert(component, directory);
        }
    }

    Ok(map)
}

/// Read the Directory table from the MSI database.
/// Returns a map from directory ID to (parent_id, default_dir).
fn read_directory_table(
    package: &mut msi::Package<std::fs::File>,
) -> Result<HashMap<String, (String, String)>> {
    let mut map = HashMap::new();
    if !package.has_table("Directory") {
        return Ok(map);
    }

    let query =
        msi::Select::table("Directory").columns(&["Directory", "Directory_Parent", "DefaultDir"]);
    let rows = package
        .select_rows(query)
        .context("querying Directory table")?;

    for row in rows {
        let dir_id = row["Directory"].as_str().unwrap_or_default().to_string();
        let parent = row["Directory_Parent"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let default_dir = row["DefaultDir"].as_str().unwrap_or_default().to_string();

        if !dir_id.is_empty() {
            map.insert(dir_id, (parent, default_dir));
        }
    }

    Ok(map)
}

/// Read the Media table from the MSI database.
fn read_media_table(package: &mut msi::Package<std::fs::File>) -> Result<Vec<MediaEntry>> {
    let mut entries = Vec::new();
    if !package.has_table("Media") {
        return Ok(entries);
    }

    let query = msi::Select::table("Media").columns(&["Cabinet"]);
    let rows = package.select_rows(query).context("querying Media table")?;

    for row in rows {
        let cabinet = row["Cabinet"].as_str().unwrap_or_default().to_string();
        entries.push(MediaEntry { cabinet });
    }

    Ok(entries)
}

/// Read the cabinet names from an MSI's Media table without extracting.
pub fn read_msi_cab_names(msi_path: &Path) -> Result<Vec<String>> {
    let mut package = msi::open(msi_path)
        .with_context(|| format!("opening MSI file '{}'", msi_path.display()))?;
    let entries = read_media_table(&mut package)?;
    Ok(entries
        .into_iter()
        .filter(|e| !e.cabinet.is_empty())
        .map(|e| e.cabinet)
        .collect())
}

/// Resolve a directory ID to a full path by walking the Directory table parent chain.
fn resolve_directory_path(
    dir_id: &str,
    directory_table: &HashMap<String, (String, String)>,
    cache: &mut HashMap<String, String>,
) -> String {
    if let Some(cached) = cache.get(dir_id) {
        return cached.clone();
    }

    let mut parts = Vec::new();
    let mut current = dir_id.to_string();
    let mut visited = std::collections::HashSet::new();

    loop {
        if visited.contains(&current) {
            break;
        }
        visited.insert(current.clone());

        let Some((parent, default_dir)) = directory_table.get(&current) else {
            break;
        };

        // The DefaultDir field can have format "short|long" or "short:long"
        let dir_name = if let Some(pipe_pos) = default_dir.find('|') {
            &default_dir[pipe_pos + 1..]
        } else if let Some(colon_pos) = default_dir.find(':') {
            &default_dir[colon_pos + 1..]
        } else {
            default_dir.as_str()
        };

        // Skip "." and "SourceDir" entries (they represent the root)
        if dir_name != "." && dir_name != "SourceDir" {
            parts.push(dir_name.to_string());
        }

        if parent.is_empty() {
            break;
        }
        current = parent.clone();
    }

    parts.reverse();

    let resolved = parts.join(std::path::MAIN_SEPARATOR_STR);
    cache.insert(dir_id.to_string(), resolved.clone());
    resolved
}

/// Extract the long filename from an MSI FileName field.
/// MSI uses "short|long" format, e.g. "READM~1.TXT|readme.txt"
fn get_long_filename(filename_field: &str) -> &str {
    if let Some(pipe_pos) = filename_field.find('|') {
        &filename_field[pipe_pos + 1..]
    } else {
        filename_field
    }
}

/// Extract files from a CAB archive using MSI metadata for path resolution.
fn extract_cab<R: Read + io::Seek>(
    reader: R,
    install_dir: &Path,
    file_table: &HashMap<String, FileEntry>,
    component_table: &HashMap<String, String>,
    directory_table: &HashMap<String, (String, String)>,
    manifest_file: &mut fs::File,
) -> Result<u32> {
    let mut cabinet = cab::Cabinet::new(reader).context("parsing CAB file")?;
    let mut dir_cache = HashMap::new();
    let mut extracted = 0u32;

    // Collect all file names from the cabinet first
    let file_names: Vec<String> = cabinet
        .folder_entries()
        .flat_map(|folder| folder.file_entries())
        .map(|entry| entry.name().to_string())
        .collect();

    for cab_file_name in &file_names {
        // Look up this file in the MSI File table
        let (target_dir, actual_name) =
            if let Some(file_entry) = file_table.get(cab_file_name.as_str()) {
                let actual_name = get_long_filename(&file_entry.file_name);

                // Resolve the target directory from Component -> Directory chain
                if let Some(dir_id) = component_table.get(&file_entry.component) {
                    let dir_path = resolve_directory_path(dir_id, directory_table, &mut dir_cache);
                    (dir_path, actual_name.to_string())
                } else {
                    // No component entry, extract to root
                    (String::new(), actual_name.to_string())
                }
            } else {
                // File not in MSI File table, use CAB filename as-is
                (String::new(), cab_file_name.clone())
            };

        let full_dir = if target_dir.is_empty() {
            install_dir.to_path_buf()
        } else {
            install_dir.join(&target_dir)
        };

        fs::create_dir_all(&full_dir)?;

        let full_path = full_dir.join(&actual_name);

        if full_path.exists() {
            writeln!(manifest_file, "add {}", full_path.display())?;
        } else {
            writeln!(manifest_file, "new {}", full_path.display())?;
            let mut reader = cabinet
                .read_file(cab_file_name)
                .with_context(|| format!("reading '{}' from CAB", cab_file_name))?;
            let mut out_file = fs::File::create(&full_path)
                .with_context(|| format!("creating '{}'", full_path.display()))?;
            io::copy(&mut reader, &mut out_file)?;
            extracted += 1;
        }
    }

    Ok(extracted)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- get_long_filename ---

    #[test]
    fn long_filename_with_pipe() {
        assert_eq!(get_long_filename("READM~1.TXT|readme.txt"), "readme.txt");
    }

    #[test]
    fn long_filename_without_pipe() {
        assert_eq!(get_long_filename("readme.txt"), "readme.txt");
    }

    #[test]
    fn long_filename_empty() {
        assert_eq!(get_long_filename(""), "");
    }

    #[test]
    fn long_filename_pipe_at_start() {
        assert_eq!(get_long_filename("|long_name.dll"), "long_name.dll");
    }

    // --- resolve_directory_path ---

    fn make_dir_table(entries: &[(&str, &str, &str)]) -> HashMap<String, (String, String)> {
        entries
            .iter()
            .map(|(id, parent, default)| {
                (id.to_string(), (parent.to_string(), default.to_string()))
            })
            .collect()
    }

    #[test]
    fn resolve_single_directory() {
        let table = make_dir_table(&[("dir1", "", "mydir")]);
        let mut cache = HashMap::new();
        assert_eq!(resolve_directory_path("dir1", &table, &mut cache), "mydir");
    }

    #[test]
    fn resolve_nested_directories() {
        let table = make_dir_table(&[
            ("TARGETDIR", "", "."),
            ("ProgramFiles", "TARGETDIR", "Program Files"),
            ("MyApp", "ProgramFiles", "MyApp"),
        ]);
        let mut cache = HashMap::new();
        let sep = std::path::MAIN_SEPARATOR_STR;
        assert_eq!(
            resolve_directory_path("MyApp", &table, &mut cache),
            format!("Program Files{}MyApp", sep)
        );
    }

    #[test]
    fn resolve_skips_dot_and_sourcedir() {
        let table = make_dir_table(&[
            ("TARGETDIR", "", "SourceDir"),
            ("subdir", "TARGETDIR", "sub"),
        ]);
        let mut cache = HashMap::new();
        assert_eq!(resolve_directory_path("subdir", &table, &mut cache), "sub");
    }

    #[test]
    fn resolve_short_long_format() {
        let table = make_dir_table(&[("dir1", "", "PROGRA~1|Program Files")]);
        let mut cache = HashMap::new();
        assert_eq!(
            resolve_directory_path("dir1", &table, &mut cache),
            "Program Files"
        );
    }

    #[test]
    fn resolve_colon_format() {
        let table = make_dir_table(&[("dir1", "", "short:longname")]);
        let mut cache = HashMap::new();
        assert_eq!(
            resolve_directory_path("dir1", &table, &mut cache),
            "longname"
        );
    }

    #[test]
    fn resolve_caches_result() {
        let table = make_dir_table(&[("dir1", "", "mydir")]);
        let mut cache = HashMap::new();
        resolve_directory_path("dir1", &table, &mut cache);
        assert!(cache.contains_key("dir1"));
        // Second call should return cached value
        assert_eq!(resolve_directory_path("dir1", &table, &mut cache), "mydir");
    }

    #[test]
    fn resolve_unknown_directory() {
        let table = HashMap::new();
        let mut cache = HashMap::new();
        assert_eq!(
            resolve_directory_path("nonexistent", &table, &mut cache),
            ""
        );
    }

    #[test]
    fn resolve_cycle_detection() {
        // dir1 -> dir2 -> dir1 (cycle)
        let table = make_dir_table(&[("dir1", "dir2", "a"), ("dir2", "dir1", "b")]);
        let mut cache = HashMap::new();
        // Should not infinite-loop; result depends on traversal order
        let result = resolve_directory_path("dir1", &table, &mut cache);
        assert!(!result.is_empty()); // Just verify it terminates
    }

    #[test]
    fn resolve_deeply_nested() {
        let table = make_dir_table(&[
            ("TARGETDIR", "", "."),
            ("L1", "TARGETDIR", "level1"),
            ("L2", "L1", "level2"),
            ("L3", "L2", "level3"),
            ("L4", "L3", "level4"),
        ]);
        let mut cache = HashMap::new();
        let sep = std::path::MAIN_SEPARATOR_STR;
        assert_eq!(
            resolve_directory_path("L4", &table, &mut cache),
            format!("level1{}level2{}level3{}level4", sep, sep, sep)
        );
    }

    // --- extract_cab with synthetic CAB data ---

    fn create_test_cab(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = cab::CabinetBuilder::new();
        let folder = builder.add_folder(cab::CompressionType::None);
        for (name, _data) in files {
            folder.add_file(*name);
        }
        let mut output = io::Cursor::new(Vec::new());
        let mut writer = builder.build(&mut output).unwrap();
        for (_name, data) in files {
            let mut file_writer = writer.next_file().unwrap().unwrap();
            file_writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
        output.into_inner()
    }

    #[test]
    fn extract_cab_with_file_table() {
        let cab_data = create_test_cab(&[("file1_key", b"hello world")]);

        let install_dir = std::env::temp_dir().join("msvcup_test_extract_cab_ft");
        let _ = std::fs::remove_dir_all(&install_dir);
        std::fs::create_dir_all(&install_dir).unwrap();

        let manifest_path = install_dir.join("manifest.txt");
        let mut manifest_file = fs::File::create(&manifest_path).unwrap();

        let mut file_table = HashMap::new();
        file_table.insert(
            "file1_key".to_string(),
            FileEntry {
                file_name: "SHORT~1.TXT|hello.txt".to_string(),
                component: "comp1".to_string(),
            },
        );

        let mut component_table = HashMap::new();
        component_table.insert("comp1".to_string(), "subdir1".to_string());

        let mut directory_table = HashMap::new();
        directory_table.insert(
            "subdir1".to_string(),
            ("".to_string(), "myfiles".to_string()),
        );

        let cursor = io::Cursor::new(cab_data);
        let count = extract_cab(
            cursor,
            &install_dir,
            &file_table,
            &component_table,
            &directory_table,
            &mut manifest_file,
        )
        .unwrap();

        assert_eq!(count, 1);

        let extracted_path = install_dir.join("myfiles").join("hello.txt");
        assert!(extracted_path.exists());
        assert_eq!(
            std::fs::read_to_string(&extracted_path).unwrap(),
            "hello world"
        );

        // Verify manifest
        drop(manifest_file);
        let manifest = std::fs::read_to_string(&manifest_path).unwrap();
        assert!(manifest.contains("new"));
        assert!(manifest.contains("hello.txt"));

        let _ = std::fs::remove_dir_all(&install_dir);
    }

    #[test]
    fn extract_cab_without_file_table_uses_cab_name() {
        let cab_data = create_test_cab(&[("raw_file.dat", b"raw content")]);

        let install_dir = std::env::temp_dir().join("msvcup_test_extract_cab_raw");
        let _ = std::fs::remove_dir_all(&install_dir);
        std::fs::create_dir_all(&install_dir).unwrap();

        let manifest_path = install_dir.join("manifest.txt");
        let mut manifest_file = fs::File::create(&manifest_path).unwrap();

        let file_table = HashMap::new(); // empty
        let component_table = HashMap::new();
        let directory_table = HashMap::new();

        let cursor = io::Cursor::new(cab_data);
        let count = extract_cab(
            cursor,
            &install_dir,
            &file_table,
            &component_table,
            &directory_table,
            &mut manifest_file,
        )
        .unwrap();

        assert_eq!(count, 1);
        let extracted_path = install_dir.join("raw_file.dat");
        assert!(extracted_path.exists());
        assert_eq!(
            std::fs::read_to_string(&extracted_path).unwrap(),
            "raw content"
        );

        let _ = std::fs::remove_dir_all(&install_dir);
    }

    #[test]
    fn extract_cab_multiple_files() {
        let cab_data = create_test_cab(&[
            ("file_a", b"content_a"),
            ("file_b", b"content_b"),
            ("file_c", b"content_c"),
        ]);

        let install_dir = std::env::temp_dir().join("msvcup_test_extract_cab_multi");
        let _ = std::fs::remove_dir_all(&install_dir);
        std::fs::create_dir_all(&install_dir).unwrap();

        let manifest_path = install_dir.join("manifest.txt");
        let mut manifest_file = fs::File::create(&manifest_path).unwrap();

        let file_table = HashMap::new();
        let component_table = HashMap::new();
        let directory_table = HashMap::new();

        let cursor = io::Cursor::new(cab_data);
        let count = extract_cab(
            cursor,
            &install_dir,
            &file_table,
            &component_table,
            &directory_table,
            &mut manifest_file,
        )
        .unwrap();

        assert_eq!(count, 3);
        assert!(install_dir.join("file_a").exists());
        assert!(install_dir.join("file_b").exists());
        assert!(install_dir.join("file_c").exists());

        let _ = std::fs::remove_dir_all(&install_dir);
    }

    #[test]
    fn extract_cab_existing_file_skipped() {
        let cab_data = create_test_cab(&[("existing_file", b"new content")]);

        let install_dir = std::env::temp_dir().join("msvcup_test_extract_cab_exist");
        let _ = std::fs::remove_dir_all(&install_dir);
        std::fs::create_dir_all(&install_dir).unwrap();

        // Pre-create the file
        std::fs::write(install_dir.join("existing_file"), "old content").unwrap();

        let manifest_path = install_dir.join("manifest.txt");
        let mut manifest_file = fs::File::create(&manifest_path).unwrap();

        let file_table = HashMap::new();
        let component_table = HashMap::new();
        let directory_table = HashMap::new();

        let cursor = io::Cursor::new(cab_data);
        let count = extract_cab(
            cursor,
            &install_dir,
            &file_table,
            &component_table,
            &directory_table,
            &mut manifest_file,
        )
        .unwrap();

        // Existing files are not re-extracted
        assert_eq!(count, 0);

        // Manifest should say "add" not "new"
        drop(manifest_file);
        let manifest = std::fs::read_to_string(&manifest_path).unwrap();
        assert!(manifest.contains("add"));

        // Content unchanged
        assert_eq!(
            std::fs::read_to_string(install_dir.join("existing_file")).unwrap(),
            "old content"
        );

        let _ = std::fs::remove_dir_all(&install_dir);
    }

    #[test]
    fn extract_cab_nested_directory_resolution() {
        let cab_data = create_test_cab(&[("key1", b"data")]);

        let install_dir = std::env::temp_dir().join("msvcup_test_extract_cab_nested");
        let _ = std::fs::remove_dir_all(&install_dir);
        std::fs::create_dir_all(&install_dir).unwrap();

        let manifest_path = install_dir.join("manifest.txt");
        let mut manifest_file = fs::File::create(&manifest_path).unwrap();

        let mut file_table = HashMap::new();
        file_table.insert(
            "key1".to_string(),
            FileEntry {
                file_name: "output.dll".to_string(),
                component: "comp1".to_string(),
            },
        );

        let mut component_table = HashMap::new();
        component_table.insert("comp1".to_string(), "leaf_dir".to_string());

        let mut directory_table = HashMap::new();
        directory_table.insert(
            "TARGETDIR".to_string(),
            ("".to_string(), "SourceDir".to_string()),
        );
        directory_table.insert(
            "parent_dir".to_string(),
            ("TARGETDIR".to_string(), "lib".to_string()),
        );
        directory_table.insert(
            "leaf_dir".to_string(),
            ("parent_dir".to_string(), "x64".to_string()),
        );

        let cursor = io::Cursor::new(cab_data);
        let count = extract_cab(
            cursor,
            &install_dir,
            &file_table,
            &component_table,
            &directory_table,
            &mut manifest_file,
        )
        .unwrap();

        assert_eq!(count, 1);
        let expected_path = install_dir.join("lib").join("x64").join("output.dll");
        assert!(expected_path.exists());
        assert_eq!(std::fs::read(&expected_path).unwrap(), b"data");

        let _ = std::fs::remove_dir_all(&install_dir);
    }
}
