# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/wolfv/msvcup/releases/tag/v0.1.0) - 2026-02-16

### Other

- Remove dead code, deduplicate, and simplify throughout
- Simplify autoenv wrapper, remove windows-sys dependency, fix formatting
- Upgrade to reqwest 0.13, use rustls by default with configurable TLS
- Convert to async reqwest + parallel fetching with tokio
- Add download progress bars with indicatif
- Fix clippy warnings
- Initial commit: Rust port of msvcup
