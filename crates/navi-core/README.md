# navi-core

[![Crates.io](https://img.shields.io/crates/v/navi-core)](https://crates.io/crates/navi-core)
[![License](https://img.shields.io/crates/l/navi-core)](../LICENSE)

The engine behind [NAVI](https://github.com/navi-ai-org/navi) — a local, agentic coding agent.
`navi-core` contains the runtime, security policy, tool executor, model abstractions, session management, and configuration system that power every NAVI frontend.

## What's inside

| Module | Purpose |
|--------|---------|
| `runtime` | Agent loop, turn execution, tool dispatch, and harness policy |
| `security` | `SecurityPolicy` with permission modes (Restricted / AcceptEdits / Yolo), per-tool allow/ask/deny rules, path validation, and command blocking |
| `config` | `NaviConfig` TOML schema, defaults, provider catalog, and harness profiles |
| `tool` | `ToolExecutor`, built-in tools (`read_file`, `write_file`, `bash`, `grep`, …), and the `Tool` trait |
| `session` | `SessionStore` persistence with secret redaction |
| `session_replay` | Rebuild provider history from events; rehydrate `view_image` attachments |
| `attachment_store` | Durable content-addressed blobs under `{data_dir}/attachments/` |
| `compact` | Three-level conversation compaction: micro, auto, and session memory |
| `credentials` | Credential store, environment resolution, and provider key lookup |
| `event` | `AgentEvent` / `RuntimeEvent` types for streaming agent state |
| `model` | `ModelProvider` trait, request/response types, and thinking config |
| `harness` | Harness profile selection (`small` / `medium`) with observation budgets |
| `registry` | Provider registry with embedded snapshot, SQLite cache, and remote sync |
| `skills` | SQLite skill store, built-ins, and prompt injection |
| `memory` | Project-scoped session memory for cross-session context |

## Quick look

```rust
use navi_core::{NaviConfig, SecurityPolicy, ToolExecutor, PermissionMode};

let config = NaviConfig::default();
let security = config.effective_security_config();
assert_eq!(security.permission_mode, PermissionMode::Restricted);

let policy = SecurityPolicy::new(
    std::env::current_dir()?,
    dirs::data_dir().unwrap().join("navi"),
    security,
)?;
let executor = ToolExecutor::new(policy);
```

## Feature flags

| Flag | Default | Description |
|------|---------|-------------|
| *(none yet)* | — | All features are enabled by default |

## Part of the NAVI workspace

This crate is not meant to be used standalone. It is consumed by [`navi-sdk`](https://crates.io/crates/navi-sdk), [`navi-tui`](https://crates.io/crates/navi-tui), and other NAVI crates.

**Full project:** <https://github.com/navi-ai-org/navi>
**License:** Apache-2.0
