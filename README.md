# NAVI

NAVI is an opinionated, extensible code agent with a terminal UI. It is built in Rust and designed around a simple idea: the agent should be customizable like an editor, with provider configuration, tool policies, sessions, plugins, and a TUI that can evolve with the user.

## Current Capabilities

- Interactive TUI with chat, command palette, model picker, thinking controls, settings, session history, and markdown/code rendering.
- Streaming model responses with visible thinking text when providers expose it.
- Shared small/medium model harness with compact observations, loop limits, and provider-correct tool transcripts.
- Tool calling for project work: `read_file`, `write_file`, `apply_patch`, `list_files`, `grep`, and `bash`.
- Compact tool-chain view by default, with `ctrl+o` to expand full tool inputs/outputs.
- Multi-provider catalog using OpenAI-compatible protocols plus provider-specific adapters for OpenAI, Anthropic, Gemini, OpenRouter, Groq, xAI, and other OpenAI-compatible APIs.
- Secure credential store with env-var precedence and per-provider API key prompting from the model picker.
- Security policy for tool invocations, path restrictions, command blocking, approvals, and session secret redaction.
- Native plugin loading through `.so`/`.dylib` libraries with API version validation.
- Headless mode for scripted use.

## Quick Start

```bash
cargo build
cargo run -p navi-cli -- "explain this codebase"
```

Headless mode requires a task argument:

```bash
cargo run -p navi-cli -- --no-tui "write a hello world in Rust"
```

Useful inspection commands:

```bash
cargo run -p navi-cli -- --print-config
cargo run -p navi-cli -- --print-providers
```

## TUI Controls

| Shortcut | Action |
|---|---|
| `ctrl+p` | Command palette |
| `ctrl+m` | Model picker |
| `ctrl+n` | New session |
| `ctrl+s` | Sessions / memory |
| `ctrl+o` | Toggle compact/full tool output view |
| `ctrl+enter` | Send prompt |
| `enter` | Insert newline |
| `ctrl+j` | Insert newline |
| `ctrl+c` | Quit |
| `/` with empty input | Open command palette |
| `?` with empty input | Open shortcuts |

## Configuration

Config is TOML, loaded in this order. Later sources override earlier ones.

1. Defaults: provider `openai`, model `gpt-5.5`
2. Global config: `~/.config/navi/config.toml` on Linux
3. Project config: `.navi/config.toml` in the current working directory

Example:

```toml
[model]
provider = "openai"
name = "gpt-5.5"

[approvals]
allow_reads = true
require_for_writes = true
require_for_commands = true

[security]
restrict_paths_to_project = true
protect_git_metadata = true
redact_secrets_in_sessions = true
allow_external_plugins = false
blocked_commands = ["rm", "rmdir", "shred", "mkfs", "dd", "sudo"]

[harness]
profile = "auto" # auto, small, or medium

[logging]
enabled = true
level = "info"
file_enabled = true
stdout_enabled = false
include_payloads = false
```

API keys are resolved from environment variables first, then from NAVI's credential store. If a selected provider has no key, the TUI asks for it from the model picker instead of blocking startup. The Settings modal also has `Provider Accounts`, which shows configured/unconfigured providers and opens API key setup or OAuth for compatible providers.

## Providers

All configured providers route through the `navi-openai` crate. NAVI supports two protocol kinds:

- `openai-responses`
- `openai-chat-completions`

Some providers need request/stream adapters even when they expose an OpenAI-compatible surface. The thinking selector uses user-facing levels `max`, `high`, `medium`, `low`, and `off`, then maps those to provider-specific request fields.

GitHub Copilot is available as an OAuth-capable provider. The OAuth flow uses GitHub device login, stores the returned bearer token in NAVI's private credential store, and sends Copilot-specific request headers.

See [docs/providers.md](docs/providers.md) for provider behavior and configuration notes.

## Tools And Security

Built-in tools are registered by `ToolExecutor` in `navi-core`:

| Tool | Purpose |
|---|---|
| `read_file` | Read UTF-8 project files, optionally by line range |
| `write_file` | Write full UTF-8 file contents |
| `apply_patch` | Apply a unified diff with `git apply` |
| `list_files` | List project files with optional filtering |
| `grep` | Literal text search over project files |
| `bash` | Run a shell command with timeout and truncation |

The security layer validates tool kind, paths, commands, plugin paths, and approval requirements before execution. See [docs/tools-security.md](docs/tools-security.md).

Native plugins configured under `[[plugins]]` are loaded at startup through `libloading`. A plugin must export `navi_plugin_entrypoint`, return metadata with the current `NAVI_PLUGIN_API_VERSION`, and register executable `Tool` implementations. Invalid plugins are reported as warnings and skipped so NAVI can continue with the remaining tools.

## Logs

NAVI writes compact structured diagnostics to `<data_dir>/logs/navi.log` by default. The log directory is private on Unix (`0700`) and the log file is restricted (`0600`). Logs include provider/tool lifecycle events, retries, cancellations, plugin warnings, and timing/status metadata. Secrets are redacted and raw provider payloads are disabled unless explicitly requested with `--debug-payloads`.

Useful flags:

```bash
navi --print-log-path
navi --log-level debug
navi --no-log-file
navi --debug-payloads --no-tui "inspect the project"
```

In the TUI, `ctrl+d` opens the Debug modal with the current log path, session id, provider/model, active state, and recent diagnostics.

## Workspace

| Crate | Role |
|---|---|
| `navi-cli` | Entry binary, CLI parsing, TUI/headless startup |
| `navi-core` | Config, model/tool/session abstractions, security policy, runtime |
| `navi-openai` | Streaming provider implementation for OpenAI-compatible APIs |
| `navi-plugin-api` | Plugin trait and `NAVI_PLUGIN_API_VERSION` |
| `navi-plugin-host` | Native library loading with `libloading` |
| `navi-tui` | ratatui/crossterm interface |

More implementation guidance lives in:

- [docs/architecture.md](docs/architecture.md)
- [docs/tui.md](docs/tui.md)
- [docs/provider-sync.md](docs/provider-sync.md)
- [docs/code-agent-guidance.md](docs/code-agent-guidance.md)

## Data Locations

| Type | Path |
|---|---|
| Global config | `~/.config/navi/config.toml` on Linux |
| Credentials | `<data_dir>/credentials.toml` |
| Sessions | `<data_dir>/sessions/*.json` |

`<data_dir>` is platform-dependent via the `directories` crate. On Linux it is typically under `~/.local/share/navi/`.

## Verification

```bash
cargo fmt
cargo check
cargo test
cargo test -p navi-tui
```

Use focused crate tests while iterating, then run `cargo test` before handing off broader behavior changes.

## License

MIT
