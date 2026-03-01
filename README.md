# msvcup

A standalone tool for installing the MSVC toolchain and Windows SDK without Visual Studio.

This is a Rust port of the original [Zig implementation](https://github.com/marler8997/msvcup).

## Why?

The Visual Studio Installer manages thousands of components, modifies the registry, and can take hours to configure. msvcup treats the toolchain as a versioned asset rather than global system state. The build environment is defined by code, not a GUI.

- **Fast**: Runs in milliseconds when already installed. Put it at the start of every build script.
- **Reproducible**: Lock file ensures everyone gets the same toolchain.
- **Isolated**: Every package is installed to its own versioned directory. No registry modifications. No conflicts.
- **Cross-compilation**: Target x64, arm64, or x86 out of the box.
- **Minimal**: Download only what's needed to get a working native toolchain/SDK.

## Installation

### From crates.io

```sh
cargo install msvcup
```

### From conda-forge

```sh
pixi global install msvcup
# or
conda install msvcup
```

### From GitHub Releases

Pre-built binaries are available for Windows (x64, arm64), Linux (x64, arm64), and macOS (x64, arm64) on the [releases page](https://github.com/wolfv/msvcup/releases).

### From source

```sh
git clone https://github.com/wolfv/msvcup
cd msvcup
cargo install --path .
```

## Quick Start

### 1. List available packages

```sh
msvcup list
```

Output shows all available MSVC, SDK, MSBuild, DiaSDK, Ninja, and CMake packages with their versions:

```
cmake-3.31.4
diasdk-17.14.35431
msvc-14.44.17.14
msbuild-17.14.8.53709
ninja-1.12.1
sdk-10.0.26100.4
```

### 2. Install packages

```batch
msvcup install --lock-file msvcup.lock --manifest-update daily msvc-14.44.17.14 sdk-10.0.22621.7
```

All packages are installed to `C:\msvcup` (or the configured install directory), creating versioned directories:

```
C:\msvcup\msvc-14.44.17.14\
C:\msvcup\sdk-10.0.22621.7\
```

### 3. Use the toolchain

Each installed package includes vcvars scripts for setting up your shell environment:

```batch
call C:\msvcup\msvc-14.44.17.14\vcvars-x64.bat
call C:\msvcup\sdk-10.0.22621.7\vcvars-x64.bat
cl.exe hello.c /Fe:hello.exe
```

## Configuration

### msvcup.toml

For repeatable setups (CI, team development), create a `msvcup.toml` config file:

```toml
[msvcup]
lock_file = "msvcup.lock"
target_arch = "x64"

[packages]
msvc = "14.44.17.14"
sdk = "10.0.22621.7"
```

#### Fields

| Field | Required | Description |
|-------|----------|-------------|
| `lock_file` | Yes | Path to the lock file (relative to config directory) |
| `target_arch` | Yes | Target architecture: `x64`, `x86`, `arm64`, or `arm` |
| `install_dir` | No | Installation directory (overrides env var and default) |
| `cache_dir` | No | Download cache directory (defaults to `{install_dir}/cache`) |

#### Package names

| Package | Description |
|---------|-------------|
| `msvc` | MSVC compiler toolchain (cl.exe, link.exe, lib.exe, etc.) |
| `sdk` | Windows SDK (headers, libs, rc.exe, mt.exe, etc.) |
| `msbuild` | MSBuild |
| `diasdk` | Debug Interface Access SDK |
| `ninja` | Ninja build system |
| `cmake` | CMake |

### Environment variables

| Variable | Description |
|----------|-------------|
| `MSVCUP_INSTALL_DIR` | Override the default installation directory |
| `MSVCUP_CACHE_DIR` | Override the default download cache directory |

Default installation directory:
- Windows: `%USERPROFILE%\.msvcup`
- Other platforms: `{XDG_DATA_HOME}/msvcup` or `~/.local/share/msvcup`

## CLI Reference

### `msvcup list`

List all available packages from the Visual Studio manifest.

```sh
msvcup list
```

### `msvcup list-payloads`

List all individual payloads (MSI/VSIX components) with their parent package IDs.

```sh
msvcup list-payloads
```

### `msvcup install`

Download and install packages.

```sh
msvcup install [OPTIONS] --lock-file <PATH> --manifest-update <MODE> <PACKAGES>...
```

| Option | Description |
|--------|-------------|
| `--lock-file <PATH>` | Path to the lock file (created or updated on install) |
| `--manifest-update <MODE>` | Manifest update policy: `off`, `daily`, or `always` |
| `--install-dir <PATH>` | Override the installation directory |
| `--cache-dir <PATH>` | Override the download cache directory |

The `--manifest-update` policy controls how the Visual Studio manifest is refreshed:

- **`off`**: Never fetch the manifest. Only works if a valid lock file already exists.
- **`daily`**: Fetch the manifest at most once per day (checks file modification time).
- **`always`**: Always fetch the latest manifest from Microsoft.

### `msvcup resolve`

Create a shim directory with wrapper executables that install packages on first use. This is useful for integrating msvcup into build systems that expect tools to be available at fixed paths.

```sh
msvcup resolve --config <PATH> --out-dir <PATH> [--manifest-update <MODE>]
```

The output directory will contain:
- Shim executables for compiler tools (`cl.exe`, `link.exe`, `lib.exe`, `rc.exe`, `mt.exe`, etc.)
- A `toolchain.cmake` file for CMake integration
- Copies of `msvcup.toml` and the lock file

### `msvcup fetch`

Download a single package URL to the cache without installing it.

```sh
msvcup fetch <URL> [--cache-dir <PATH>]
```

### Global options

| Option | Description |
|--------|-------------|
| `-v, --verbose` | Enable verbose output (timing, detailed progress) |

## Autoenv (Automatic Environment)

The `msvcup resolve` command creates a shim directory where each tool (e.g., `cl.exe`, `link.exe`) is a small wrapper that:

1. Reads `msvcup.toml` in its own directory to find package info
2. Installs packages if not yet present (via `msvcup-autoenv install`)
3. Sets up environment variables (PATH, INCLUDE, LIB) from the installed packages
4. Forwards execution to the real tool

This means you can add the shim directory to your PATH and use the tools transparently:

```batch
set PATH=C:\project\autoenv;%PATH%
cl.exe hello.c /Fe:hello.exe
```

On first invocation, msvcup downloads and installs the toolchain. Subsequent invocations run in milliseconds.

### CMake integration

The generated `toolchain.cmake` file points CMake at the shim executables:

```sh
cmake -B build -DCMAKE_TOOLCHAIN_FILE=autoenv/toolchain.cmake
cmake --build build
```

## Lock Files

Lock files store the resolved URLs and SHA256 checksums for every package component. This enables fully reproducible builds — the exact same files are downloaded regardless of when or where the install runs.

```sh
# Generate a lock file
msvcup install --lock-file msvcup.lock --manifest-update always msvc-14.44.17.14 sdk-10.0.22621.7

# Install from lock file (no network needed if cached)
msvcup install --lock-file msvcup.lock --manifest-update off msvc-14.44.17.14 sdk-10.0.22621.7
```

Lock files are JSON and should be committed to version control.

## CI/CD Usage

### GitHub Actions

```yaml
- name: Install MSVC toolchain
  run: |
    cargo install msvcup
    msvcup install --lock-file msvcup.lock --manifest-update daily msvc-14.44.17.14 sdk-10.0.22621.7

- name: Build with MSVC
  shell: cmd
  run: |
    call C:\msvcup\msvc-14.44.17.14\vcvars-x64.bat
    call C:\msvcup\sdk-10.0.22621.7\vcvars-x64.bat
    cl.exe /EHsc main.cpp /Fe:main.exe
```

### Using resolve for CMake projects

```yaml
- name: Set up MSVC toolchain
  run: |
    cargo install msvcup
    msvcup resolve --config msvcup.toml --out-dir toolchain --manifest-update daily

- name: Build with CMake
  run: |
    cmake -B build -DCMAKE_TOOLCHAIN_FILE=toolchain/toolchain.cmake
    cmake --build build
```

## Visual Studio Command Prompts

Each package includes a vcvars script for each target architecture:

- `vcvars-x64.bat`
- `vcvars-arm64.bat`
- `vcvars-x86.bat`
- `vcvars-arm.bat`

These scripts set environment variables like a "Visual Studio Command Prompt" would, including `PATH`, `INCLUDE`, and `LIB`.

## Additional Features

- **Download cache**: Packages are cached in `{install_dir}/cache`. Failed installs can be retried without network access.
- **Install metadata**: Every installed file is tracked in `<package>/install/`. This lets msvcup detect file conflicts and query which components installed which files.
- **Concurrent downloads**: Up to 8 packages are downloaded in parallel with progress bars.
- **SHA256 verification**: Every downloaded file is verified against the checksum from the manifest or lock file.

## License

BSD-3-Clause

## Acknowledgements

Special thanks to Martiņš Možeiko (@mmozeiko) for his original [Python MSVC installer](https://gist.github.com/mmozeiko/7f3162ec2988e81e56d5c4e22cde9977), which served as a vital reference for this project.
