//! Autoenv wrapper binary.
//!
//! This executable is meant to be copied/renamed as `cl.exe`, `link.exe`, etc.
//!
//! **Shim mode** (default, when invoked as a tool name):
//! 1. Reads `msvcup.toml` next to the binary for package info and install dir
//! 2. Loads `env-{arch}.json` from each installed package directory
//! 3. If env JSON is missing, errors with "run msvcup-autoenv install first"
//! 4. Prepends env vars (PATH, INCLUDE, LIB) from the JSON
//! 5. Finds the real tool in PATH and forwards execution
//!
//! **Install mode** (`msvcup-autoenv install`):
//! 1. Reads `msvcup.toml` to find packages and lock file
//! 2. Runs `msvcup install` to download and extract packages
//!
//! On non-Windows platforms this binary just prints an error and exits.

fn main() {
    #[cfg(windows)]
    {
        std::process::exit(windows_main());
    }
    #[cfg(not(windows))]
    {
        eprintln!("msvcup-autoenv: this wrapper is only supported on Windows");
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn windows_main() -> i32 {
    use std::env;

    let args: Vec<String> = env::args().collect();

    // Determine our own exe path
    let self_exe = match env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("msvcup-autoenv: cannot determine own path: {e}");
            return 1;
        }
    };
    let self_dir = match self_exe.parent() {
        Some(d) => d,
        None => {
            eprintln!("msvcup-autoenv: exe path has no parent directory");
            return 1;
        }
    };
    let self_basename = match self_exe.file_name() {
        Some(n) => n.to_string_lossy().to_string(),
        None => {
            eprintln!("msvcup-autoenv: exe path has no file name");
            return 1;
        }
    };

    // Check for install subcommand:
    // Either invoked as `msvcup-autoenv install` or as `msvcup-autoenv.exe install`
    let is_autoenv_name = {
        let lower = self_basename.to_ascii_lowercase();
        lower == "msvcup-autoenv.exe" || lower == "msvcup-autoenv"
    };

    if is_autoenv_name {
        if args.len() >= 2 && args[1] == "install" {
            return match install_command(self_dir) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("msvcup-autoenv install: {e}");
                    1
                }
            };
        }
        eprintln!("usage: msvcup-autoenv install");
        eprintln!("  Installs MSVC packages according to msvcup.toml and the lock file.");
        return 1;
    }

    // Shim mode: forward to the real tool
    match shim_forward(self_dir, &self_basename, &args[1..]) {
        Ok(exit_code) => exit_code,
        Err(e) => {
            eprintln!("msvcup-autoenv: {e}");
            1
        }
    }
}

// --- Directory resolution ---

/// Resolve install_dir with priority: config > MSVCUP_INSTALL_DIR env var > platform default.
#[cfg(windows)]
fn resolve_install_dir(config: &MsvcupConfig) -> String {
    if let Some(ref dir) = config.msvcup.install_dir {
        return dir.clone();
    }
    if let Ok(dir) = std::env::var("MSVCUP_INSTALL_DIR") {
        return dir;
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        format!("{}\\.msvcup", userprofile)
    } else {
        "C:\\msvcup".to_string()
    }
}

/// Resolve cache_dir with priority: config > MSVCUP_CACHE_DIR env var > {install_dir}\cache.
#[cfg(windows)]
fn resolve_cache_dir(config: &MsvcupConfig, install_dir: &str) -> String {
    if let Some(ref dir) = config.msvcup.cache_dir {
        return dir.clone();
    }
    if let Ok(dir) = std::env::var("MSVCUP_CACHE_DIR") {
        return dir;
    }
    format!("{}\\cache", install_dir)
}

// --- Install command ---

#[cfg(windows)]
fn install_command(self_dir: &std::path::Path) -> Result<(), String> {
    use std::process::Command;

    let config = read_config(self_dir)?;

    let install_dir = resolve_install_dir(&config);
    let cache_dir = resolve_cache_dir(&config, &install_dir);

    let lock_file_path = self_dir.join(&config.msvcup.lock_file);
    let lock_file_str = lock_file_path.to_string_lossy();

    let mut pkg_strings: Vec<String> = Vec::new();
    for (name, version) in &config.packages {
        pkg_strings.push(format!("{}-{}", name, version));
    }

    if pkg_strings.is_empty() {
        return Err("no packages specified in msvcup.toml".to_string());
    }

    // Find msvcup binary next to us or in PATH
    let msvcup_exe = find_msvcup_binary(self_dir).ok_or("cannot find 'msvcup' binary")?;

    eprintln!("msvcup-autoenv: installing packages...");

    let mut cmd = Command::new(&msvcup_exe);
    cmd.arg("install")
        .arg("--lock-file")
        .arg(lock_file_str.as_ref())
        .arg("--manifest-update")
        .arg("off")
        .arg("--cache-dir")
        .arg(&cache_dir)
        .arg("--install-dir")
        .arg(&install_dir);
    for pkg in &pkg_strings {
        cmd.arg(pkg);
    }

    let status = cmd
        .status()
        .map_err(|e| format!("failed to run '{}': {e}", msvcup_exe.display()))?;

    if !status.success() {
        return Err(format!(
            "msvcup install failed with exit code {:?}",
            status.code()
        ));
    }

    // Verify env JSON files exist
    let target_arch = &config.msvcup.target_arch;

    for pkg_str in &pkg_strings {
        if pkg_str.starts_with("ninja-") || pkg_str.starts_with("cmake-") {
            continue;
        }
        let json_path = format!("{}\\{}\\env-{}.json", install_dir, pkg_str, target_arch);
        if !std::path::Path::new(&json_path).exists() {
            return Err(format!(
                "installation succeeded but '{}' was not generated",
                json_path
            ));
        }
    }

    eprintln!("msvcup-autoenv: installation complete");
    Ok(())
}

// --- Shim forwarding ---

#[cfg(windows)]
fn shim_forward(
    self_dir: &std::path::Path,
    self_basename: &str,
    args: &[String],
) -> Result<i32, String> {
    use std::process::Command;

    let config = read_config(self_dir)?;

    let install_dir = resolve_install_dir(&config);
    let target_arch = &config.msvcup.target_arch;

    // Collect package strings
    let mut pkg_strings: Vec<String> = Vec::new();
    for (name, version) in &config.packages {
        pkg_strings.push(format!("{}-{}", name, version));
    }

    // Load env JSON for each package and apply env vars
    for pkg_str in &pkg_strings {
        if pkg_str.starts_with("ninja-") || pkg_str.starts_with("cmake-") {
            continue;
        }
        let json_path = format!("{}\\{}\\env-{}.json", install_dir, pkg_str, target_arch);
        load_env_json(&json_path)?;
    }

    // Find and execute the real tool
    let real_exe = find_in_path(self_basename, self_dir).ok_or_else(|| {
        format!(
            "unable to find '{}' in PATH after setting up environment",
            self_basename
        )
    })?;

    match Command::new(&real_exe).args(args).status() {
        Ok(status) => Ok(status.code().unwrap_or(1)),
        Err(e) => Err(format!("failed to execute '{}': {e}", real_exe.display())),
    }
}

// --- Helpers ---

#[cfg(windows)]
fn read_config(self_dir: &std::path::Path) -> Result<MsvcupConfig, String> {
    let config_path = self_dir.join("msvcup.toml");
    if !config_path.exists() {
        return Err(format!(
            "'msvcup.toml' not found in '{}'. Use 'msvcup resolve' to set up the shim directory.",
            self_dir.display()
        ));
    }
    let config_content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("cannot read '{}': {e}", config_path.display()))?;
    toml::from_str(&config_content)
        .map_err(|e| format!("cannot parse '{}': {e}", config_path.display()))
}

/// Load env-{arch}.json and prepend entries to environment variables.
#[cfg(windows)]
fn load_env_json(json_path: &str) -> Result<(), String> {
    use std::collections::HashMap;
    use std::env;

    let content = match std::fs::read_to_string(json_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "packages not installed (missing '{}'). Run 'msvcup-autoenv install' first.",
                json_path
            ));
        }
        Err(e) => return Err(format!("cannot read '{}': {e}", json_path)),
    };

    let env_map: HashMap<String, Vec<String>> =
        serde_json::from_str(&content).map_err(|e| format!("cannot parse '{}': {e}", json_path))?;

    for (name, new_paths) in &env_map {
        if new_paths.is_empty() {
            continue;
        }
        let current = env::var(name).unwrap_or_default();
        let new_value = if current.is_empty() {
            new_paths.join(";")
        } else {
            format!("{};{}", new_paths.join(";"), current)
        };
        // SAFETY: this binary is single-threaded
        unsafe {
            env::set_var(name, &new_value);
        }
    }
    Ok(())
}

/// Find the msvcup binary: first next to ourselves, then in PATH.
#[cfg(windows)]
fn find_msvcup_binary(self_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    for name in &["msvcup.exe", "msvcup"] {
        let candidate = self_dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    let path_var = std::env::var("PATH").ok()?;
    for dir in path_var.split(';') {
        if dir.is_empty() {
            continue;
        }
        let candidate = std::path::PathBuf::from(dir).join("msvcup.exe");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Search PATH for an executable, skipping the directory `skip_dir` (our own dir).
#[cfg(windows)]
fn find_in_path(exe_name: &str, skip_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::env;
    use std::path::PathBuf;

    let path_var = env::var("PATH").ok()?;
    for dir in path_var.split(';') {
        if dir.is_empty() {
            continue;
        }
        let dir_path = PathBuf::from(dir);
        if same_dir(&dir_path, skip_dir) {
            continue;
        }
        let candidate = dir_path.join(exe_name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Check if two directory paths refer to the same directory.
#[cfg(windows)]
fn same_dir(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy()),
    }
}

// --- Config types ---

#[allow(dead_code)]
#[derive(serde::Deserialize)]
struct MsvcupConfig {
    msvcup: MsvcupSettings,
    packages: std::collections::BTreeMap<String, String>,
}

#[allow(dead_code)]
#[derive(serde::Deserialize)]
struct MsvcupSettings {
    cache_dir: Option<String>,
    install_dir: Option<String>,
    lock_file: String,
    target_arch: String,
}
