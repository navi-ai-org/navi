# navi-cli

[![Crates.io](https://img.shields.io/crates/v/navi-cli)](https://crates.io/crates/navi-cli)
[![License](https://img.shields.io/crates/l/navi-cli)](../LICENSE)

The `navi` binary — the command-line entry point for [NAVI](https://github.com/navi-ai-org/navi), a local agentic coding agent.

## What it does

`navi-cli` parses CLI arguments, loads configuration, and launches one of three modes:

| Mode | When | Description |
|------|------|-------------|
| **TUI** | `navi` (no flags) | Interactive terminal UI with chat, model picker, and tool approval |
| **Headless** | `navi --no-tui "task"` | Runs a single task non-interactively and exits |
| **ACP** | editors / external clients | JSON-RPC over stdio for agent client protocol |

## Usage

```bash
# Interactive TUI
navi

# Headless task
navi --no-tui "refactor the auth module"

# Show resolved config
navi --print-config

# List provider catalog
navi --print-providers

# Subcommands
navi plugin install <id>    # Install a WASM plugin
navi registry sync           # Force-sync provider registry
navi registry list           # List cached providers
navi mcp list                # List configured MCP servers
```

## Configuration

NAVI loads config from three sources in order:

1. **Defaults** — provider `openai`, model `gpt-5.5`
2. **Global** — `~/.config/navi/config.toml`
3. **Project** — `.navi/config.toml`

See the [main README](https://github.com/navi-ai-org/navi#configuration) for the full config schema.

## Part of the NAVI workspace

This binary depends on [`navi-sdk`](https://crates.io/crates/navi-sdk) and [`navi-tui`](https://crates.io/crates/navi-tui).

**Full project:** <https://github.com/navi-ai-org/navi>
**License:** Apache-2.0
