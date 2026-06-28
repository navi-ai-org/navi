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
use std::sync::Arc;
use async_trait::async_trait;
use navi_core::ToolKind;
use navi_sdk::{
    HostToolDefinition, HostToolHandler, HostToolInvocation, SdkHostTool,
    SdkHostToolResult,
};
use serde_json::json;

struct MyTool;

#[async_trait]
impl HostToolHandler for MyTool {
    async fn invoke(&self, invocation: HostToolInvocation) -> anyhow::Result<SdkHostToolResult> {
        // Return structured JSON result
        Ok(SdkHostToolResult::success(json!({
            "id": invocation.invocation_id,
            "status": "ok",
            "data": "..."
        })))
    }
}

let host_tool = SdkHostTool::new(
    HostToolDefinition {
        name: "my_custom_tool".into(),
        description: "Custom host app capability".into(),
        kind: ToolKind::Read,
        input_schema: json!({ "type": "object" }),
    },
    Arc::new(MyTool),
);

// Register during engine build
let engine = NaviEngineBuilder::from_project(".")
    .host_tool(Arc::new(host_tool))
    .build()?;
```

Host tools go through the same `ToolExecutor` and `SecurityPolicy` as built-in
tools. They receive the tool invocation id and model-produced JSON input.

## TypeScript / NAPI

The `navi-napi` crate exposes the SDK to Node clients without native plugin
libraries. A host can build the learning runtime and register TypeScript tools
before starting a session:

```ts
import { NaviNapiEngineBuilder } from "@navi/napi";

const builder = new NaviNapiEngineBuilder(process.cwd());
builder.configureLearning({
  maxConsecutiveErrors: 7,
  stopOnRepeatedTool: false,
  compactObservationMaxBytes: 4096,
  role: "professor",
  style: "socratico",
  language: "pt-BR",
  keepAllAssessments: true,
  exemptToolNames: ["questionario", "grill_avaliacao", "student_progress"],
});
builder.hostTool(
  {
    name: "consultar_materiais",
    description: "Consulta materiais didaticos do aluno",
    kind: "read",
    inputSchema: {
      type: "object",
      properties: {
        topic: { type: "string" },
      },
      required: ["topic"],
    },
  },
  async ({ invocationId, input }) => {
    const material = await materialDb.lookup(input.topic);
    return {
      ok: true,
      output: { invocationId, material },
    };
  },
);

const engine = builder.build();
const models = engine.listModels();
const session = await engine.startSession();
const events = engine.subscribeEvents(session.id);
await engine.addContextPacket(session.id, {
  source: "StudyBlock",
  title: "Limites",
  content: "O aluno ja conhece derivadas.",
  priority: 5,
});
const response = await engine.sendTurn(session.id, "Explique limites com exemplos");
const firstEvent = await events.next();
```

The callback receives `{ invocationId, input }` and returns a promise for
`{ ok, output }`. The tool is registered through the same SDK `SdkHostTool`
adapter used by Rust hosts, so it is visible to the model without changing
`navi-core` or depending on `navi-tui`. `subscribeEvents(sessionId)` returns a
stream object whose `next()` method resolves to the next serialized
`RuntimeEvent`, or `null` when the stream closes. `configureLearning(...)`
maps structured TypeScript options onto the Rust learning harness, tutor prompt
builder, and study compaction strategy. The NAPI engine also exposes runtime
control methods for `cancelTurn`, `resolveApproval`, `addContextPacket`,
`listModels`, and `setModel`.

## Native Plugin Policies

Native plugins can call `register_agent_policy(name)`. The SDK consumes known
policy names before constructing the session runtime:

| Policy | Effect |
|---|---|
| `learning_tutor`, `navi_learning`, `tutor` | Uses `learning_runtime_components()`. |
| `default`, `code_agent` | Uses `RuntimeComponents::default()`. |

Unknown policy names are reported as plugin warnings and do not replace the
host-configured runtime components. `register_tui_component(...)` remains a
TUI-scoped declaration; `navi-core` and `navi-sdk` do not instantiate terminal
widgets.

## Runtime Customization

`NaviEngineBuilder` can replace runtime components while keeping the default
code-agent behavior available through `RuntimeComponents::default()`:

```rust
use std::sync::Arc;
use navi_core::{PermissiveSecurityPolicy, RuntimeComponents};
use navi_sdk::NaviEngineBuilder;

let engine = NaviEngineBuilder::from_project(".")
    .security(Arc::new(PermissiveSecurityPolicy))
    .host_tool(Arc::new(host_tool))
    .build()?;
```

For a tutor-style learning runtime:

```rust
let engine = NaviEngineBuilder::from_project(".")
    .learning_tutor()
    .host_tool(Arc::new(material_lookup_tool))
    .host_tool(Arc::new(quiz_tool))
    .build()?;
```

Custom components can replace:

| Component | Purpose |
|---|---|
| `ToolSecurityPolicy` | Tool approval/deny behavior. |
| `HarnessDriver` | Tool filtering, loop-stop decisions, and tool-result observations. |
| `PromptBuilder` | System prompt construction. |
| `CompactionStrategy` | Micro-compact and auto-compact behavior. |
| `SessionHooks` | Session, turn, and tool lifecycle callbacks. |

The default composition preserves NAVI's terminal code-agent behavior. A host
that wants full tool autonomy, such as NAVI Tutor, should opt in explicitly with
`PermissiveSecurityPolicy`; permissive security is never the default.

`learning_tutor()` composes `PermissiveSecurityPolicy`, `LearningHarness`,
`TutorPromptBuilder`, and `StudyCompactionStrategy`. It is intended as the Rust
SDK preset that NAPI/TypeScript wrappers can expose to `navi-learning`.

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
