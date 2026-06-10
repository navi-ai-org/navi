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
use navi_sdk::{NaviEngineBuilder, NaviEngine};

// From a project directory (loads config, providers, plugins, MCP)
let engine = NaviEngineBuilder::from_project(".")
    .build()
    .expect("engine");

// With explicit config
let engine = NaviEngineBuilder::from_project(".")
    .loaded_config(loaded_config)
    .build()
    .expect("engine");

// With host tools
let engine = NaviEngineBuilder::from_project(".")
    .host_tool(Arc::new(MyTool))
    .build()
    .expect("engine");
```

`from_project` returns a builder (not `Result`). Config is loaded from defaults, global config, and project config on `build()`. The selected provider is resolved from the catalog plus user overrides. Credentials are resolved from environment variables first, then the credential store.

## Session Lifecycle

Session-scoped operations require a `session_id` parameter:

```rust
use navi_sdk::NaviSessionRequest;

// Start a session
let info = engine.start_session(NaviSessionRequest {
    project_dir: None,       // defaults to engine's project
    session_id: None,        // auto-generate
    context_packets: vec![],
    active_skills: vec![],
    initial_messages: vec![],
}).await?;
let session_id = info.id;

// Send a turn (user message -> response)
let response = engine.send_turn(NaviTurnRequest {
    session_id: session_id.clone(),
    message: "explain this codebase".into(),
    context_packets: vec![],
}).await?;

// Cancel an active turn
engine.cancel_turn(&session_id).await?;

// Take a persistence snapshot
let snapshot = engine.snapshot_session(&session_id).await?;

// Close a session
engine.close_session(&session_id).await?;
```

## Runtime Events

Runtime events are delivered via `subscribe_events` as a `broadcast::Receiver<RuntimeEvent>`. Each event has `version` and `kind` fields:

```rust
use navi_core::RuntimeEvent;

// Subscribe to events for a session (synchronous, returns broadcast::Receiver)
let mut events = engine.subscribe_events(&session_id)?;

// In an async task, receive events
while let Ok(event) = events.recv().await {
    match event {
        RuntimeEvent::AssistantDelta { text, .. } => { /* render text */ }
        RuntimeEvent::ToolRequested { name, input, .. } => { /* show tool call */ }
        RuntimeEvent::ApprovalRequired { call_id, .. } => {
            engine.resolve_approval(&session_id, ApprovalDecision::Allow).await?;
        }
        // ...
    }
}
```

Events are serializable and UI-agnostic. Common event kinds include `session.started`, `turn.started`, `assistant.delta`, `assistant.thinking_delta`, `tool.requested`, `approval.required`, `tool.started`, `tool.completed`, `context.updated`, `tokens.updated`, `session.saved`, `turn.completed`, `session.finished`, and `error`.

## Approval Flow

Write and command tools require approval by default. When `approval.required` fires:

```rust
use navi_core::ApprovalDecision;

// Approve
engine.resolve_approval(&session_id, ApprovalDecision::Allow).await?;

// Deny
engine.resolve_approval(&session_id, ApprovalDecision::Deny).await?;
```

In headless/autonomous mode, approvals are gated by default. Configure `[approvals]` to auto-allow specific tool kinds if needed.

## Model Management

```rust
// List available models (synchronous)
let models = engine.list_models();

// Select a model (persists config change)
let result = engine.select_model(NaviModelSelectionRequest {
    provider_id: "openai".into(),
    model: "gpt-5.5".into(),
    save_target: NaviConfigSaveTarget::Auto,
})?;

// List provider accounts and credential status
let accounts = engine.list_provider_accounts()?;

// Set the active model for a session
engine.set_model(&session_id, "openai", "gpt-5.5").await?;
```

## Skills

```rust
// List discovered skills
let skills = engine.list_skills()?;

// Set active skills for a session
engine.set_session_skills(&session_id, vec!["planner".into(), "reviewer".into()]).await?;
```

Skills must be configured in `[skills]` and discovered from disk. `set_session_skills` activates them for the current session's system prompt.

## MCP Servers

```rust
// List MCP servers for a session
let servers = engine.list_mcp_servers(&session_id)?;

// List MCP tool names for a session
let tools = engine.list_mcp_tools(&session_id)?;
```

MCP servers are started by `navi-sdk` on engine build. Their tools are registered with prefixed names (e.g. `memory__search`) and follow the same approval flow as built-in tools. MCP servers can only be configured in global config, not project config.

## Host Tools

Applications embedding NAVI can register custom tools:

```rust
use navi_sdk::host_tool::{SdkHostTool, HostToolHandler};
use navi_core::tool::Invocation;
use async_trait::async_trait;

struct MyTool;

#[async_trait]
impl HostToolHandler for MyTool {
    fn name(&self) -> &str { "my_custom_tool" }

    async fn invoke(&self, invocation: &Invocation) -> anyhow::Result<serde_json::Value> {
        // Return structured JSON result
        Ok(serde_json::json!({ "status": "ok", "data": "..." }))
    }
}

let host_tool = SdkHostTool::new(MyTool);
// Register during engine build
let engine = NaviEngineBuilder::from_project(".")
    .host_tool(Arc::new(host_tool))
    .build()?;
```

Host tools go through the same `ToolExecutor` and `SecurityPolicy` as built-in tools. They receive invocation metadata including `project_root` and `session_id`.

## Context Injection

Inject external context into an active session:

```rust
use navi_core::ContextPacket;

engine.add_context_packet(&session_id, ContextPacket {
    source: "external".into(),
    content: "Relevant context for the agent...".into(),
    priority: ContextPriority::High,
}).await?;
```

Context packets are included in the next turn's model request.

## Error Handling

All engine methods return `Result<T, NaviError>`. `NaviError` variants:

| Variant | Description |
|---|---|
| `MissingCredential` | Provider has no resolved API key. |
| `UnknownProvider` | Requested provider id not in catalog. |
| `SessionNotFound` | Session id does not exist. |
| `TurnInProgress` | Cannot start a new turn while one is active. |
| `Config` | Configuration error. |
| `Internal` | Underlying runtime error. |

`NaviError` implements `std::error::Error` and can be downcast from `anyhow::Error`.

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
just test-crate navi-sdk
```

Run the full suite with resource limits:

```bash
just test
```

## See Also

- [AGENTS.md](../AGENTS.md) — Full technical reference for the NAVI codebase.
- [User Guide](user-guide.md) — Configuration, providers, tools, and security from the user's perspective.
