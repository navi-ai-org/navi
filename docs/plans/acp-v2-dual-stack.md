# ACP v2 Support Alongside v1 (ADR 0023 Implementation)

Implementation plan in 4 phases for: adopting the official `agent-client-protocol`
crate, fixing payload preservation, adding the v2 surface behind a feature flag,
and projecting `RuntimeEvent` → v2 updates. Scope is **ADR 0023 only** (ADR
0018/0019/0020/0021/0022 are follow-ups; the future app launcher will live in a new
`navi-app` crate).

## Context

- `crates/navi-acp` is a hand-written implementation with `serde` only:
  `PROTOCOL_VERSION = 1` (`src/types.rs:5`), v1 capability shapes
  (`ClientCapabilities.fs/terminal`, `AgentCapabilities.loadSession: bool`),
  hand-written line-delimited JSON-RPC transport (`src/transport.rs`),
  `AcpAgentHandler`/`AcpServer`/`AcpServerHandle` (`src/server.rs:112`),
  `AcpClient` + `AcpConnectOptions` + `PermissionHandler` (`src/client.rs`),
  `AcpEvent` (`src/event.rs`).
- **Payload preservation is broken:** fieldless `#[serde(other)]` on
  `SessionUpdate` (`types.rs:241-243`), `ContentBlock` (`types.rs:124-126`) and
  `StopReason` (`types.rs:160-162`) discards unknown payloads; a session saved by
  a newer engine and replayed by an older one loses data.
- NAVI plays both roles: **agent** (`NaviAcpServer`, `navi-sdk/src/acp_server.rs`,
  713 lines; `navi acp` at `navi-cli/src/main.rs:665-672`) and **client** (turn
  delegation via `AcpClient`/`ExternalAgentPeer`; re-exports at
  `navi-sdk/src/lib.rs:27-31`).
- `process_event` (`acp_server.rs:75-178`) maps `RuntimeEvent` → `SessionUpdate`
  with `message_id: None` on every chunk (no stable IDs).
- `NaviEngine::send_turn` resolves at end of turn (`navi-sdk/src/engine.rs:634`) —
  the v1 contract.
- Official crate: `agent-client-protocol` **2.0.0** (published 2026-07-23, passes
  the AGENTS.md dependency-age rule), own runtime (**async-io/async-process/
  blocking**, not Tokio), `unstable_protocol_v2` feature, separate
  `agent-client-protocol-schema` 1.5.0 crate, Builder/Channel/Role architecture.
- AGENTS.md constraints that apply: single Tokio runtime (CLI owns it), max 4
  test threads, Windows is a CI platform, commits need a minimal changelog.

---

## Phase 1 — Adopt the official crate, v1 with identical behavior

**Goal:** replace the hand-written schema/transport core with the official crate
**without changing v1 wire behavior or `navi-acp`'s public API** (so `navi-sdk`,
`navi-cli`, `navi-napi`, `navi-dart` stay untouched).

1. **Spike (Gate A decision):** add `agent-client-protocol = "2.0.0"` to
   `crates/navi-acp/Cargo.toml`; build a minimal agent via the crate's `Builder`
   against the existing mock-client test. Verify the crate's async-io runtime
   coexists with the workspace's Tokio (no deadlock, no panic-hook conflict)
   under `#[tokio::test]`.
2. **Gate A — formal decision:** full crate adoption **or** `agent-client-protocol-schema`
   only (types) + own transport kept. Criteria: (a) `tokio::test` passes; (b) no
   interference with the CLI's single runtime; (c) the public API can be preserved
   through an adapter layer. Pre-decided fallback (ADR 0023 §2): schema-only, own
   transport — what is not acceptable is maintaining a second hand-written copy of
   a draft schema.
3. Replace `types.rs` with crate types; adapt `client.rs`/`server.rs`/`transport.rs`
   to the Builder/Channel model **or** keep the own transport over schema types
   (per Gate A).
4. Keep compiling the `navi-sdk` re-exports (`lib.rs:27-31`):
   `AcpAgentHandler`, `AcpAgentSpec`, `AcpClient`, `AcpConnectOptions`, `AcpEvent`,
   `AcpProcessConfig`, `AcpServer`, `AcpServerHandle`, `AcpTurnResult`,
   `ExternalAgentPeer`, `PermissionHandler`, `SpawnedAcpPeer` — the adapter lives
   in `navi-acp` so the SDK does not change.

### Files
- `crates/navi-acp/Cargo.toml` — new dependency
- `crates/navi-acp/src/types.rs` — replaced by (or re-exported from) crate types
- `crates/navi-acp/src/{client,server,transport}.rs` — adapted
- `crates/navi-acp/src/lib.rs` — re-exports unchanged in shape

### Tests
- Existing mock lifecycle tests (`navi-acp/src/lib.rs:80-192`) and
  `permission_request_auto_approved` pass **without assertion changes**
- New: crate-runtime/Tokio coexistence test (Gate A evidence)

### Verification
```powershell
$env:PATH="C:\Users\enrel\scoop\persist\rustup\.cargo\bin;$env:PATH"
cargo check -p navi-acp -p navi-sdk -p navi-cli -p navi-napi -p navi-dart
cargo test -p navi-acp -p navi-sdk -- --test-threads=4
```

**Exit:** crate adopted (or schema-only fallback documented), green, v1 wire
identical.

## Phase 2 — Payload preservation

**Goal:** no data loss when saving/replaying unknown variants (ADR 0023 §3).

1. Fix the three enums: `Other` captures the raw payload and re-serializes
   identically (stable round-trip for session replay).
2. v2 discrimination rules: unknown `_`-prefixed value = third-party extension
   (preserve); unknown non-prefixed value = future ACP variant (preserve, **never**
   treat as an extension).
3. If Gate A chose full adoption: verify the official crate already preserves
   (it is the reference implementation); if so this phase becomes tests plus any
   capture needed at the `navi-acp` boundary.

### Files
- `crates/navi-acp/src/types.rs` — `Other` variants capture raw value

### Tests
- Round-trip `serialize → deserialize → serialize` identical for unknown variants
  (both `_x` and `x_future`)
- Unknown notification ignored; unknown request → `-32601`
- Session save/replay with an unknown variant preserves the payload

**Exit:** session save/replay lossless with unknown variants.

## Phase 3 — v2 surface behind a feature flag

**Goal:** ACP v2 working on the wire, **stable baseline only**
(`schema/v2/schema.json`); v1 untouched; negotiated per connection.

1. `acp-v2` feature in `navi-acp/Cargo.toml` (+ `unstable_protocol_v2` of the
   crate, if full adoption).
2. Negotiation: client sends `protocolVersion: 2`; agent replies
   `min(2, its max)`; version fixed per connection after `initialize`; a single
   connection never mixes versions.
3. v2 types/methods (stable baseline): reorganized capabilities (one
   `capabilities` block + required `info` on both sides, session-scoped groups,
   object markers instead of booleans), `auth/login` and `auth/logout`,
   `session/resume` + `replayFrom` (replaces `session/load`), required
   `session/list`/`session/close`/`session/resume`, `session/prompt` ack
   semantics, `state_update` (`running`/`idle`/`requires_action` + stop reasons),
   whole-message upserts (`user_message`/`agent_message`/`agent_thought`),
   `tool_call` removed (first `tool_call_update` creates), `tool_call_content_chunk`,
   `terminal_update`, `terminal_output_chunk`, `plan_update` (planId + `type`
   discriminator), restructured `session/request_permission` (`title` required),
   `$/cancel_request`.
4. `AcpServer` dispatches both versions; `AcpAgentHandler` gains v2 methods with
   defaults (v1 handlers keep compiling).
5. CLI: `navi acp` negotiates automatically (offers 2; if the client answers 1,
   continues as v1).
6. **Explicitly out of this phase:** `schema.v2.unstable` features — MCP-over-ACP
   stays behind its own flag (ADR 0023 §5; ADR 0018 §4 depends on it but 0018 is
   out of scope).

### Files
- `crates/navi-acp/Cargo.toml` — `acp-v2` feature
- `crates/navi-acp/src/types.rs` — v2 schema types
- `crates/navi-acp/src/server.rs` — dual-version dispatch, v2 handler methods
- `crates/navi-acp/src/client.rs` — v2 negotiation
- `crates/navi-sdk/src/acp_server.rs` — v2 `initialize` response shape
- `crates/navi-cli/src/main.rs` — no change expected (negotiation is automatic)

### Tests
- Negotiation matrix v1↔v1, v1↔v2, v2↔v1, v2↔v2 with mock peers
- v2 `initialize` round-trip (capabilities + `_meta`)

**Exit:** v2 on the wire behind the flag; v1 unchanged; negotiation matrix green.

## Phase 4 — `RuntimeEvent` → v2 projection and the prompt lifecycle

**Goal:** a v2 client drives NAVI end to end (turn, tool call, approval,
cancellation, resume).

1. **Stable IDs:** `process_event` (`navi-sdk/src/acp_server.rs:75-178`) emits
   stable `message_id` — `RuntimeEventKind::AssistantDelta`/
   `AssistantThinkingDelta` carry no ID today; assign per turn in `NaviAcpServer`
   (session-scoped counter/state). Tool calls already have `inv.id`; plans need a
   `plan_id` mapping (engine id or session-generated).
2. **Upsert semantics:** on the v2 path, `tool_call_update` with omit/null/value
   patch; buffer deltas per `message_id` and emit a whole-message upsert on
   completion; the `tool_call` variant stays v1-only.
3. **Prompt lifecycle:** the v2 `session/prompt` handler acks immediately (empty
   result) and runs the turn in a spawned task streaming through `handle`;
   completion via `state_update` with stop reason. The v1 path is unchanged
   (ack at end). `NaviEngine::send_turn` keeps resolving at end — the contract
   change is confined to the ACP server layer.
4. **Permission:** v2 `request_permission` (`title` required) — map
   `ApprovalRequest.summary` → `title`, optional `subject`.
5. **Terminal:** `BackgroundCommandSnapshot` → `terminal_update`/
   `terminal_output_chunk` on the v2 path.
6. **Plan:** `plan_update` projection (variants of the `type` discriminator).

### Files
- `crates/navi-sdk/src/acp_server.rs` — `process_event` rewrite, prompt lifecycle,
  id assignment, permission/terminal/plan projections
- `crates/navi-sdk/src/acp_server.rs` or `crates/navi-sdk/src/types.rs` — v2
  update types if the SDK needs them

### Tests
- Full integration: mock v2 client ↔ `NaviAcpServer` over duplex — v2 initialize,
  session/new, prompt (ack + stream + `state_update`), tool-call flow (update
  creates), approval round-trip, cancel mid-turn, session resume with replay
- Phase 1 tests unchanged and still green (`cargo test -p navi-sdk -- --test-threads=4`)

**Exit:** a v2 client drives NAVI end to end; the v1 path stays green.

---

## Risks and gates

- **Gate A (Phase 1)** is the central risk: the crate's async-io runtime vs. the
  workspace's single Tokio runtime. On failure → schema-only (pre-decided fallback).
- **v2 is a draft:** churn during implementation; pin to 2.0.0 and re-evaluate on
  upgrades.
- **Windows:** run everything on this machine (CI is `windows-latest`; the crate
  has `windows-sys`).
- **Coverage:** `navi-acp`/`navi-sdk` are not tool crates (no
  `check-critical-coverage.py` floors), but full `cargo test` of the touched
  crates is mandatory.

## Out of scope (follow-ups, order fixed)

0019 (storage namespacing) → 0020 (app profile config tier + bundle) → 0021
(launcher, new `navi-app` crate) → 0018 (`navi.dev` profile) → 0022 (schema export
+ codegen + conformance; retire `engine_api.rs`).

## Commits

One commit per phase with a minimal changelog body (`type(scope): summary` +
`### Changed`/`### Fixed` sections), per AGENTS.md. Phase 1 deliberately separates
"we changed the schema source" from "we added a protocol version" — never mix.
