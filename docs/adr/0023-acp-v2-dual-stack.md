# ADR 0023 — ACP v2 Support Alongside v1

## Status
Proposed

## Context

ADR 0018 makes the first-party application boundary an ACP extension profile and
relies on rules — `_meta` on all types, `_`-prefixed methods, `_`-prefixed enum
variants, capability advertisement — that are **v2** rules. It listed "migrate
`navi-acp` to v2" as a blocking prerequisite without specifying it. This ADR does,
and corrects the framing: the requirement is to **add** v2, not to migrate to it.

### Current state

`crates/navi-acp` is a hand-written implementation with `serde` only — no
`agent-client-protocol` dependency. It pins `PROTOCOL_VERSION: u32 = 1`
(`types.rs:5`) and uses the v1 capability shape (`AgentCapabilities.loadSession:
bool`, `ClientCapabilities.fs/terminal`).

The crate doc describes it as a client ("spawns external ACP agent servers over
JSON-RPC stdio and delegates full turns"), but NAVI also acts as an **Agent** via
`NaviAcpServer` and the `navi acp` subcommand (`navi-cli/src/main.rs:665`). Both
roles are in scope; only `navi-sdk` depends on the crate.

### What v2 actually is

v2 is a consolidation release, and upstream is explicit about two things that
determine this ADR:

> "Migrating does not mean dropping v1. v1-only Agents and Clients will remain
> common for some time, so implementers should support both versions side by side:
> negotiate the version per connection."

> "The v2 protocol surface as a whole is still labeled draft, so gate v2 support
> behind explicit version negotiation and feature flags until it stabilizes."

So a hard cut to v2 would be wrong on both counts: it would cut NAVI off from
existing v1 peers, and it would bind a shipped product to a draft.

There is also a stable/unstable split: `schema/v2/schema.json` is the stable
baseline, `schema/v2/schema.unstable.json` layers opt-in draft features.
Negotiating `protocolVersion: 2` implies **none** of the unstable features.

### The change that matters most

The v1→v2 breaking changes are extensive, but one is architecturally invasive:

> "The `session/prompt` response no longer ends the turn. It acknowledges
> acceptance. Foreground progress and completion arrive as `state_update`
> notifications, and the stop reason moved there too."

NAVI's own model matches v1 here: `NaviEngine::send_turn` resolves when the
assistant finishes. Under v2, the Agent side must acknowledge immediately and
report completion out-of-band, and the Client side must stop treating the prompt
response as terminal. This is not a serde change; it is a control-flow change on
both roles.

The second-order one: **updates are upserts patched by ID** — omitted field means
unchanged, `null` means cleared, a value means replaced, chunks append. NAVI emits
event-shaped `RuntimeEvent`s that are largely append-only. Projecting them to v2
requires stable identity per message, tool call and plan, which NAVI does not
uniformly have today.

## Decision

### 1. Dual-stack, negotiated per connection

NAVI supports v1 and v2 simultaneously. The negotiated version is fixed for the
lifetime of a connection after `initialize`; a single connection never mixes
versions. v1 support is not deprecated by this ADR and is not scheduled for removal.

This supersedes ADR 0018 §3's "migrate to v2" framing.

### 2. Adopt the official `agent-client-protocol` crate

Replace the hand-written schema in `navi-acp` with the upstream crate, which
provides both the `Agent` and `Client` traits NAVI needs and powers Zed's
integration.

Rationale:
- v2 is a **moving draft**; hand-tracking it is a recurring cost with no ceiling.
- v2's semantics are the kind that fail *silently* when implemented approximately:
  upsert patch rules, the redesigned prompt lifecycle, and tool calls created by
  their first `tool_call_update`. Divergence here produces wrong UI state, not errors.
- ADR 0022 makes conformance first-class; upstream types are the reference
  implementation of the thing being conformed to.
- ADR 0018 principle 4 is "upstream first". Owning a redundant schema contradicts it.

**Fallback trigger, decided in advance:** if the crate's runtime or transport model
conflicts with NAVI's constraint that the CLI owns the only Tokio runtime, adopt it
for **types and schema only** and keep NAVI's own transport. What is not acceptable
is maintaining a second hand-written copy of a draft schema.

### 3. Fix payload preservation

v2 requires that unknown values and raw tagged-union payloads be **preserved** when
storing, replaying, proxying or forwarding. NAVI persists and replays sessions, so
this is load-bearing, not theoretical.

`SessionUpdate`, `ContentBlock` and `StopReason` currently use `#[serde(other)]`,
whose fieldless `Other` variant **discards** the payload (`navi-acp/src/types.rs:241`).
A session recorded by a newer engine and replayed by an older one loses data
silently. Unknown variants must capture the raw value.

The v2 discrimination rule must also be honoured: `_`-prefixed unknown values are
foreign extensions; non-prefixed unknown values are **reserved future ACP** and must
not be treated as extensions.

### 4. Breaking-change map

| v1 → v2 | NAVI impact |
|---|---|
| `session/prompt` acknowledges; completion via `state_update` | Largest change. Agent side decouples ack from completion; Client side stops treating the response as terminal (§Context) |
| Updates are upserts by ID | Requires stable IDs per message / tool call / plan; `RuntimeEvent` projection is rewritten, not remapped |
| `tool_call` variant removed; first `tool_call_update` creates the call | Tool lifecycle events project to update-only |
| `plan` → `plan_update` with `planId` + `type` discriminator | NAVI's plan mode is a markdown document (`plan_store`); needs an id and to fit the discriminated variants |
| `session/load` removed → `session/resume` + `replayFrom` | Maps onto `start_session_from_snapshot` |
| `session/list`, `session/close`, `session/resume` now **required** | Already available (`list_saved_sessions`, `close_session`, snapshot reopen) |
| `fs/*` and `terminal/*` **removed** | Client-side tools now come from client-provided MCP servers — the mechanism ADR 0018 depends on |
| `session/set_mode` removed → `session/set_config_option` | `AgentMode` (plan/default) and `PermissionMode` project onto config options |
| Capabilities reorganized: single `capabilities`, required `info`, session-scoped groups nested, object markers not booleans | `ClientCapabilities` / `AgentCapabilities` rewritten |
| `authenticate` → `auth/login`; `logout` → `auth/logout`, required when `authMethods` is non-empty | Maps onto device OAuth and API-key credential paths |
| New `terminal_update` / `terminal_output_chunk` for Agent-owned display terminals | Projects NAVI's `BackgroundCommandSnapshot`; replaces a custom surface with a standard one |
| `messageId` required on chunk variants | NAVI must emit stable message ids |
| Enums and tagged unions accept unknown values; `_`-prefixed reserved for implementations | This is what enables ADR 0018 |

### 5. Gating and rollout

- v2 behind a cargo feature and runtime negotiation until upstream marks it stable.
- Only the **stable** v2 baseline (`schema/v2/schema.json`) is implemented.
  `schema.unstable.json` features are each gated behind their own flag and
  capability — including MCP-over-ACP (ADR 0018 §4).
- Order: (1) adopt the crate for v1 with behaviour unchanged and green tests;
  (2) add the v2 surface behind the flag; (3) fix payload preservation, which
  applies to both versions; (4) only then build ADR 0018's extension profile on top.

Step 1 exists to separate "changed our schema source" from "added a protocol
version". Doing both at once makes any regression ambiguous.

### 6. Conformance

The ADR 0022 corpus covers both surfaces, with the v1/v2 negotiation outcome as an
explicit case. ADR 0018's app profile requires v2 and must refuse to activate over a
v1 connection rather than partially degrade.

## Consequences

Positive:
- Unblocks ADR 0018 without cutting off v1 peers
- Upstream owns the schema; NAVI stops maintaining a redundant copy of a moving draft
- `fs/*`/`terminal/*` removal plus client-provided MCP servers is exactly the
  mechanism ADR 0018 needs, so v2 makes that ADR smaller
- Payload-preservation fix removes a real data-loss path in session replay
- `terminal_update` replaces a bespoke background-command surface with a standard one
- Adopting the crate gives both `Agent` and `Client` roles from one dependency

Negative:
- **Two protocol surfaces to implement, test and support indefinitely.** v1 is not
  scheduled for removal, so this is permanent cost, not transitional
- The prompt-lifecycle change is invasive: `send_turn`'s "resolve when finished"
  contract does not survive contact with v2, and that contract is visible in
  `navi-sdk` and every binding
- Upsert projection needs stable identity for messages, tool calls and plans that
  NAVI does not uniformly emit today — this is engine work, not protocol work
- Targeting a draft means churn: stable-baseline changes will land during
  implementation
- A new workspace dependency where there were none; its async/runtime opinions may
  conflict with the single-runtime rule (§2 fallback)
- Adopting upstream types removes the freedom to be deliberately lenient where the
  spec is strict — which has been convenient and will occasionally hurt
- Per `AGENTS.md`, the dependency version must be ≥7 days published, which may lag
  the newest v2 schema revision

## Alternatives rejected

| Alternative | Why rejected |
|---|---|
| Hard-cut to v2 | Cuts off v1-only peers, and binds a shipped product to a draft; upstream explicitly advises against it |
| Stay on v1 | ADR 0018 depends on v2 extensibility rules; without them there is no app boundary |
| Keep the hand-written schema and add v2 by hand | Doubles a hand-maintained copy of a moving draft; upsert and lifecycle semantics fail silently when approximated |
| Adopt the crate and drop `navi-acp` entirely | NAVI still needs its own projection between `RuntimeEvent` and ACP updates; the crate supplies schema and traits, not that mapping |
| Implement the unstable v2 schema now | Negotiating v2 implies none of it; shipping draft-on-draft compounds churn |
| Implement v2 and defer the payload-preservation fix | It is a data-loss bug on the v1 path too, and it gets harder once two surfaces exist |

## References
- [ACP v2 — Migrating from v1](https://agentclientprotocol.com/protocol/v2/migration) — dual-version guidance, draft status, method and variant tables
- [ACP v2 — Extensibility](https://agentclientprotocol.com/protocol/v2/extensibility)
- [`agent-client-protocol` crate](https://crates.io/crates/agent-client-protocol) — `Agent` and `Client` traits
- ADR 0018 — App boundary; §3 framing corrected by §1 here
- ADR 0022 — Conformance corpus covering both surfaces
- `crates/navi-acp/src/types.rs:5,241` — v1 pin and the payload-discarding `#[serde(other)]`
- `crates/navi-cli/src/main.rs:665` — `navi acp`, NAVI in the Agent role
- `AGENTS.md` — single Tokio runtime rule (§2 fallback trigger), dependency age policy
