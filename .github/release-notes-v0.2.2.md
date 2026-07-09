## New Features

- **`navi-lite`** — sealed, mission-scoped headless runtime for edge/embedded Linux prototypes (no TUI, MCP, plugins, embeddings, or registry sync)
- Prebuilt **`navi-lite`** binaries for Linux (musl), macOS, and Windows
- **Portable musl Linux** full `navi` binaries for Alpine, Debian, Ubuntu, Amazon Linux, Rocky, Fedora, and distroless containers
- **xAI Composer 2.5** models (`composer-2.5`, `grok-composer-2.5-fast`)
- Hardened installers (required SHA-256, single-file archives, optional Sigstore)
- Sigstore-signed `SHA256SUMS.txt` via GitHub Actions OIDC

## Bug Fixes

- `install.sh` works with dash/ash (`curl | sh` on Ubuntu and Alpine)
- Installer rejects multi-member / path-traversal archives
- Linux arm64 musl builds via Docker Alpine on arm runners
- macOS packaging without bash-only `mapfile`

## Documentation

- `navi-lite` README: mission allowlist, security model, demo usage
- Install security and container portability notes
- Sample Alpine Dockerfile for agent sidecars

## Chores

- Feature-gate heavy `navi-core` deps; lite builds with `default-features = false`
- Drop OpenSSL/`native-tls` for musl-friendly builds
- CI builds and smoke-tests `navi-lite` next to `navi`

## Changelog

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.0...v0.2.2

### Install (`navi`)

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh -s -- --version 0.2.2
```

### `navi-lite` assets

Download `navi-lite-<platform>.tar.gz` / `.zip` from this release, extract `navi-lite`, and put it on your `PATH`.

```bash
# example: Linux x64
curl -fsSL -O https://github.com/navi-ai-org/navi/releases/download/v0.2.2/navi-lite-linux-x64.tar.gz
tar -xzf navi-lite-linux-x64.tar.gz
./navi-lite --help
```
