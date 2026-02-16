use std::cmp::Ordering;

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

pub fn order_dotted_alphabetical(lhs: &str, rhs: &str) -> Ordering {
    let mut lhs_it = lhs.split('.');
    let mut rhs_it = rhs.split('.');
    loop {
        match (lhs_it.next(), rhs_it.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(l), Some(r)) => match l.cmp(r) {
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
    }
}
