# Architecture

NAVI is a Rust workspace using edition 2024 and resolver 3. The repo root has no `src/`; all implementation lives under `crates/`.

## Crates

| Crate | Responsibility |
|---|---|
| `navi-cli` | CLI entry point. Loads config, dispatches to TUI, headless runtime, or ACP stdio server, prints config/provider diagnostics. |
| `navi-core` | Shared domain layer: config, provider catalog, model abstractions, tool definitions/execution, security policy, sessions, events, runtime. |
| `navi-openai` | `ModelProvider` implementation for OpenAI-compatible APIs and provider adapters. Implementation crate behind `navi-providers` facade. |
| `navi-plugin-api` | Public plugin trait/types and `NAVI_PLUGIN_API_VERSION`. |
| `navi-plugin-host` | Dynamic library loading and API-version validation via `libloading`. |
| `navi-providers` | Provider facade. Re-exports `navi-openai` public API. Downstream crates depend on this, not `navi-openai` directly. |
| `navi-sdk` | Public embedding facade for local clients (Tutor, TUI, ACP). Wraps core runtime, provider setup, plugin loading, host tools, MCP, sessions and events. |
| `navi-tui` | Interactive terminal UI using ratatui and crossterm. Owns chat rendering, modals, key handling, and an async bridge to the shared SDK runtime. |

## Runtime Flow

1. `navi-cli` loads `NaviConfig` from defaults, global config, and project config.
2. The selected provider config is resolved from the built-in catalog plus user overrides.
3. Credentials are resolved from environment variables first, then the credential store.
4. TUI mode creates a `TuiApp`; headless mode creates `AgentRuntime`; ACP mode serves JSON-RPC over stdio. TUI and ACP drive turns through `navi-sdk::NaviEngine`, which owns provider/tool/plugin/MCP setup and wraps `AgentRuntime` sessions.
5. The harness layer selects a `small` or `medium` profile, builds the system prompt, and applies loop/observation limits.
6. A user prompt becomes a `ModelRequest` containing conversation history, selected model, thinking mode, and available tool definitions.
7. `navi-providers` (via `navi-openai`) streams `ModelStreamEvent` values back to the caller.
8. Text/thinking deltas update the active assistant message; tool calls go through `ToolExecutor` and `SecurityPolicy`.
9. Tool calls and tool results are sent back using provider tool-message protocol, not fake user text.
10. Completed assistant output, tool results, and harness traces are persisted as session events.

## ACP Mode

`navi --acp` exposes NAVI as an Agent Client Protocol server over stdin/stdout. It supports initialization, new sessions, prompt turns, prompt cancellation, model/thinking text chunks, tool call updates, and permission requests. The ACP adapter is now a protocol bridge over `navi-sdk::NaviEngine`; it does not construct `SessionRuntime`, providers, tools, plugins, MCP, or security policy directly. It does not run the TUI and must not write diagnostics to stdout.

## Shared SDK Runtime

`navi-sdk::NaviEngine` is the common runtime surface for local clients. TUI, ACP, and NAVI Tutor should use it for session lifecycle, turn submission, cancellation, approval resolution, event streaming, model selection, context packets, host tools, skills, and MCP tools. UI crates may keep display-oriented state such as chat rows, modal selection, retry labels, and loaded-session replay, but core runtime construction belongs behind the SDK/runtime boundary.

## Logging

`navi-core::logging` initializes process-wide `tracing` output from `[logging]` config. File logs default to `<data_dir>/logs/navi.log`; stdout logging is off for TUI by default so it does not corrupt the alternate screen.

Logging is for diagnostics, while `AgentEvent` and `SessionSnapshot` remain user/session history. Instrument lifecycle boundaries such as provider requests, stream errors, retries, approvals, tool execution, plugin loading, and cancellation. Avoid hot draw-path logs unless they are behind debug-level diagnostics.

Log fields must stay compact and redacted. Do not log raw API keys, Authorization headers, credential-store contents, full prompts, or full tool output by default.

## Model Abstractions

`ModelProvider` exposes:

- `stream(ModelRequest) -> ModelStream`
- `complete(ModelRequest) -> ModelResponse`, implemented by consuming the stream
- `list_models()`, optional per provider

`ModelStreamEvent` variants:

- `TextDelta`
- `ThinkingDelta`
- `Status`
- `Usage`
- `ToolCall`
- `Done`

The TUI depends on streaming events. Avoid replacing streaming with a blocking complete call in TUI paths.

## Harness Layer

`navi-core/src/harness.rs` owns the agent harness policy for small and medium models:

- profile selection (`auto`, `small`, `medium`)
- shared system prompt construction
- tool iteration limits
- repeated tool-call detection
- compact tool observations
- request trace summaries

Both TUI and headless mode should use this shared layer. Do not add a second prompt or loop policy in the TUI.

## Conversation State

The TUI keeps two related structures:

- `messages: Vec<ChatMessage>` for visual rendering.
- `conversation_history: Vec<ModelMessage>` for provider requests.

Do not assume every rendered message should be sent back to the model. Tool UI rows and status-only placeholders are visual artifacts. Tool calls should be sent as `ModelMessage::assistant_tool_call`; tool results should be sent as `ModelMessage::tool_result`.

## Sessions

`SessionStore` writes `SessionSnapshot` JSON files under `<data_dir>/sessions/`. Secret redaction is enabled by default through `SecurityConfig.redact_secrets_in_sessions`.

When adding event types:

- Add them to `AgentEvent`.
- Update session load/replay logic in the TUI if the event affects user-visible history.
- Confirm redaction still handles secret-bearing text.

## Plugins

Plugins are native libraries exporting `navi_plugin_entrypoint`. The host loads them with `libloading`, rejects incompatible `api_version` values, and registers executable plugin tools into the same `ToolExecutor` used by built-in tools.

Trusted plugin locations are enforced by `SecurityPolicy` unless `allow_external_plugins = true`. Failed plugins are reported as warnings and skipped. Agent policy and TUI component plugin registrations are discovery-only until those runtime contracts are implemented. Plugin changes should be handled conservatively because native libraries execute arbitrary code.
