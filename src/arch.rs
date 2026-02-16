use std::fmt;

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
