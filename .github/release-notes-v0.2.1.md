## New Features

- **Portable musl Linux binaries** — built on Alpine so the same artifact runs on Alpine, Debian, Ubuntu, Amazon Linux, Rocky/RHEL-class images, Fedora, and distroless containers
- **xAI Composer 2.5** in the model registry (`composer-2.5`, `grok-composer-2.5-fast`)
- **Hardened installers** — required SHA-256 match against `SHA256SUMS.txt`, single-file archive checks (no path traversal), optional Sigstore verification when `cosign` is installed
- **Sigstore-signed checksums** — `SHA256SUMS.txt.sigstore.json` produced keyless via GitHub Actions OIDC

## Bug Fixes

- `install.sh` works with **dash/ash** (`curl | sh` on Ubuntu and Alpine) — no longer requires bash-only features
- Installer rejects multi-member or unsafe archive paths
- Integrity verification can no longer be skipped with `--no-verify`

## Documentation

- Install security table (HTTPS, checksums, Sigstore, archive layout)
- Container / Linux portability notes and sample Alpine Dockerfile
- User guide updates for prebuilt install and containers

## Chores

- Remove OpenSSL/`native-tls` (route `hf-hub` through rustls) for musl builds
- Release CI: Linux builds run inside `rust:1-alpine`
- Stricter release asset + checksum validation before publish

## Changelog

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.0...v0.2.1

### Install

```bash
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh -s -- --version 0.2.1
```

```powershell
# Windows
$env:NAVI_VERSION = "0.2.1"
irm https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.ps1 | iex
```
