# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1] - 2026-07-09

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.0...v0.2.1

### New Features

- Ship **portable musl Linux binaries** (Alpine toolchain) so one artifact runs on Alpine, Debian, Ubuntu, Amazon Linux, Rocky/RHEL-class images, Fedora, and distroless bases
- Add **xAI Composer 2.5** models to the registry (`composer-2.5`, `grok-composer-2.5-fast`)
- Harden release installers: strict SHA-256 (hard fail), single-file archive validation, optional Sigstore/cosign verification of `SHA256SUMS.txt`
- Sign release checksums keyless with **Sigstore** (GitHub Actions OIDC) as `SHA256SUMS.txt.sigstore.json`

### Bug Fixes

- Make `install.sh` **POSIX / dash-safe** so `curl | sh` works on Ubuntu (dash) and Alpine (ash)
- Reject unsafe or multi-member release archives during install (path traversal / zip-slip style members)
- Disable `--no-verify` on the shell installer (integrity checks are required)

### Documentation

- Document install security controls (HTTPS, checksums, Sigstore, archive shape)
- Document Linux/container portability and a sample Alpine Dockerfile
- Expand user-guide install section for prebuilt binaries and containers

### Chores

- Drop OpenSSL/`native-tls` from the dependency graph (`hf-hub` on rustls only) to enable musl builds
- Build Linux release artifacts inside `rust:1-alpine` containers in CI
- Validate release asset set and checksum lines before publishing

## [0.2.0] - 2026-07-08

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.1.2...v0.2.0

**First public multi-platform release** with prebuilt binaries and a one-line installer.

### New Features

- Tag-triggered multi-platform release pipeline (Linux, macOS, Windows)
- Primary install path via `curl` / PowerShell download of prebuilt binaries
- Plan Mode, goals checklist, multi-provider registry, OAuth (incl. xAI), session cost estimates
- Copland modular TUI; SQLite memory; multimodal attachments

### Bug Fixes

- Registry metadata merge, concurrent SQLite, deferred MCP tools, TUI layout clipping

### Documentation

- README install path, CHANGELOG, user guide updates for the public release

### Chores

- Dependency and registry snapshot updates for the first binary release

## [0.1.2] - 2026-07-04

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.1.0...v0.1.2

### New Features

- Multimodal attachments and `analyze_attachment` host tool
- Registry attachment metadata

### Bug Fixes

- Compact image indicators; registry background tasks without a Tokio runtime

### Documentation

- Release notes for multimodal support

### Chores

- Provider request mapping updates for Gemini/Anthropic media parts

## [0.1.0] - 2026-06-29

Full changelog: https://github.com/navi-ai-org/navi/releases/tag/v0.1.0

### New Features

- Initial open-source scaffold of the NAVI agent engine and TUI

[Unreleased]: https://github.com/navi-ai-org/navi/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/navi-ai-org/navi/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/navi-ai-org/navi/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/navi-ai-org/navi/compare/v0.1.0...v0.1.2
[0.1.0]: https://github.com/navi-ai-org/navi/releases/tag/v0.1.0
