//! Autoenv wrapper binary.
//!
//! This executable is meant to be copied/renamed as `cl.exe`, `link.exe`, etc.
//! When invoked, it:
//! 1. Reads the `env` file in the same directory (list of vcvars .bat paths)
//! 2. Parses each vcvars .bat file to set environment variables (PATH, INCLUDE, LIB, etc.)
//! 3. Finds the real tool in the (now-modified) PATH
//! 4. Spawns it as a child process, forwarding all arguments and the exit code
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
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    // 1. Determine our own exe name (e.g. "cl.exe")
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

    // 2. Read the `env` file
    let env_path = self_dir.join("env");
    let env_content = match fs::read_to_string(&env_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "msvcup-autoenv: unable to load environment, '{}' does not exist: {e}",
                env_path.display()
            );
            return 1;
        }
    };

    // 3. Parse each vcvars bat file and set environment variables
    for line in env_content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Err(e) = load_vcvars(line) {
            eprintln!("msvcup-autoenv: error loading '{line}': {e}");
            return 1;
        }
    }

    // 4. Find the real tool in PATH (skipping ourselves)
    let real_exe = match find_in_path(&self_basename, self_dir) {
        Some(p) => p,
        None => {
            eprintln!("msvcup-autoenv: unable to find '{self_basename}' in PATH");
            return 1;
        }
    };

    // 5. Spawn the real tool, forwarding all arguments
    let args: Vec<String> = env::args().skip(1).collect();
    match Command::new(&real_exe).args(&args).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(e) => {
            eprintln!(
                "msvcup-autoenv: failed to execute '{}': {e}",
                real_exe.display()
            );
            1
        }
    }
}

/// Parse a vcvars .bat file and update environment variables.
///
/// Each line in a vcvars bat file looks like:
///   set "PATH=%~dp0..\..\VC\Tools;%PATH%"
///
/// We extract the variable name, the new path entries (replacing %~dp0 with the
/// bat file's directory), and prepend them to the existing env var.
#[cfg(windows)]
fn load_vcvars(vcvars_path: &str) -> Result<(), String> {
    use std::env;
    use std::fs;
    use std::path::Path;

    let content =
        fs::read_to_string(vcvars_path).map_err(|e| format!("cannot read '{vcvars_path}': {e}"))?;

    let root_dir = Path::new(vcvars_path)
        .parent()
        .ok_or_else(|| format!("invalid vcvars path '{vcvars_path}' missing directory"))?
        .to_string_lossy();

    for (lineno, line) in content.lines().enumerate() {
        let lineno = lineno + 1;
        let line = line.trim_end_matches('\r');

        // Expected format: set "NAME=<paths>;%NAME%"
        let prefix = "set \"";
        if !line.starts_with(prefix) {
            return Err(format!(
                "{vcvars_path}:{lineno}: line did not start with '{prefix}'"
            ));
        }
        let after_prefix = &line[prefix.len()..];

        let eq_pos = after_prefix
            .find('=')
            .ok_or_else(|| format!("{vcvars_path}:{lineno}: missing '=' to end name"))?;
        let name = &after_prefix[..eq_pos];

        // Verify line ends with ;%NAME%"
        let expected_suffix = format!(";%{name}%\"");
        if !line.ends_with(&expected_suffix) {
            return Err(format!(
                "{vcvars_path}:{lineno}: line did not end with '{expected_suffix}'"
            ));
        }

        // Extract paths between = and ;%NAME%"
        let paths_start = prefix.len() + eq_pos + 1;
        let paths_end = line.len() - expected_suffix.len();
        let paths_str = &line[paths_start..paths_end];

        // Build new paths, replacing %~dp0 with root_dir
        let dp0 = "%~dp0";
        let mut new_paths = Vec::new();
        for path_entry in paths_str.split(';') {
            if path_entry.is_empty() {
                continue;
            }
            if let Some(rest) = path_entry.strip_prefix(dp0) {
                new_paths.push(format!("{root_dir}\\{rest}"));
            } else {
                return Err(format!(
                    "{vcvars_path}:{lineno}: path entry does not start with '{dp0}': '{path_entry}'"
                ));
            }
        }

        // Prepend new paths to existing env var
        let current = env::var(name).unwrap_or_default();
        let new_value = if current.is_empty() {
            new_paths.join(";")
        } else {
            format!("{};{current}", new_paths.join(";"))
        };
        // SAFETY: this binary is single-threaded
        unsafe {
            env::set_var(name, &new_value);
        }
    }
    Ok(())
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
        // Skip our own directory to avoid infinite recursion
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
    // Try canonical comparison first, fall back to string comparison
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => {
            // Fall back to case-insensitive comparison on Windows
            a.to_string_lossy()
                .eq_ignore_ascii_case(&b.to_string_lossy())
        }
    }
}
