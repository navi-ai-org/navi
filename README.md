# NAVI

NAVI is an opinionated, extensible code agent with a terminal UI. It is built in Rust and designed around a simple idea: the agent should be customizable like an editor, with provider configuration, tool policies, sessions, plugins, and a TUI that can evolve with the user.

## Current Capabilities

- Interactive TUI with chat, command palette, model picker, thinking controls, settings, session history, and markdown/code rendering.
- Streaming model responses with visible thinking text when providers expose it.
- Shared small/medium model harness with compact observations, loop limits, and provider-correct tool transcripts.
- Agent modes: Plan, Edit, Review, Tutor, Socratic, Recall, Focus. Each mode controls system prompt, allowed tools, mutation permissions, output style, and approval policy. Cycle with `tab` in the TUI.
- Specialized bash-redundancy tools that replace ~1,500 redundant bash calls per session with structured output: `test_runner`, `build_runner`, `fs_browser`, `git_ops`, `package_manager`.
- General project tools: `read_file`, `write_file`, `apply_patch`, `grep`, and `bash`.
- Compact tool-chain view by default, with `ctrl+o` to expand full tool inputs/outputs.
- Multi-provider catalog using OpenAI-compatible protocols plus provider-specific adapters for OpenAI, Anthropic, Gemini, OpenRouter, Groq, xAI, GitHub Copilot (OAuth), Gitlawb, and other OpenAI-compatible APIs.
- Secure credential store with env-var precedence and per-provider API key prompting from the model picker.
- Security policy for tool invocations, path restrictions, command blocking, approvals, and session secret redaction.
- Native plugin loading through `.so`/`.dylib` libraries with API version validation.
- MCP client support: configured stdio MCP servers are started by `navi-sdk`, their tools registered with prefixed names (e.g. `memory__search`).
- ACP stdio server mode for editor/client integration.
- Headless mode for scripted use.
- Session memory, auto-compaction, and three-level context management (micro, auto, session memory).

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

ACP server mode is for editors and clients that speak the Agent Client Protocol:

```bash
navi --acp
```

`--acp` uses stdout/stdin for JSON-RPC, so diagnostics stay in the log file and the flag cannot be combined with `--no-tui` or a task argument.

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

[skills]
enabled = false
dirs = [".navi/skills"]
active = []

[mcp]
enabled = false

[[mcp.servers]]
id = "memory"
command = "memory-mcp-server"
args = []
enabled = false
```

API keys are resolved from environment variables first, then from NAVI's credential store. If a selected provider has no key, the TUI asks for it from the model picker instead of blocking startup. Provider account management lives in the command palette as `Providers`, which shows configured/unconfigured providers and opens API key setup or OAuth for compatible providers.

## Providers

All configured providers route through the `navi-openai` crate. NAVI supports two protocol kinds:

- `openai-responses`
- `openai-chat-completions`

Some providers need request/stream adapters even when they expose an OpenAI-compatible surface. The thinking selector uses user-facing levels `max`, `high`, `medium`, `low`, and `off`, then maps those to provider-specific request fields.

GitHub Copilot is available as an OAuth-capable provider. The OAuth flow uses GitHub device login, stores the returned bearer token in NAVI's private credential store, and sends Copilot-specific request headers.

Gitlawb provides an OpenAI-compatible gateway at `https://opengateway.gitlawb.com/v1` with models like `mimo-v2.5-pro`. Uses `openai-chat-completions` protocol with `Authorization: Bearer` auth.

See [docs/providers.md](docs/providers.md) for provider behavior and configuration notes.

## Tools And Security

Built-in tools are registered by `ToolExecutor` in `navi-core`:

| Tool | Kind | Purpose |
|---|---|---|
| `read_file` | Read | Read UTF-8 project files, optionally by line range |
| `write_file` | Write | Write full UTF-8 file contents |
| `apply_patch` | Write | Apply a unified diff with `git apply` |
| `grep` | Read | Literal text search over project files |
| `bash` | Command | Run a shell command with timeout, background tasks, and truncation |
| `test_runner` | Command | Run project tests with structured output. Auto-detects cargo/jest/vitest/bun/pytest/go |
| `build_runner` | Command | Build/compile with caching. Returns structured warnings/errors. Skips rebuild if no source changed |
| `fs_browser` | Read | Browse filesystem: `list`, `tree`, `find`, `stat`. Replaces `list_files` |
| `git_ops` | Command | Git operations: `status`, `diff`, `log`, `branch`, `stash`, `remote`. Read-only commands bypass approval |
| `package_manager` | Write | Manage deps: `install`, `add`, `remove`, `update`, `check`. Auto-detects npm/bun/cargo/go |

The security layer validates tool kind, paths, commands, plugin paths, and approval requirements before execution. See [docs/tools-security.md](docs/tools-security.md).

`git_ops` read-only commands (`status`, `diff`, `log`, `branch`) bypass approval automatically through a special-case in `SecurityPolicy`. Destructive commands (`stash`, `remote`) require approval.

Native plugins configured under `[[plugins]]` are loaded at startup through `libloading`. A plugin must export `navi_plugin_entrypoint`, return metadata with the current `NAVI_PLUGIN_API_VERSION`, and register executable `Tool` implementations. Invalid plugins are reported as warnings and skipped so NAVI can continue with the remaining tools.

Skills are local folders containing `SKILL.md`. When `[skills].enabled = true`, NAVI discovers configured skill directories and injects active skills into the model prompt. Skills do not execute scripts or install remote content in the initial implementation.

MCP support is client-side through the shared SDK runtime used by Tutor, TUI, and ACP. Configured stdio MCP servers under `[[mcp.servers]]` are started by `navi-sdk`, their tools are registered with prefixed names like `memory__search`, and tool execution follows the same approval flow as other custom tools.

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
| `navi-mcp` | MCP stdio client integration that maps remote MCP tools into NAVI tools |
| `navi-openai` | Streaming provider implementation for OpenAI-compatible APIs |
| `navi-plugin-api` | Plugin trait and `NAVI_PLUGIN_API_VERSION` |
| `navi-plugin-host` | Native library loading with `libloading` |
| `navi-providers` | Provider facade. Re-exports `navi-openai` public API |
| `navi-sdk` | Public embedding facade for local clients (Tutor, TUI, ACP). Wraps core runtime |
| `navi-tui` | ratatui/crossterm interface |

## SDK Embedding

`navi-sdk` is the intended Rust boundary for embedding NAVI in other applications (e.g. NAVI Tutor). It provides:

- `NaviEngineBuilder::from_project(path)` — loads config, providers, plugins, MCP
- `NaviEngine::start_session(...)` — creates a new agent session
- `NaviEngine::send_turn(...)` — sends a user message, returns structured events
- `NaviEngine::cancel_turn(...)` — cancels an active turn
- `NaviEngine::resolve_approval(...)` — resolves tool approval prompts
- `NaviEngine::subscribe_events(...)` — streams `RuntimeEvent`s (assistant deltas, tool lifecycle, etc.)
- `NaviEngine::snapshot_session(...)` — takes a persistence snapshot
- Host tools through `SdkHostTool` — register custom tools from the host app

NAVI Tutor consumes `navi-sdk` by path dependency. See [docs/architecture.md](docs/architecture.md).

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
