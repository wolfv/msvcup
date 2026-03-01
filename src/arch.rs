//! Target architecture types.
//!
//! Represents the CPU architectures supported by the MSVC toolchain:
//! x64, x86, arm, and arm64.

use std::fmt;

/// A target CPU architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Arch {
    X64,
    X86,
    Arm,
    Arm64,
}

impl Arch {
    pub fn native() -> Option<Arch> {
        if cfg!(target_arch = "x86_64") {
            Some(Arch::X64)
        } else if cfg!(target_arch = "x86") {
            Some(Arch::X86)
        } else if cfg!(target_arch = "arm") {
            Some(Arch::Arm)
        } else if cfg!(target_arch = "aarch64") {
            Some(Arch::Arm64)
        } else {
            None
        }
    }

    pub fn from_str_exact(s: &str) -> Option<Arch> {
        match s {
            "x64" => Some(Arch::X64),
            "x86" => Some(Arch::X86),
            "arm" => Some(Arch::Arm),
            "arm64" => Some(Arch::Arm64),
            _ => None,
        }
    }

    pub fn from_str_ignore_case(s: &str) -> Option<Arch> {
        if s.eq_ignore_ascii_case("x64") {
            Some(Arch::X64)
        } else if s.eq_ignore_ascii_case("x86") {
            Some(Arch::X86)
        } else if s.eq_ignore_ascii_case("arm") {
            Some(Arch::Arm)
        } else if s.eq_ignore_ascii_case("arm64") {
            Some(Arch::Arm64)
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Arch::X64 => "x64",
            Arch::X86 => "x86",
            Arch::Arm => "arm",
            Arch::Arm64 => "arm64",
        }
    }

    pub const ALL: [Arch; 4] = [Arch::X64, Arch::X86, Arch::Arm, Arch::Arm64];
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_returns_some() {
        // On any supported CI platform, native() should return Some
        assert!(Arch::native().is_some());
    }

    #[test]
    fn from_str_exact_valid() {
        assert_eq!(Arch::from_str_exact("x64"), Some(Arch::X64));
        assert_eq!(Arch::from_str_exact("x86"), Some(Arch::X86));
        assert_eq!(Arch::from_str_exact("arm"), Some(Arch::Arm));
        assert_eq!(Arch::from_str_exact("arm64"), Some(Arch::Arm64));
    }

    #[test]
    fn from_str_exact_rejects_wrong_case() {
        assert_eq!(Arch::from_str_exact("X64"), None);
        assert_eq!(Arch::from_str_exact("ARM64"), None);
    }

    #[test]
    fn from_str_exact_rejects_unknown() {
        assert_eq!(Arch::from_str_exact(""), None);
        assert_eq!(Arch::from_str_exact("riscv64"), None);
    }

    #[test]
    fn from_str_ignore_case_valid() {
        assert_eq!(Arch::from_str_ignore_case("X64"), Some(Arch::X64));
        assert_eq!(Arch::from_str_ignore_case("x64"), Some(Arch::X64));
        assert_eq!(Arch::from_str_ignore_case("X86"), Some(Arch::X86));
        assert_eq!(Arch::from_str_ignore_case("ARM"), Some(Arch::Arm));
        assert_eq!(Arch::from_str_ignore_case("Arm64"), Some(Arch::Arm64));
        assert_eq!(Arch::from_str_ignore_case("ARM64"), Some(Arch::Arm64));
    }

    #[test]
    fn from_str_ignore_case_rejects_unknown() {
        assert_eq!(Arch::from_str_ignore_case(""), None);
        assert_eq!(Arch::from_str_ignore_case("mips"), None);
    }

    #[test]
    fn as_str_roundtrip() {
        for arch in Arch::ALL {
            assert_eq!(Arch::from_str_exact(arch.as_str()), Some(arch));
        }
    }

    #[test]
    fn display_matches_as_str() {
        for arch in Arch::ALL {
            assert_eq!(format!("{}", arch), arch.as_str());
        }
    }

    #[test]
    fn all_contains_four_variants() {
        assert_eq!(Arch::ALL.len(), 4);
    }
}
