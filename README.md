# NAVI

A modular, plugin-based autonomous code agent with a terminal UI. Built in Rust.

## Features

- **TUI** — interactive chat with model picker, thinking mode controls, vim mode, and session history
- **Headless mode** — run scripted tasks via CLI, response printed to stdout
- **20+ LLM providers** — OpenAI, Anthropic, Gemini, xAI, Mistral, DeepSeek, Ollama, LMStudio, and more, all via OpenAI-compatible API
- **Plugin system** — dynamically load `.so`/`.dylib` libraries that register tools, policies, or TUI components
- **Session persistence** — conversations auto-save on quit, reloadable via the Memory modal
- **Credential store** — API keys saved securely to disk (chmod 0600), env vars take precedence

## Quick Start

```bash
cargo build
cargo run -p navi-cli -- "explain this codebase"
```

### Headless mode

```bash
cargo run -p navi-cli -- --no-tui "write a hello world in Rust"
```

### Debug flags

```bash
cargo run -p navi-cli -- --print-config      # dump resolved config as JSON
cargo run -p navi-cli -- --print-providers    # dump provider catalog as JSON
```

## Configuration

Config is TOML, loaded in order (later overrides earlier):

1. Defaults (provider `openai`, model `gpt-5.5`)
2. Global: `~/.config/navi/config.toml`
3. Project: `.navi/config.toml` in the working directory

```toml
[model]
provider = "openai"
name = "gpt-5.5"

[approvals]
allow_reads = true
require_for_writes = true
require_for_commands = true
```

API keys are read from environment variables first, then from the credential store. Set via the TUI model picker when a key is missing.

## Providers

All providers use the OpenAI-compatible API. Two protocol kinds: `openai-responses` and `openai-chat-completions`.

| Provider | Env var | Base URL |
|---|---|---|
| OpenAI | `OPENAI_API_KEY` | `api.openai.com` |
| Anthropic | `ANTHROPIC_API_KEY` | `api.anthropic.com` |
| Gemini | `GEMINI_API_KEY` | `generativelanguage.googleapis.com` |
| xAI | `XAI_API_KEY` | `api.x.ai` |
| Mistral | `MISTRAL_API_KEY` | `api.mistral.ai` |
| Ollama | `OLLAMA_API_KEY` | `localhost:11434` |
| LMStudio | `LMSTUDIO_API_KEY` | `localhost:1234` |

Custom providers can be added in config with any base URL.

## TUI Controls

| Shortcut | Action |
|---|---|
| `ctrl+p` | Command palette |
| `ctrl+m` | Model picker |
| `ctrl+n` | New session |
| `ctrl+c` | Quit |
| `ctrl+enter` | Send message |
| `/` (empty input) | Open command palette |
| `q` (empty input, no messages) | Quit |

## Plugin System

Plugins are native libraries exporting `navi_plugin_entrypoint`. The host validates `api_version` against `NAVI_PLUGIN_API_VERSION` (currently `1`). Register plugins in config:

```toml
[[plugins]]
path = "/path/to/plugin.so"
enabled = true
```

## Workspace

| Crate | Role |
|---|---|
| `navi-cli` | Entry binary, CLI parsing, TUI/headless startup |
| `navi-core` | Agent loop, config, model/tool/session abstractions |
| `navi-openai` | `ModelProvider` impl using OpenAI-compatible HTTP |
| `navi-plugin-api` | Plugin trait + API version constant |
| `navi-plugin-host` | Dynamic library loading via `libloading` |
| `navi-tui` | Terminal UI with ratatui + crossterm |

## Data Locations

| Type | Path |
|---|---|
| Global config | `~/.config/navi/config.toml` |
| Credentials | `<data_dir>/credentials.toml` |
| Sessions | `<data_dir>/sessions/*.json` |

`<data_dir>` is platform-dependent (`directories` crate): `~/.local/share/navi/` on Linux.

## License

MIT
