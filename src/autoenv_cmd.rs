use crate::arch::Arch;
use crate::packages::{MsvcupPackage, MsvcupPackageKind};
use anyhow::{Result, bail};
use std::fs;
use std::io::Write;
use std::path::Path;

struct Tool {
    name: &'static str,
    cmake_names: &'static [&'static str],
}

const MSVC_TOOLS: &[Tool] = &[
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

const SDK_TOOLS: &[Tool] = &[
    Tool {
        name: "rc",
        cmake_names: &["RC_COMPILER"],
    },
    Tool {
        name: "mt",
        cmake_names: &["MT"],
    },
];

pub fn autoenv_command(
    msvcup_pkgs: &[MsvcupPackage],
    target_cpu: Arch,
    out_dir: &str,
) -> Result<()> {
    fs::create_dir_all(out_dir)?;

    let mut maybe_msvc_version: Option<&str> = None;
    let mut maybe_sdk_version: Option<&str> = None;

    // Write env file
    {
        let env_path = Path::new(out_dir).join("env");
        let mut env_file = fs::File::create(&env_path)?;

        for pkg in msvcup_pkgs {
            match pkg.kind {
                MsvcupPackageKind::Msvc => {
                    if maybe_msvc_version.is_some() {
                        bail!("you can't specify multiple msvc packages");
                    }
                    maybe_msvc_version = Some(&pkg.version);
                }
                MsvcupPackageKind::Sdk => {
                    if maybe_sdk_version.is_some() {
                        bail!("you can't specify multiple sdk packages");
                    }
                    maybe_sdk_version = Some(&pkg.version);
                }
                MsvcupPackageKind::Msbuild | MsvcupPackageKind::Diasdk => {}
                MsvcupPackageKind::Ninja | MsvcupPackageKind::Cmake => continue,
            }
            let vcvars_path = format!(
                "C:\\msvcup\\{}\\vcvars-{}.bat",
                pkg.pool_string(),
                target_cpu
            );
            // Check file exists
            if !Path::new(&vcvars_path).exists() {
                bail!("package '{}' has no vcvars file '{}'", pkg, vcvars_path);
            }
            writeln!(env_file, "{}", vcvars_path)?;
        }
    }

    // Write autoenv exe wrappers (cl.exe, link.exe, etc.)
    let autoenv_exe = find_autoenv_binary()?;
    if maybe_msvc_version.is_some() {
        for tool in MSVC_TOOLS {
            let dest = Path::new(out_dir).join(format!("{}.exe", tool.name));
            update_file_from_file(&autoenv_exe, &dest)?;
        }
    }
    if maybe_sdk_version.is_some() {
        for tool in SDK_TOOLS {
            let dest = Path::new(out_dir).join(format!("{}.exe", tool.name));
            update_file_from_file(&autoenv_exe, &dest)?;
        }
    }

    // Generate toolchain.cmake
    {
        let cmake = generate_toolchain_cmake(
            target_cpu,
            maybe_msvc_version.is_some(),
            maybe_sdk_version.is_some(),
        );
        let cmake_path = Path::new(out_dir).join("toolchain.cmake");
        update_file(&cmake_path, cmake.as_bytes())?;
    }

    // Generate libc.txt
    {
        let libc_txt = generate_libc_txt(maybe_msvc_version, maybe_sdk_version, target_cpu)?;
        let libc_path = Path::new(out_dir).join("libc.txt");
        update_file(&libc_path, libc_txt.as_bytes())?;
    }

    Ok(())
}

fn generate_toolchain_cmake(target_cpu: Arch, has_msvc: bool, has_sdk: bool) -> String {
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

fn generate_libc_txt(
    maybe_msvc_version: Option<&str>,
    maybe_sdk_version: Option<&str>,
    _target_cpu: Arch,
) -> Result<String> {
    let mut content = String::new();

    if let Some(_msvc_version) = maybe_msvc_version {
        // We'd need the install version (from the installed directory), but for now
        // use a placeholder approach - the actual install version comes from querying
        // the installed directory
        log::warn!("libc.txt generation for MSVC requires querying installed version");
    }

    if let Some(_sdk_version) = maybe_sdk_version {
        log::warn!("libc.txt generation for SDK requires querying installed version");
    }

    content.push_str("gcc_dir=\n");
    Ok(content)
}

/// Find the msvcup-autoenv binary next to the current executable.
fn find_autoenv_binary() -> Result<std::path::PathBuf> {
    let current_exe = std::env::current_exe()?;
    let dir = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot determine directory of current executable"))?;

    // Look for msvcup-autoenv or msvcup-autoenv.exe next to ourselves
    let candidates = ["msvcup-autoenv.exe", "msvcup-autoenv"];
    for name in &candidates {
        let path = dir.join(name);
        if path.exists() {
            return Ok(path);
        }
    }
    bail!(
        "cannot find msvcup-autoenv binary in '{}'. Build it with: cargo build --bin msvcup-autoenv",
        dir.display()
    );
}

/// Copy a file to `dest`, only if the content differs (or dest doesn't exist).
fn update_file_from_file(src: &Path, dest: &Path) -> Result<()> {
    let src_content = fs::read(src)?;
    let needs_update = match fs::read(dest) {
        Ok(existing) => existing != src_content,
        Err(_) => true,
    };
    if needs_update {
        log::info!("{}: updating...", dest.display());
        fs::copy(src, dest)?;
    } else {
        log::info!("{}: already up-to-date", dest.display());
    }
    Ok(())
}

fn update_file(path: &Path, content: &[u8]) -> Result<()> {
    let needs_update = match fs::read(path) {
        Ok(existing) => existing != content,
        Err(_) => true,
    };
    if needs_update {
        log::info!("{}: updating...", path.display());
        fs::write(path, content)?;
    } else {
        log::info!("{}: already up-to-date", path.display());
    }
    Ok(())
}
