# @navi-agent/navi

**Install the [NAVI](https://github.com/navi-ai-org/navi) coding agent CLI via npm.**

This package provides the prebuilt `navi` binary for your platform. No Rust toolchain needed.

## Install

```bash
npm install -g @navi-agent/navi
```

## Usage

```bash
# Open the interactive TUI
navi

# Run a task headlessly
navi --no-tui "explain this codebase"
```

## How it works

This is a thin wrapper that installs the correct prebuilt binary for your platform via optionalDependencies:

- `@navi-agent/navi-linux-x64`
- `@navi-agent/navi-linux-arm64`
- `@navi-agent/navi-darwin-x64`
- `@navi-agent/navi-darwin-arm64`
- `@navi-agent/navi-win32-x64`

If no prebuilt binary is available for your platform, the postinstall script will attempt to download one from GitHub Releases.

## Alternative install methods

```bash
# Shell installer (primary — no Node required)
curl -fsSL https://github.com/navi-ai-org/navi/raw/refs/heads/main/scripts/install.sh | sh

# From source (development)
cargo install --git https://github.com/navi-ai-org/navi navi-cli
```
## License

Apache-2.0
