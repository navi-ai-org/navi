# navi-sdk

[![Crates.io](https://img.shields.io/crates/v/navi-sdk)](https://crates.io/crates/navi-sdk)
[![License](https://img.shields.io/crates/l/navi-sdk)](../LICENSE)

Public embedding facade for [NAVI](https://github.com/navi-ai-org/navi) — the local agentic coding engine.

`navi-sdk` wraps `navi-core` into a stable, UI-agnostic API that can be consumed by the TUI, NAVI Tutor, ACP server, or any Rust application that wants to embed the NAVI agent.

## Why a separate crate?

`navi-core` owns the runtime internals. `navi-sdk` provides a clean boundary:

- **Small, serializable API** — sessions, turns, events, and approvals are all serializable
- **No terminal dependency** — `navi-sdk` does not depend on `ratatui` or `crossterm`
- **Provider setup** — resolves credentials, builds providers, and registers tools
- **MCP & plugins** — loads MCP servers and WASM plugins behind the same `ToolExecutor`

## API surface

```rust
use navi_sdk::{NaviEngine, NaviEngineBuilder, NaviSessionRequest, NaviTurnRequest};

let engine = NaviEngineBuilder::new(std::env::current_dir()?)
    .build()
    .await?;

let session = engine.start_session(NaviSessionRequest::default()).await?;
let response = engine.send_turn(NaviTurnRequest {
    session_id: session.id.clone(),
    content: "Explain this codebase".into(),
    ..Default::default()
}).await?;

println!("{}", response.text);
engine.close_session(&session.id).await?;
```

### Core methods

| Method | Description |
|--------|-------------|
| `start_session` | Create a new agent session |
| `send_turn` | Send a user message, returns structured events |
| `cancel_turn` | Cancel an active turn |
| `resolve_approval` | Resolve tool approval prompts |
| `add_context_packet` | Inject context from external sources |
| `snapshot_session` | Take a persistence snapshot |
| `list_models` / `set_model` | List and select models |
| `subscribe_events` | Stream `RuntimeEvent`s |
| `set_session_skills` | Activate skills for a session |
| `list_mcp_servers` | List configured MCP servers |
| `get_goal` / `set_goal` / `clear_goal` | Thread goals (host API) |
| `update_goal_status` | Host pause/resume/complete/blocked |
| `set_goal_with_short_description` | Goal + compact UI label |

### Events

All state changes flow through serializable events:

`session.started` → `turn.started` → `assistant.delta` → `tool.requested` → `approval.required` → `tool.completed` → `turn.completed` → `session.finished`

## Part of the NAVI workspace

This crate is the recommended dependency for any NAVI frontend or embedding.

**Full project:** <https://github.com/navi-ai-org/navi>
**License:** Apache-2.0
