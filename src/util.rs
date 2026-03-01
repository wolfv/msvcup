//! Shared utility functions.
//!
//! Version string parsing and comparison, sorted insertion, URL basename
//! extraction, percent-decoding, and atomic file writing.

use anyhow::Result;
use std::cmp::Ordering;
use std::path::Path;

pub fn order_dotted_numeric(lhs: &str, rhs: &str) -> Ordering {
    let mut lhs_it = lhs.split('.');
    let mut rhs_it = rhs.split('.');
    loop {
        match (lhs_it.next(), rhs_it.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(l), Some(r)) => match order_numeric(l, r) {
                Ordering::Equal => continue,
                other => return other,
            },
        }
    }
}

pub fn order_numeric(lhs: &str, rhs: &str) -> Ordering {
    match (lhs.parse::<u64>(), rhs.parse::<u64>()) {
        (Ok(l), Ok(r)) => l.cmp(&r),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => lhs.cmp(rhs),
    }
}

pub fn is_valid_version(version: &str) -> bool {
    if version.is_empty() {
        return false;
    }
    scan_id_version(version, 0).1 == version.len()
}

/// Returns (slice, end_offset). Scans a dotted numeric version like "14.30.17.6"
pub fn scan_id_version(id: &str, start: usize) -> (&str, usize) {
    let bytes = id.as_bytes();
    let mut offset = start;
    while offset < bytes.len() {
        match bytes[offset] {
            b'.' | b'0'..=b'9' => offset += 1,
            _ => break,
        }
    }
    // Trim trailing dots
    while offset > start && bytes[offset - 1] == b'.' {
        offset -= 1;
    }
    // Must have at least one digit
    if offset == start {
        return (&id[start..start], start);
    }
    (&id[start..offset], offset)
}

/// Scans to the next occurrence of `to` char, returns (slice, end_after_delim)
pub fn scan_to(s: &str, start: usize, to: char) -> (&str, usize) {
    if let Some(pos) = s[start..].find(to) {
        let abs_pos = start + pos;
        if abs_pos > start {
            return (&s[start..abs_pos], abs_pos + 1);
        }
    }
    (&s[start..], s.len())
}

/// Scans an id part (delimited by '.')
pub fn scan_id_part(id: &str, start: usize) -> (&str, usize) {
    scan_to(id, start, '.')
}

pub fn basename_from_url(url: &str) -> &str {
    match url.rfind('/') {
        Some(i) => &url[i + 1..],
        None => url,
    }
}

/// Insert into a sorted Vec, deduplicating
pub fn insert_sorted<T, F>(list: &mut Vec<T>, item: T, cmp: F)
where
    F: Fn(&T, &T) -> Ordering,
{
    match list.binary_search_by(|probe| cmp(probe, &item)) {
        Ok(_) => {} // already present
        Err(pos) => list.insert(pos, item),
    }
}

/// Write `content` to `path` only if it differs from the existing file.
pub fn update_file(path: &Path, content: &[u8]) -> Result<()> {
    let needs_update = match fs_err::read(path) {
        Ok(existing) => existing != content,
        Err(_) => true,
    };
    if needs_update {
        log::debug!("{}: updating...", path.display());
        fs_err::write(path, content)?;
    } else {
        log::debug!("{}: already up-to-date", path.display());
    }
    Ok(())
}

pub fn alloc_url_percent_decoded(url: &str) -> String {
    percent_encoding::percent_decode_str(url)
        .decode_utf8_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_dotted_numeric() {
        assert_eq!(order_dotted_numeric("0.1", "0.1"), Ordering::Equal);
        assert_eq!(order_dotted_numeric("0", "0.1"), Ordering::Less);
        assert_eq!(order_dotted_numeric("0.1", "0"), Ordering::Greater);
        assert_eq!(order_dotted_numeric("9", "10"), Ordering::Less);
        assert_eq!(
            order_dotted_numeric("14.30.17.6", "14.30.17.6"),
            Ordering::Equal
        );
        assert_eq!(
            order_dotted_numeric("14.30.17.6", "14.30.17.7"),
            Ordering::Less
        );
        assert_eq!(
            order_dotted_numeric("14.31.0.0", "14.30.99.99"),
            Ordering::Greater
        );
        assert_eq!(order_dotted_numeric("1.0.0", "1.0.0.1"), Ordering::Less);
    }

    #[test]
    fn test_order_numeric() {
        assert_eq!(order_numeric("0", "0"), Ordering::Equal);
        assert_eq!(order_numeric("0", "1"), Ordering::Less);
        assert_eq!(order_numeric("1", "0"), Ordering::Greater);
        assert_eq!(order_numeric("9", "10"), Ordering::Less);
        assert_eq!(order_numeric("10", "9"), Ordering::Greater);
        assert_eq!(order_numeric("0", "a"), Ordering::Less);
        assert_eq!(order_numeric("a", "0"), Ordering::Greater);
        assert_eq!(order_numeric("abc", "def"), Ordering::Less);
    }

    #[test]
    fn test_is_valid_version() {
        assert!(is_valid_version("14.30.17.6"));
        assert!(is_valid_version("1"));
        assert!(is_valid_version("10.0.22621.7"));
        assert!(is_valid_version("0.0.0"));

        assert!(!is_valid_version(""));
        assert!(!is_valid_version("abc"));
        assert!(!is_valid_version("14.abc"));
    }

    #[test]
    fn test_scan_id_version() {
        assert_eq!(scan_id_version("14.30.17.6", 0), ("14.30.17.6", 10));
        assert_eq!(scan_id_version("14.30.17.6.rest", 0), ("14.30.17.6", 10));
        assert_eq!(scan_id_version("abc", 0), ("", 0));
        assert_eq!(scan_id_version("14.", 0), ("14", 2));
        assert_eq!(scan_id_version("14.30.abc", 0), ("14.30", 5));
    }

    #[test]
    fn test_scan_id_version_with_offset() {
        assert_eq!(scan_id_version("prefix14.30", 6), ("14.30", 11));
    }

    #[test]
    fn test_scan_to() {
        assert_eq!(scan_to("hello.world", 0, '.'), ("hello", 6));
        assert_eq!(scan_to("hello", 0, '.'), ("hello", 5));
        assert_eq!(scan_to(".hello", 0, '.'), (".hello", 6)); // starts with delimiter
    }

    #[test]
    fn test_scan_id_part() {
        assert_eq!(scan_id_part("Tools.HostX64.TargetX64", 0), ("Tools", 6));
        assert_eq!(scan_id_part("Tools.HostX64.TargetX64", 6), ("HostX64", 14));
        assert_eq!(scan_id_part("end", 0), ("end", 3));
    }

    #[test]
    fn test_basename_from_url() {
        assert_eq!(
            basename_from_url("https://example.com/path/to/file.vsix"),
            "file.vsix"
        );
        assert_eq!(basename_from_url("file.msi"), "file.msi");
        assert_eq!(basename_from_url("https://example.com/"), "");
        assert_eq!(
            basename_from_url("https://example.com/deep/nested/path/archive.cab"),
            "archive.cab"
        );
    }

    #[test]
    fn test_insert_sorted_ascending() {
        let mut list: Vec<i32> = Vec::new();
        insert_sorted(&mut list, 3, |a, b| a.cmp(b));
        insert_sorted(&mut list, 1, |a, b| a.cmp(b));
        insert_sorted(&mut list, 2, |a, b| a.cmp(b));
        assert_eq!(list, vec![1, 2, 3]);
    }

    #[test]
    fn test_insert_sorted_deduplicates() {
        let mut list: Vec<i32> = Vec::new();
        insert_sorted(&mut list, 1, |a, b| a.cmp(b));
        insert_sorted(&mut list, 1, |a, b| a.cmp(b));
        insert_sorted(&mut list, 2, |a, b| a.cmp(b));
        assert_eq!(list, vec![1, 2]);
    }

    #[test]
    fn test_insert_sorted_empty() {
        let mut list: Vec<i32> = Vec::new();
        insert_sorted(&mut list, 42, |a, b| a.cmp(b));
        assert_eq!(list, vec![42]);
    }

    #[test]
    fn test_alloc_url_percent_decoded() {
        assert_eq!(alloc_url_percent_decoded("hello%20world"), "hello world");
        assert_eq!(alloc_url_percent_decoded("no%20encoding"), "no encoding");
        assert_eq!(alloc_url_percent_decoded("plain"), "plain");
        assert_eq!(alloc_url_percent_decoded("path/to%2Ffile"), "path/to/file");
    }

    #[test]
    fn test_update_file() {
        let dir = std::env::temp_dir().join("msvcup_test_update_file");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let path = dir.join("test.txt");

        // Create new file
        update_file(&path, b"hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");

        // No-op when content matches
        update_file(&path, b"hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");

        // Update when content differs
        update_file(&path, b"world").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "world");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
