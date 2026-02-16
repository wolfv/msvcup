use crate::arch::Arch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    Ninja,
    Cmake,
}

#[derive(Debug, Clone)]
pub struct Package {
    pub kind: PackageKind,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct ExtraPayload {
    pub package: Package,
    pub arch: Arch,
}

pub enum ParseUrlResult {
    Ok(ExtraPayload),
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
        let version = &rest[..version_end];
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
        return ParseUrlResult::Ok(ExtraPayload {
            package: Package {
                kind: PackageKind::Ninja,
                version: version.to_string(),
            },
            arch,
        });
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
            return ParseUrlResult::Ok(ExtraPayload {
                package: Package {
                    kind: PackageKind::Cmake,
                    version: version.to_string(),
                },
                arch,
            });
        } else {
            return ParseUrlResult::Unexpected {
                offset: cmake_prefix.len() + version_end,
                what: format!("'/cmake-<version>-windows-<arch>.zip'"),
            };
        }
    }

    ParseUrlResult::Unexpected {
        offset: 0,
        what: format!(
            "either '{}' or '{}'",
            ninja_prefix, cmake_prefix
        ),
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
