# Agent Guide for NAVI

NAVI is the local agentic engine and terminal-first code agent. It is implemented in Rust, has a ratatui/crossterm TUI for interactive sessions, supports headless/scripted execution, and should expose engine capabilities to other local clients.

NAVI Tutor is a separate visual active-learning workspace that may use NAVI as its engine. Treat NAVI Tutor as an important client/case of use, not as the NAVI product boundary.

## Product Boundary

NAVI owns:

- agent runtime
- TUI/CLI interaction
- ACP server
- model providers and provider auth
- built-in tools and host/plugin tool execution
- tool safety and approvals
- session execution
- token/context tracking
- plugin runtime
- event emission
- project/code operations

NAVI does not own:

- visual study canvas
- mind maps
- drawing UI
- study blocks UI
- skill map UI
- tutor dashboard
- learning product UX
- Tauri frontend layout
- educational workflow design, except as engine capabilities exposed to clients

## Architecture Principle

The TUI is not the engine.

```txt
NAVI Engine = runtime, tools, providers, sessions, events, approvals
NAVI TUI    = terminal frontend for technical builders
NAVI Tutor  = visual learning frontend using the same engine
```

Core agent behavior belongs in engine crates, primarily `navi-core`, not in `navi-tui`. The TUI should be a powerful frontend/client of the engine. NAVI Tutor must be able to embed or drive the engine without depending on terminal UI internals.

NAVI should expose three stable surfaces:

1. CLI/TUI for humans operating the agent directly in a terminal.
2. Runtime SDK / Rust API for NAVI Tutor/Tauri and other local apps embedding NAVI.
3. ACP stdio server for editors and external agent clients.

Do not make WebSocket/daemon the primary interface unless explicitly requested. External process mode should prefer a stable stdio/headless runtime protocol first.

Default integration direction: NAVI Tutor embeds NAVI Engine directly. Advanced mode may use an external NAVI installation through a stable headless/stdio protocol. NAVI Tutor must not require NAVI TUI to be installed.

## Engine API Direction

NAVI should evolve toward a small, serializable, UI-agnostic runtime API equivalent to:

- `NaviRuntime::start_session(...)`
- `NaviRuntime::send_turn(...)`
- `NaviRuntime::cancel_turn(...)`
- `NaviRuntime::list_models(...)`
- `NaviRuntime::set_model(...)`
- `NaviRuntime::register_host_tool(...)`
- `NaviRuntime::add_context_packet(...)`
- `NaviRuntime::stream_events(...)`
- `NaviRuntime::snapshot_session(...)`

Structured events should be serializable, versioned, and suitable for both TUI and Tutor. Expected event concepts include `session.started`, `turn.started`, `assistant.delta`, `assistant.thinking_delta`, `tool.requested`, `approval.required`, `tool.started`, `tool.completed`, `context.updated`, `tokens.updated`, `session.saved`, `turn.completed`, `session.finished`, and `error`.

External clients must be able to provide context packets from sources such as files, project state, user selection, canvas nodes, study blocks, focus threads, material excerpts, session summaries, decisions, and memory search. NAVI should accept, prioritize, and inject these packets without owning the client's UI or database.

Agent modes must become real runtime state, not only slash-command text. Examples include `Plan`, `Edit`, `Review`, `Tutor`, `Socratic`, `Recall`, and `Focus`. Modes may control system prompt, allowed tools, mutation permissions, output style, approval policy, and whether NAVI answers directly or asks Socratic questions.

Host apps must be able to register tools without dynamic native plugins. For NAVI Tutor, host tools may include `create_canvas_node`, `update_study_block`, `link_nodes`, `add_not_now_item`, `create_quiz`, `record_answer`, `update_skill_score`, `search_memory`, and `register_decision`.

## Plugin Boundary

Plugins must have scope:

- Engine plugins: providers, tools, context processors, routing, memory/session hooks, approval policies. Usable by TUI and Tutor.
- TUI plugins: terminal UI widgets, ratatui panels, keybindings, terminal commands, themes. Usable only by NAVI TUI.
- Tutor plugins: visual blocks, canvas tools, study behaviors, tutor widgets. Usable only by NAVI Tutor.

NAVI Tutor can inherit engine sessions, providers, runtime plugins, tools, and events. NAVI Tutor cannot inherit TUI-specific plugins because there is no terminal UI to modify.

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
| `navi-sdk` | Public embedding facade for local clients such as NAVI Tutor. Wraps core runtime, provider setup, plugin loading, host tools, sessions and events. |
| `navi-tui` | Terminal UI with chat, model picker, thinking/settings/session modals, markdown/code rendering. |

## Current Integration State

Latest important commits:

- `d7f0f46 Add NAVI SDK integration surface` in this repo.
- `3efb238 Connect Tutor backend to NAVI SDK` in `/home/enrell/projects/navi-tutor`.

`navi-sdk` exists locally and is not published to crates.io. NAVI Tutor currently consumes it by path dependency:

```toml
navi-sdk = { path = "../../navi/crates/navi-sdk" }
navi-core = { path = "../../navi/crates/navi-core" }
```

The SDK is the intended public Rust boundary for Tutor integration. Do not make Tutor depend on `navi-tui`. Do not assume the `navi` binary exists in `PATH`.

Implemented SDK/runtime capabilities:

- `NaviEngineBuilder::from_project(...)`
- `NaviEngine::start_session(...)`
- `NaviEngine::send_turn(...)`
- `NaviEngine::cancel_turn(...)`
- `NaviEngine::resolve_approval(...)`
- `NaviEngine::add_context_packet(...)`
- `NaviEngine::snapshot_session(...)`
- `NaviEngine::list_models(...)`
- `NaviEngine::set_model(...)`
- `NaviEngine::subscribe_events(...)`
- host tools through `SdkHostTool` and `HostToolHandler`

`navi-core` now has embeddable runtime state for session lifecycle, cancellation, approval resolution, event streaming and snapshots. Approval and cancellation have lightweight handles so a client can resolve approval or cancel while a turn is active.

Next integration target:

- Build the Tutor frontend loop on top of existing Tauri commands: start a NAVI session, send a turn, listen to `navi-runtime-event`, render assistant deltas, show approval prompts, and verify host tools mutate Tutor SQLite/canvas state.

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

API keys are read from env vars first, provider-specific external auth sources next, then the credential store. The TUI must not ask for API keys on startup; it prompts from the model picker when selecting a provider without a resolved key. Provider account management belongs in the command palette as `Providers`, not in Settings.

## Providers

All providers are routed through `navi-openai`. Configured protocol kinds:

- `openai-responses`
- `openai-chat-completions`

Some provider ids have special adapters:

- `anthropic` uses Anthropic Messages streaming.
- `google-gemini` uses Gemini Generate Content streaming.
- `openrouter` adds required OpenRouter headers and reasoning config.
- `github-copilot` uses GitHub device OAuth bearer tokens and Copilot request headers.
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
- `tab` cycles the active agent mode in the TUI through `navi_core::AgentMode`; runtime APIs should treat this as real engine state, not only a slash-command prefix.
- `ctrl+d` opens the Debug modal with log path, session id, provider/model, active state, and recent diagnostics.
- Prompt sending is `ctrl+enter`; plain `enter` inserts a newline.
- Chat rendering uses cached markdown/code rendering. If rendered output depends on new message fields, update `chat_render_signature`.
- Avoid expensive work in draw functions.

## KISS Rules

- Do not couple NAVI Tutor to NAVI TUI.
- Do not make NAVI Tutor depend on terminal UI internals.
- Do not force WebSocket/daemon before needed.
- Do not make plugin scope ambiguous.
- Do not put core runtime logic inside `navi-tui`.
- Keep engine APIs small, serializable, and stable.
- Keep TUI as a powerful frontend, not the product boundary.

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
