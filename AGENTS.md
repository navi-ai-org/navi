# Agent Guide for NAVI

NAVI is a modular, plugin-based autonomous code agent implemented in Rust. It has a ratatui/crossterm TUI for interactive sessions and a headless mode for scripted use.

## Read First

Use these docs as the current map before making non-trivial changes:

- `README.md` — user-facing overview, commands, controls.
- `docs/architecture.md` — crate boundaries and runtime flow.
- `docs/tui.md` — TUI state, keybindings, rendering, performance rules.
- `docs/providers.md` — provider protocols, thinking adapters, credentials.
- `docs/tools-security.md` — built-in tools, approvals, security policy.
- `docs/code-agent-guidance.md` — workflow guidance for coding agents.

## Workspace

Rust workspace, edition 2024, resolver 3. All implementation lives under `crates/`; the repo root has no active `src/`.

| Crate | Role |
|---|---|
| `navi-cli` | Entry binary. Parses CLI, loads config, starts TUI or headless runtime. |
| `navi-core` | Harness policy, config, provider catalog, model/tool/session abstractions, security policy, runtime. |
| `navi-openai` | `ModelProvider` implementation for OpenAI-compatible APIs and provider adapters. |
| `navi-plugin-api` | Plugin trait and `NAVI_PLUGIN_API_VERSION = 1`. |
| `navi-plugin-host` | Dynamic `.so`/`.dylib` loading via `libloading`. |
| `navi-tui` | Terminal UI with chat, model picker, thinking/settings/session modals, markdown/code rendering. |

## Commands

```bash
cargo build
cargo fmt
cargo check
cargo test
cargo test -p <crate_name>
cargo run -p navi-cli -- TASK
cargo run -p navi-cli -- --no-tui TASK
cargo run -p navi-cli -- --print-config
cargo run -p navi-cli -- --print-providers
```

Headless mode requires a task argument.

## Configuration

Config is TOML, loaded in this order:

1. Defaults: provider `openai`, model `gpt-5.5`
2. Global config: `~/.config/navi/config.toml` on Linux
3. Project config: `.navi/config.toml`

Key sections:

- `model`: provider id and model name.
- `harness`: `auto`, `small`, or `medium` profile plus tool-loop and observation budgets.
- `approvals`: read/write/command approval behavior.
- `security`: path restrictions, `.git` protection, session redaction, plugin trust, blocked commands.
- `logging`: structured diagnostics, log level, file/stdout logging, debug payload opt-in.
- `providers`: built-in provider overrides or custom providers.
- `plugins`: native plugin library paths.

API keys are read from env vars first, then the credential store. The TUI must not ask for API keys on startup; it prompts from the model picker when selecting a provider without a resolved key.

## Providers

All providers are routed through `navi-openai`. Configured protocol kinds:

- `openai-responses`
- `openai-chat-completions`

Some provider ids have special adapters:

- `anthropic` uses Anthropic Messages streaming.
- `google-gemini` uses Gemini Generate Content streaming.
- `openrouter` adds required OpenRouter headers and reasoning config.
- `openai` and `xai` use Responses-style reasoning effort.

The UI thinking levels are `max`, `high`, `medium`, `low`, and `off`. `ThinkingConfig::adapter_for_provider` maps them to provider-specific request fields.

Tool transcripts must remain provider-correct. Chat Completions uses assistant `tool_calls` plus role `tool` results. Responses uses `function_call` and `function_call_output` input items.

## Tools And Security

Built-in tools:

- `read_file`
- `write_file`
- `apply_patch`
- `list_files`
- `grep`
- `bash`

`ToolExecutor` validates invocations through `SecurityPolicy` before execution. Reads are allowed by default, writes and commands require approval by default, blocked commands are denied, paths are restricted to the project by default, NAVI private storage is denied, and writes to `.git` are denied.

When adding tools, make security-sensitive inputs visible to policy validation. File tools should expose `path` or `file`; command tools should expose `program` or `command`.

Native plugins are loaded with `libloading` from configured `[[plugins]]` entries. They must export `navi_plugin_entrypoint`, match `NAVI_PLUGIN_API_VERSION`, and register executable `Tool` implementations. Bad plugins warn and are skipped; agent policy and TUI component registrations are discovery-only for now.

## TUI Notes

- The TUI event loop is synchronous; async model/tool/provider work uses `tokio::spawn` and reports through `AsyncEvent`.
- Do not create a second Tokio runtime in the TUI.
- The shared harness lives in `navi-core/src/harness.rs`; do not add separate TUI-only prompts or loop policy.
- Ctrl shortcuts: `ctrl+p` commands, `ctrl+m` models, `ctrl+n` new session, `ctrl+s` sessions, `ctrl+o` full tool view, `ctrl+c` quit.
- `ctrl+d` opens the Debug modal with log path, session id, provider/model, active state, and recent diagnostics.
- Prompt sending is `ctrl+enter`; plain `enter` inserts a newline.
- Chat rendering uses cached markdown/code rendering. If rendered output depends on new message fields, update `chat_render_signature`.
- Avoid expensive work in draw functions.

## Persistence

`SessionStore` saves `SessionSnapshot` JSON under `<data_dir>/sessions/`. Secret redaction is enabled by default. If you add event fields containing user/model/tool text, update redaction and session replay logic.

## Logging

NAVI uses `tracing` through `navi-core::logging`. File logs default to `<data_dir>/logs/navi.log` with private permissions on Unix. Logs are diagnostics, not session history. Keep them compact and redacted by default; raw payload logging is only for explicit debug mode. Do not log secrets, Authorization headers, credential-store values, full prompts, or full tool output. Avoid logging from TUI draw paths.

## Testing Expectations

Use focused tests while iterating and broader checks before handoff:

```bash
cargo fmt
cargo check
cargo test
```

For targeted changes:

- TUI/key/rendering: `cargo test -p navi-tui`
- provider/request/stream parsing: `cargo test -p navi-openai`
- tools/security/session/config: `cargo test -p navi-core`

## Gotchas

- The worktree may be dirty; do not revert changes you did not make.
- `target/` is gitignored.
- `test_reqwest.rs` may exist as an untracked local scratch file. Leave it alone unless the user explicitly asks.
- No CI, clippy, or rustfmt config is committed; use default cargo behavior.
