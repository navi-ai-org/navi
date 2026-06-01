# NAVI SDK Agents Guide

This document covers the technical API for embedding NAVI in other applications using `navi-sdk`. It is intended for developers building clients that drive NAVI programmatically (e.g. NAVI Tutor, custom UIs, automation).

## Overview

`navi-sdk` is the stable Rust embedding facade. It wraps core runtime, provider setup, plugin loading, host tools, MCP, sessions, and events into a single entry point: `NaviEngine`.

```
Your App (Tauri, CLI, etc.)
    |
    v
navi-sdk (NaviEngine)
    |
    +-- navi-core (runtime, tools, security, sessions)
    +-- navi-providers (navi-openai facade)
    +-- navi-plugin-host (native plugin loading)
    +-- navi-mcp (MCP client)
```

## Engine Construction

```rust
use navi_sdk::NaviEngineBuilder;

// From a project directory (loads config, providers, plugins, MCP)
let engine = NaviEngineBuilder::from_project("/path/to/project")?.build()?;

// With explicit config
let engine = NaviEngineBuilder::with_config(config).build()?;
```

`from_project` loads `NaviConfig` from defaults, global config, and project config. The selected provider is resolved from the catalog plus user overrides. Credentials are resolved from environment variables first, then the credential store.

## Session Lifecycle

```rust
// Start a session
let session = engine.start_session(None).await?; // None = auto-generate ID
// or
let session = engine.start_session(Some("my-session-id".into())).await?;

// Send a turn (user message -> events)
let events = engine.send_turn("explain this codebase").await?;

// Cancel an active turn
engine.cancel_turn().await?;

// Take a persistence snapshot
engine.snapshot_session().await?;

// Close a session
engine.close_session().await?;
```

## Runtime Events

`send_turn` returns a stream of `RuntimeEvent` values. Each event is serializable and UI-agnostic:

| Event | Payload | Description |
|---|---|---|
| `session.started` | session id | New session initialized. |
| `turn.started` | turn id | User message received, turn processing begins. |
| `assistant.delta` | text chunk | Streaming assistant text. |
| `assistant.thinking_delta` | text chunk | Streaming thinking text (when visible). |
| `tool.requested` | tool name, call id, input | Tool call pending approval. |
| `approval.required` | call id, tool name, input | Approval prompt for write/command tools. |
| `tool.started` | call id, tool name | Tool execution started. |
| `tool.completed` | call id, output, success | Tool execution finished. |
| `context.updated` | packets | Context injection changed. |
| `tokens.updated` | usage | Token counts updated. |
| `session.saved` | snapshot path | Session persisted. |
| `turn.completed` | turn id | Turn finished. |
| `session.finished` | session id | Session ended. |
| `error` | message | Error occurred. |

Subscribe to events for streaming:

```rust
let mut events = engine.subscribe_events().await?;

while let Some(event) = events.recv().await {
    match event {
        RuntimeEvent::AssistantDelta { text, .. } => { /* render text */ }
        RuntimeEvent::ToolRequested { name, input, .. } => { /* show tool call */ }
        RuntimeEvent::ApprovalRequired { call_id, .. } => {
            engine.resolve_approval(&call_id, true).await?;
        }
        // ...
    }
}
```

## Approval Flow

Write and command tools require approval by default. When `approval.required` fires:

```rust
// Approve
engine.resolve_approval(&call_id, true).await?;

// Deny
engine.resolve_approval(&call_id, false).await?;
```

In headless/autonomous mode, approvals are gated by default. Configure `[approvals]` to auto-allow specific tool kinds if needed.

## Model Management

```rust
// List available models
let models = engine.list_models().await?;

// Set the active model
engine.set_model("openai", "gpt-5.5").await?;

// List provider accounts and credential status
let accounts = engine.list_provider_accounts().await?;
```

## Skills

```rust
// Set active skills for the session
engine.set_session_skills(vec!["planner".into(), "reviewer".into()]).await?;
```

Skills must be configured in `[skills]` and discovered from disk. `set_session_skills` activates them for the current session's system prompt.

## MCP Servers

```rust
// List configured MCP servers
let servers = engine.list_mcp_servers().await?;
```

MCP servers are started by `navi-sdk` on engine build. Their tools are registered with prefixed names (e.g. `memory__search`) and follow the same approval flow as built-in tools. MCP servers can only be configured in global config, not project config.

## Host Tools

Applications embedding NAVI can register custom tools:

```rust
use navi_sdk::host_tool::{SdkHostTool, HostToolHandler};
use navi_core::tool::Invocation;

struct MyTool;

impl HostToolHandler for MyTool {
    fn name(&self) -> &str { "my_custom_tool" }

    fn invoke(&self, invocation: &Invocation) -> anyhow::serde_json::Value {
        // Return structured JSON result
        serde_json::json!({ "status": "ok", "data": "..." })
    }
}

let host_tool = SdkHostTool::new(MyTool);
// Register during engine build
let engine = NaviEngineBuilder::with_config(config)
    .with_host_tool(host_tool)
    .build()?;
```

Host tools go through the same `ToolExecutor` and `SecurityPolicy` as built-in tools. They receive invocation metadata including `project_root` and `session_id`.

## Context Injection

Inject external context into the active session:

```rust
engine.add_context_packet(ContextPacket {
    source: "external".into(),
    content: "Relevant context for the agent...".into(),
    priority: ContextPriority::High,
}).await?;
```

Context packets are included in the next turn's model request.

## Error Handling

All engine methods return `Result<T, SdkError>`. `SdkError` variants:

| Variant | Description |
|---|---|
| `MissingCredential` | Provider has no resolved API key. |
| `UnknownProvider` | Requested provider id not in catalog. |
| `SessionNotFound` | Session id does not exist. |
| `TurnInProgress` | Cannot start a new turn while one is active. |
| `Internal` | Underlying runtime error. |

`SdkError` implements `std::error::Error` and can be downcast from `anyhow::Error`.

## Configuration for Embedding

When embedding NAVI, you typically:

1. Set `NaviConfig` programmatically or load from a known path.
2. Configure the provider with explicit `api_key_env` and `base_url` if not using defaults.
3. Register host tools for application-specific capabilities.
4. Set `SecurityConfig` to match your application's trust model.
5. Disable TUI-specific features (modals, keybindings) — these are not part of the SDK.

## Crate Dependencies

Depend on `navi-sdk` by path:

```toml
[dependencies]
navi-sdk = { path = "../navi-sdk" }
```

`navi-sdk` re-exports the necessary types from `navi-core` for constructing configs, handling events, and registering tools. Do not depend on `navi-core` or `navi-tui` directly unless you need internals not exposed by the SDK.

## Testing

Run SDK-specific tests:

```bash
cargo test -p navi-sdk
```

Run the full suite with resource limits:

```bash
CARGO_TEST_THREADS=4 cargo test
```

## See Also

- [AGENTS.md](../AGENTS.md) — Full technical reference for the NAVI codebase.
- [User Guide](user-guide.md) — Configuration, providers, tools, and security from the user's perspective.
