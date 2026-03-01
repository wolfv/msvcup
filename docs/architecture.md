# msvcup Architecture

This document describes the internal architecture of msvcup for contributors and anyone who wants to understand how the tool works.

## Overview

msvcup downloads and extracts MSVC toolchain components from the official Visual Studio manifest published by Microsoft. It resolves package dependencies, downloads MSI/VSIX archives, extracts them to versioned directories, and generates environment scripts so the installed tools can be used without Visual Studio.

## Data Flow

```
Microsoft VS Manifest (JSON)
        │
        ▼
┌─────────────────┐
│  manifest.rs    │  Fetch & cache the VS channel manifest
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  packages.rs    │  Parse manifest, identify packages & payloads
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  install.rs     │  Resolve lock file, download & extract payloads
└────────┬────────┘
         │
    ┌────┴────┐
    ▼         ▼
┌────────┐ ┌──────────────┐
│zip_ext │ │ msi_extract  │  Extract VSIX (ZIP) and MSI/CAB archives
└────────┘ └──────────────┘
         │
         ▼
   Versioned install directories with vcvars scripts & env JSON
```

## Module Reference

### Entry Points

| Module | Description |
|--------|-------------|
| `main.rs` | CLI entry point. Parses arguments with clap, dispatches to subcommand handlers. |
| `bin/autoenv.rs` | Windows-only shim binary. When renamed to `cl.exe` etc., it loads environment from installed packages and forwards to the real tool. |

### Core Modules

#### `manifest.rs`
Manages the Visual Studio channel manifest — the JSON file published by Microsoft that lists all available packages and their download URLs.

- **`MsvcupDir`**: Resolves the installation root directory from CLI args, env vars, or platform defaults.
- **`read_vs_manifest()`**: Fetches or reads the cached VS manifest, respecting the update policy (`Off` / `Daily` / `Always`).
- **`read_ch_manifest()`**: Fetches the channel manifest that points to the VS manifest.
- **`fetch()`**: HTTP download with SHA256 verification and progress reporting.
- **`read_file_if_fresh()`**: Returns cached file contents only if modified within the last 24 hours (used by `Daily` update policy).

#### `packages.rs`
Parses the VS manifest JSON into structured package and payload data.

- **`Packages`**: Holds all parsed packages and payloads with index-based cross-references.
- **`MsvcupPackage`**: A user-facing package identifier like `msvc-14.44.17.14`.
- **`MsvcupPackageKind`**: Enum of supported package types (Msvc, Sdk, Msbuild, Diasdk, Ninja, Cmake).
- **`identify_package()`** / **`identify_payload()`**: Classify raw manifest IDs into msvcup types.
- **`get_install_pkg()`**: Determines how a payload should be installed (MSI extract, ZIP extract, etc.).

#### `install.rs`
The main installation pipeline — the largest module.

1. **Lock file resolution**: Checks if the lock file covers all requested packages; if not, resolves URLs from the manifest.
2. **Download phase**: Downloads all payloads concurrently (up to 8 at a time) with progress bars, verifying SHA256 checksums.
3. **Extraction phase**: Extracts MSI and ZIP archives concurrently (parallelism based on CPU cores).
4. **Post-install**: Generates `vcvars-{arch}.bat` and `env-{arch}.json` for each installed package.

Key functions:
- `install_command()` — top-level orchestrator
- `update_lock_file()` — creates or updates the JSON lock file
- `generate_vcvars_bat()` — produces batch scripts setting PATH, INCLUDE, LIB
- `generate_env_json()` — produces JSON files with the same environment info (used by autoenv)

#### `msi_extract.rs`
Extracts files from MSI packages, which contain embedded CAB archives.

- Reads MSI database tables: `Directory`, `Component`, `File`, `Media`
- Resolves the MSI directory tree into filesystem paths
- Extracts individual files from CAB archives within the MSI
- Handles long/short filename pairs, SourceDir resolution, and the `_Streams` table for embedded CABs

#### `zip_extract.rs`
Extracts ZIP and VSIX (which are ZIP files) archives.

- Strips common root directories from ZIP entries
- Handles VSIX-specific path prefixes (e.g., `Contents/`)
- Sanitizes paths to prevent directory traversal attacks
- Decodes percent-encoded filenames
- Tracks extracted files in a manifest for metadata

#### `lockfile_parse.rs`
Parses and validates JSON lock files.

- `parse_lock_file()` — deserializes lock file JSON
- `check_lock_file_pkgs()` — verifies a lock file contains entries for all requested packages
- Validates architectures, SHA256 checksums, and URL patterns

#### `config.rs`
Parses `msvcup.toml` configuration files.

- Validates target architecture, package names, and required fields
- Resolves lock file paths relative to the config file location

#### `resolve_cmd.rs`
Implements the `resolve` subcommand — creates a shim directory with wrapper executables.

1. Resolves packages and generates/updates the lock file
2. Copies msvcup binaries and config into the output directory
3. Creates shim executables by copying `msvcup-autoenv` as `cl.exe`, `link.exe`, etc.
4. Generates `toolchain.cmake`

### Supporting Modules

| Module | Description |
|--------|-------------|
| `arch.rs` | Architecture enum (`X64`, `X86`, `Arm`, `Arm64`) with string parsing and display. |
| `channel_kind.rs` | Release vs Preview channel selection with manifest URL construction. |
| `sha.rs` | SHA256 hashing — both one-shot (`Sha256`) and streaming (`Sha256Streaming`) variants. |
| `util.rs` | Shared helpers: version string scanning, sorted insertion, URL basename extraction, atomic file writing. |
| `extra.rs` | URL parsing for standalone packages (Ninja, CMake) that aren't in MSI format. |
| `lock_file.rs` | File locking abstraction using `fs2` advisory locks. |
| `fetch_cmd.rs` | Simple `fetch` subcommand — downloads a URL to cache. |
| `autoenv_cmd.rs` | Tool name lists (MSVC_TOOLS, SDK_TOOLS) and CMake toolchain file generation. |

## Key Design Decisions

### Versioned directories
Each package is installed to `{install_dir}/{kind}-{version}/` (e.g., `C:\msvcup\msvc-14.44.17.14\`). This allows multiple versions to coexist and makes cleanup trivial — just delete the directory.

### Lock files
Lock files are JSON and store every download URL with its SHA256 checksum. This decouples resolution (which requires the manifest) from installation (which only needs the lock file). Teams commit the lock file to version control so everyone gets identical toolchains.

### Manifest update policies
Three policies balance freshness against speed:
- **`off`** — never fetches; requires a pre-existing lock file
- **`daily`** — caches the manifest for 24 hours (checked via file mtime)
- **`always`** — always fetches the latest manifest

### Concurrent I/O
Downloads use a tokio semaphore to limit concurrent connections (default 8). Extractions run on a thread pool sized to available CPU cores. Progress bars use `indicatif::MultiProgress` to avoid output corruption.

### Autoenv shims
The `msvcup-autoenv` binary serves double duty: as an installer (`msvcup-autoenv install`) and as a tool shim. When copied as `cl.exe`, it determines which tool to forward to by inspecting its own filename. This avoids needing per-tool wrapper scripts and works transparently with build systems.

## Testing

Run the full test suite:

```sh
cargo test
```

The project has 147+ unit tests covering all modules. Tests use real archive formats (CAB, ZIP) where possible rather than mocks.

Run linting:

```sh
cargo clippy --all-targets -- -D warnings
```

Check formatting:

```sh
cargo fmt --check
```
