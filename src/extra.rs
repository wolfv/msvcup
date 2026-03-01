//! URL parsing for standalone packages (Ninja, CMake).
//!
//! These packages are distributed as standalone downloads rather than through
//! the MSI-based Visual Studio installer, so their URLs need special handling.

use crate::arch::Arch;

pub enum ParseUrlResult {
    Ok { arch: Arch },
    Unexpected { offset: usize, what: String },
}

pub fn parse_url(url: &str) -> ParseUrlResult {
    let ninja_prefix = "https://github.com/ninja-build/ninja/releases/download/v";
    if let Some(rest) = url.strip_prefix(ninja_prefix) {
        let version_end = scan_version(rest, 0);
        if version_end == 0 {
            return ParseUrlResult::Unexpected {
                offset: ninja_prefix.len(),
                what: "a version".to_string(),
            };
        }
        let remaining = &rest[version_end..];
        let arch = if remaining == "/ninja-win.zip" {
            Arch::X64
        } else if remaining == "/ninja-winarm64.zip" {
            Arch::Arm64
        } else {
            return ParseUrlResult::Unexpected {
                offset: ninja_prefix.len() + version_end,
                what: "either '/ninja-win.zip' or '/ninja-winarm64.zip'".to_string(),
            };
        };
        return ParseUrlResult::Ok { arch };
    }

    let cmake_prefix = "https://github.com/Kitware/CMake/releases/download/v";
    if let Some(rest) = url.strip_prefix(cmake_prefix) {
        let version_end = scan_version(rest, 0);
        if version_end == 0 {
            return ParseUrlResult::Unexpected {
                offset: cmake_prefix.len(),
                what: "a version".to_string(),
            };
        }
        let version = &rest[..version_end];
        let remaining = &rest[version_end..];

        let expected_mid = format!("/cmake-{}-windows-", version);
        if let Some(after_mid) = remaining.strip_prefix(expected_mid.as_str()) {
            let arch = if after_mid == "x86_64.zip" {
                Arch::X64
            } else if after_mid == "i386.zip" {
                Arch::X86
            } else if after_mid == "arm64.zip" {
                Arch::Arm64
            } else {
                return ParseUrlResult::Unexpected {
                    offset: cmake_prefix.len() + version_end + expected_mid.len(),
                    what: "'x86_64.zip', 'i386.zip', or 'arm64.zip'".to_string(),
                };
            };
            return ParseUrlResult::Ok { arch };
        } else {
            return ParseUrlResult::Unexpected {
                offset: cmake_prefix.len() + version_end,
                what: "'/cmake-<version>-windows-<arch>.zip'".to_string(),
            };
        }
    }

    ParseUrlResult::Unexpected {
        offset: 0,
        what: format!("either '{}' or '{}'", ninja_prefix, cmake_prefix),
    }
}

fn scan_version(s: &str, start: usize) -> usize {
    let bytes = s.as_bytes();
    let mut offset = start;
    while offset < bytes.len() {
        match bytes[offset] {
            b'.' | b'0'..=b'9' => offset += 1,
            _ => break,
        }
    }
    offset - start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ninja_x64() {
        let url = "https://github.com/ninja-build/ninja/releases/download/v1.12.1/ninja-win.zip";
        match parse_url(url) {
            ParseUrlResult::Ok { arch } => assert_eq!(arch, Arch::X64),
            ParseUrlResult::Unexpected { offset, what } => {
                panic!("unexpected at {}: {}", offset, what)
            }
        }
    }

    #[test]
    fn ninja_arm64() {
        let url =
            "https://github.com/ninja-build/ninja/releases/download/v1.12.1/ninja-winarm64.zip";
        match parse_url(url) {
            ParseUrlResult::Ok { arch } => assert_eq!(arch, Arch::Arm64),
            ParseUrlResult::Unexpected { offset, what } => {
                panic!("unexpected at {}: {}", offset, what)
            }
        }
    }

    #[test]
    fn ninja_no_version() {
        let url = "https://github.com/ninja-build/ninja/releases/download/v/ninja-win.zip";
        assert!(matches!(parse_url(url), ParseUrlResult::Unexpected { .. }));
    }

    #[test]
    fn ninja_bad_suffix() {
        let url = "https://github.com/ninja-build/ninja/releases/download/v1.12.1/ninja-linux.zip";
        assert!(matches!(parse_url(url), ParseUrlResult::Unexpected { .. }));
    }

    #[test]
    fn cmake_x64() {
        let url = "https://github.com/Kitware/CMake/releases/download/v3.31.4/cmake-3.31.4-windows-x86_64.zip";
        match parse_url(url) {
            ParseUrlResult::Ok { arch } => assert_eq!(arch, Arch::X64),
            ParseUrlResult::Unexpected { offset, what } => {
                panic!("unexpected at {}: {}", offset, what)
            }
        }
    }

    #[test]
    fn cmake_x86() {
        let url = "https://github.com/Kitware/CMake/releases/download/v3.31.4/cmake-3.31.4-windows-i386.zip";
        match parse_url(url) {
            ParseUrlResult::Ok { arch } => assert_eq!(arch, Arch::X86),
            ParseUrlResult::Unexpected { offset, what } => {
                panic!("unexpected at {}: {}", offset, what)
            }
        }
    }

    #[test]
    fn cmake_arm64() {
        let url = "https://github.com/Kitware/CMake/releases/download/v3.31.4/cmake-3.31.4-windows-arm64.zip";
        match parse_url(url) {
            ParseUrlResult::Ok { arch } => assert_eq!(arch, Arch::Arm64),
            ParseUrlResult::Unexpected { offset, what } => {
                panic!("unexpected at {}: {}", offset, what)
            }
        }
    }

    #[test]
    fn cmake_no_version() {
        let url = "https://github.com/Kitware/CMake/releases/download/v/cmake--windows-x86_64.zip";
        assert!(matches!(parse_url(url), ParseUrlResult::Unexpected { .. }));
    }

    #[test]
    fn cmake_bad_arch() {
        let url = "https://github.com/Kitware/CMake/releases/download/v3.31.4/cmake-3.31.4-windows-mips.zip";
        assert!(matches!(parse_url(url), ParseUrlResult::Unexpected { .. }));
    }

    #[test]
    fn unknown_url() {
        assert!(matches!(
            parse_url("https://example.com/something"),
            ParseUrlResult::Unexpected { offset: 0, .. }
        ));
    }

    #[test]
    fn empty_url() {
        assert!(matches!(
            parse_url(""),
            ParseUrlResult::Unexpected { offset: 0, .. }
        ));
    }

    #[test]
    fn scan_version_basic() {
        assert_eq!(scan_version("1.12.1/rest", 0), 6);
        assert_eq!(scan_version("3.31.4/rest", 0), 6);
        assert_eq!(scan_version("abc", 0), 0);
        assert_eq!(scan_version("", 0), 0);
        assert_eq!(scan_version("123", 0), 3);
    }
}
