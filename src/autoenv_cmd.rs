use crate::arch::Arch;

pub struct Tool {
    pub name: &'static str,
    pub cmake_names: &'static [&'static str],
}

pub const MSVC_TOOLS: &[Tool] = &[
    Tool {
        name: "cl",
        cmake_names: &["C_COMPILER", "CXX_COMPILER"],
    },
    Tool {
        name: "ml64",
        cmake_names: &["ASM_COMPILER"],
    },
    Tool {
        name: "link",
        cmake_names: &["LINKER"],
    },
    Tool {
        name: "lib",
        cmake_names: &["AR"],
    },
];

pub const SDK_TOOLS: &[Tool] = &[
    Tool {
        name: "rc",
        cmake_names: &["RC_COMPILER"],
    },
    Tool {
        name: "mt",
        cmake_names: &["MT"],
    },
];

pub fn generate_toolchain_cmake(target_cpu: Arch, has_msvc: bool, has_sdk: bool) -> String {
    let mut content = String::new();
    content.push_str("set(CMAKE_SYSTEM_NAME Windows)\n");

    let processor = match target_cpu {
        Arch::X64 => Some("AMD64"),
        Arch::X86 => Some("X86"),
        Arch::Arm => None,
        Arch::Arm64 => Some("ARM64"),
    };
    if let Some(proc) = processor {
        content.push_str(&format!("set(CMAKE_SYSTEM_PROCESSOR {})\n", proc));
    }

    if has_msvc {
        for tool in MSVC_TOOLS {
            for cmake_name in tool.cmake_names {
                content.push_str(&format!(
                    "set(CMAKE_{} \"${{CMAKE_CURRENT_LIST_DIR}}/{}.exe\")\n",
                    cmake_name, tool.name
                ));
            }
        }
    }
    if has_sdk {
        for tool in SDK_TOOLS {
            for cmake_name in tool.cmake_names {
                content.push_str(&format!(
                    "set(CMAKE_{} \"${{CMAKE_CURRENT_LIST_DIR}}/{}.exe\")\n",
                    cmake_name, tool.name
                ));
            }
        }
    }

    content
}
