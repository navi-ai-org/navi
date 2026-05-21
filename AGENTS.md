# Agent Guide for NAVI

NAVI is a modular, plugin-based autonomous code agent implemented in Rust. It has a TUI (ratatui + crossterm) for interactive sessions and a headless mode for scripted use.

## Workspace

Rust workspace, edition 2024, resolver 3. Six crates under `crates/`:

| Crate | Role |
|---|---|
| `navi-cli` | Entry binary (`src/main.rs`). Parses CLI, starts TUI or headless runtime |
| `navi-core` | Agent loop, config, model/tool/session abstractions |
| `navi-openai` | `ModelProvider` impl using OpenAI-compatible HTTP |
| `navi-plugin-api` | Plugin trait + `NAVI_PLUGIN_API_VERSION = 1` |
| `navi-plugin-host` | Dynamic `.so`/`.dylib` loading via `libloading` |
| `navi-tui` | Terminal UI with chat, model picker, vim mode |

## Commands

```
cargo build                     # build workspace
cargo test                      # run all unit tests
cargo test -p <crate_name>      # run one crate's tests
cargo run -p navi-cli -- TASK   # run TUI with pre-filled task
cargo run -p navi-cli -- --no-tui TASK   # headless, prints response to stdout
cargo run -p navi-cli -- --print-config  # dump resolved config as JSON
cargo run -p navi-cli -- --print-providers  # dump provider catalog as JSON
```

Headless mode **requires** a task argument; it will bail otherwise.

## Configuration

Config is TOML, loaded in this order (later overrides earlier):
1. Defaults (provider `openai`, model `gpt-5.5`)
2. Global: `~/.config/navi/config.toml` (platform-dependent via `directories` crate)
3. Project: `.navi/config.toml` in the working directory

Key config sections: `model` (provider + name), `approvals` (require_for_writes, require_for_commands), `providers` (override built-ins), `plugins` (list of `.so` paths).

All providers use the OpenAI-compatible API. Two kinds: `openai-responses` and `openai-chat-completions`. API keys are read from env vars first, then from the credential store (persisted in the data dir).

## Plugin System

Plugins are native libraries exporting `navi_plugin_entrypoint`. The host validates `api_version` against `NAVI_PLUGIN_API_VERSION` (currently `1`). Plugins register tools, agent policies, or TUI components via `PluginRegistry`.

## Key Abstractions

- **`ModelProvider`** — trait for LLM backends. `navi-openai` is the only impl; all providers (Anthropic, Gemini, etc.) route through it via different base URLs.
- **`Tool`** — trait with `definition()` and `invoke()`. `ToolKind`: `Read`, `Write`, `Command`, `Custom`.
- **`AgentRuntime`** — holds config + provider, exposes `submit_task()`. Always uses `ThinkingConfig::High` in headless mode.
- **`SessionStore`** — saves `SessionSnapshot` as JSON to `<data_dir>/sessions/`.

## TUI Notes

- The TUI event loop is synchronous; `tokio::spawn` is used for async model calls (CLI already owns the runtime — do NOT create a second one).
- Vim mode is opt-in (toggle via command palette). When enabled, input starts in Normal mode.
- Ctrl shortcuts: `ctrl+p` commands, `ctrl+m` model picker, `ctrl+n` new session, `ctrl+c` quit.

## Gotchas

- `src/` at repo root is empty; all code lives under `crates/`.
- No CI, clippy, or rustfmt config committed. Use default `cargo` behavior.
- The `target/` directory is gitignored.
