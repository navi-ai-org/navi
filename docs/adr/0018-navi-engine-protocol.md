# ADR 0018 — NAVI App Boundary as an ACP Extension Profile

## Status
Proposed

## Context

### The problem

NAVI is an agent engine. The product goal is that **NAVI is a runtime**: a user
installs NAVI once, and any number of first-party applications
(`organize-anything`, `NAVI Learning`, Tutor, future apps) drive that installed
engine. An application supplies its own harness, its own tool selection, its own
domain tools, and its own UI. It must never require rebuilding NAVI, and it must
not disturb the config, sessions, or memory of the user's own `navi` TUI.

Today there is no such boundary. Every client re-marshals the same engine surface
by hand:

| Surface | Lines | Mechanism |
|---------|-------|-----------|
| `navi-napi/src/lib.rs` | 2 654 | 181 hand-written `#[napi]` methods |
| `navi-dart/src/{engine,surface}.rs` | 3 069 | hand-written C ABI |
| `navi-server/src/routes/` | ~2 000 | hand-written warp routes |
| `navi-sdk/src/engine_api.rs` | 391 | **two hand-maintained string arrays** compared by a test |

That is ~8 000 lines of marshalling for one logical API, and `AGENTS.md`
non-negotiable #2 codifies the cost: every new engine capability must be wired
through `core → sdk → napi → cli → tui`. The parity arrays in `engine_api.rs`
exist only to detect that a human forgot a step.

Replicating that per client language (TypeScript, Ruby, …) reproduces the same
defect at higher multiplicity: N languages × M methods, hand-maintained, with no
compiler to catch drift.

### Requirements

| # | Requirement |
|---|-------------|
| R1 | Drive agent turns and receive streamed assistant output (app → engine) |
| R2 | App declares **arbitrary named tools** with JSON Schema; engine invokes them (engine → app) |
| R3 | Engine requests interaction: tool approval, structured question, plan review, secret |
| R4 | R1–R3 on **one** connection with one lifecycle and one failure mode |
| R5 | App profile negotiated at connect: harness, tool selection, prompt, storage namespace |
| R6 | Local (desktop sidecar) **and** remote multi-tenant (SaaS) transport |
| R7 | Tolerate version skew in both directions (installed engine older/newer than the app) |
| R8 | Multiple client languages generated from one schema |
| R9 | No engine rebuild to add an application |

R2 and R4 are load-bearing: an application's domain logic ("scan mods", "score an
exercise") is the application's product and lives in the application process, next
to the UI state it depends on.

### Why not a new protocol

An earlier revision of this ADR proposed **NEP**, a new JSON-RPC protocol, on the
grounds that ACP ∪ MCP covers the requirements but neither alone does — ACP drives
an agent but its client capabilities are a closed, editor-shaped set
(`fs/*`, `terminal/*`), while MCP has the right tool-declaration shape but no way
to run a turn.

That reasoning was correct about the *gap* and wrong about the *remedy*. ACP v2
specifies a formal extension mechanism that closes the gap without a new protocol:

| Mechanism | Rule |
|---|---|
| `_meta` field | present on **all** types — requests, responses, notifications, content blocks, tool calls, **and capability objects** |
| Extension methods | any method name prefixed `_` is reserved for implementations; standard JSON-RPC request/notification semantics |
| Extension enum variants | `_`-prefixed values reserved for implementations in tagged unions with a fallback path |
| Capability advertisement | `_meta` inside capability objects is the sanctioned way to negotiate extensions |
| Hard constraint | *"Implementations MUST NOT add any custom fields at the root of a type that's part of the specification. All possible names are reserved for future protocol versions."* |

Decisively, **NAVI owns both ends** of this boundary: the application is the ACP
Client, NAVI is the ACP Agent. An extension needs only those two to agree. A
standard ACP client (e.g. Zed) sees `_meta` it ignores and never calls `_navi.dev/*`,
so base interoperability is preserved by construction rather than by discipline.

A new protocol would therefore buy nothing and cost: a third protocol in a space
that already has two, no ecosystem, and full ownership of spec, SDKs, conformance
and docs for a single maintainer.

### Prior art, restated

| | R1 | R2 | R3 | R4 | R5 | R6 | R7 | R8 | R9 |
|---|---|---|---|---|---|---|---|---|---|
| ACP **v1** base | yes | no⁵ | partial | yes | no | yes¹ | yes | yes | yes |
| ACP **v2** base | yes | **yes**⁶ | yes | yes | no | yes¹ | yes | yes | yes |
| ACP v2 **+ `navi.dev` profile** | yes | yes | yes | yes | **yes** | yes | yes | yes | yes |
| MCP | no² | yes | partial | no³ | no | yes | yes | yes | yes |
| REST + SSE | yes | no⁴ | no | no | n/a | yes | manual | yes | yes |
| gRPC bidi | yes | yes | yes | yes | n/a | yes | manual | yes | yes |

¹ stdio is ACP's default transport, not a schema constraint; the same messages are
valid over WebSocket. The earlier revision scored this as a failure — that was an error.
² MCP is not an agent-driving protocol; there is no `session/prompt` equivalent.
³ Roles are inverted: MCP servers provide, hosts consume.
⁴ No server→client request path, which R3 and R2 both require.
⁵ v1's client capabilities are the closed set `fs/*` + `terminal/*`.
⁶ v2 removed `fs/*` and `terminal/*` and directs implementers to client-provided MCP
servers for client-side tools. **R2 is therefore a base-protocol feature in v2, not a
NAVI extension** — the single largest correction to the earlier revision of this ADR.

The net effect: the `navi.dev` profile shrinks to R5 (app profile) plus, at most, one
secret-handling method. Everything else is base ACP v2.

gRPC meets the matrix but is rejected on operability: binary framing for a
localhost pipe is hostile to debug, browsers need a proxy, and the toolchain weight
is disproportionate.

### The composition alternative

An application could drive the engine over base ACP and ship its domain tools as an
**MCP server** — both already supported (`navi acp`, `crates/navi-mcp`,
and `NewSessionParams.mcp_servers` already carries per-session MCP servers).

**This is the adopted mechanism for R2**, and upstream v2 makes it the sanctioned
one. ACP v2 *removed* `fs/*` and `terminal/*` and states the replacement directly:

> "The Client file system, terminal execution, and session modes APIs are gone. […]
> use client-provided MCP servers when the Agent needs Client-side tools."

An earlier revision of this ADR rejected composition on the grounds that the MCP
server is a separate process, severed from the UI state its tools depend on. That
objection was correct for stdio/HTTP MCP and is **retired** by the
[MCP-over-ACP RFD](https://agentclientprotocol.com/rfds/mcp-over-acp), which adds
`"type": "acp"` as an MCP transport: the client-provided MCP server lives *in the
client's address space* and its traffic is routed over the existing ACP connection
via `mcp/connect`, `mcp/message`, `mcp/disconnect`. The RFD's own motivation is this
requirement verbatim — "allow a client […] to create custom MCP tools that are
tailored to a specific request and which live in the client's address space" — and
it names the alternative NAVI would otherwise have built ("some sort of backdoor")
as the problem it exists to remove.

So R2 and R4 are satisfied by ACP, not by a NAVI extension. What composition does
**not** supply is R5: harness, tool selection and storage namespace have no upstream
representation and would otherwise travel as CLI flags — unversioned, undiscoverable,
and unable to degrade when the installed engine is older than the app expects (R7).
That gap, and only that gap, is what the `navi.dev` extension covers.

## Decision

**The NAVI application boundary is ACP v2 plus a `navi.dev` extension profile.**
No new protocol is defined. Base ACP remains fully functional and unmodified.

### 1. Principles

1. **Extensions are additive and optional.** Base ACP must keep working. NAVI never
   *requires* a `_navi.dev/*` method; it feature-detects via advertised capabilities
   and degrades. Announcing a protocol version and then requiring a non-standard
   method is a compatibility lie and is prohibited.
2. **No root fields, ever.** All extension data lives under `_meta`, keyed by
   `navi.dev`, per the spec constraint quoted above.
3. **Two audiences, two stability tiers.** Base ACP + the app-facing extension are
   small and stable. Engine administration (credentials, model catalog, plugin
   install) is a separate, explicitly volatile extension namespace behind its own
   capability. This structurally fixes the current design, where one 123-method API
   serves both audiences so every app inherits credential-manager churn.
4. **Upstream first.** Where an extension is generally useful — client-declared
   tools most of all — propose it to ACP. If accepted, the extension disappears into
   the spec and maintenance cost goes to zero.
5. **Fork is a last resort.** Needing to *change* base ACP semantics (not add to
   them) is the signal to reconsider a fork. Adding is not that signal.

### 2. Non-goals

- A general agent-interoperability protocol — ACP already is one, and NAVI keeps
  speaking it unmodified.
- A third-party extension mechanism — that is MCP (client) and WASM plugins (ADR 0013).
- A UI transport. The boundary carries **state, never markup**. Shipping HTML/JS
  across it (the "MCP Apps" pattern) is rejected: the application owns its UI, and a
  markup channel would couple engine releases to frontend rendering.
- Multi-agent orchestration; model-provider wire formats.

### 3. Prerequisite: ACP v2 support

`crates/navi-acp` targets `PROTOCOL_VERSION = 1` (`types.rs:5`) and uses the v1
capability shape. This ADR depends on v2 for both its extensibility rules and — per
footnote 6 — its client-provided-MCP mechanism.

**Adding** v2 is the prerequisite; migrating *to* v2 is not. Upstream directs
implementers to support both versions side by side and negotiate per connection, and
labels the v2 surface draft. Specified in **ADR 0023**.

Consequence: the app profile requires v2 and must **refuse to activate** over a v1
connection rather than partially degrade — a half-applied profile would mean the
wrong harness or the wrong storage scope.

### 4. Requirement → mechanism

| Requirement | Mechanism | Notes |
|---|---|---|
| R1 drive turns | base ACP `session/new`, `session/prompt`, `session/cancel` | unchanged |
| R1 streaming | base ACP `session/update` | `agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`, `plan`, `usage_update` already cover the surface |
| R2 declare tools | base ACP **client-provided MCP servers** in `session/new` | standard MCP `tools/list` shape; no NAVI extension |
| R2 invoke tool | base ACP MCP routing; **MCP-over-ACP** (`type: "acp"`, `mcp/connect`, `mcp/message`) when advertised via `mcpCapabilities.acp` | keeps app tools in the app's address space, beside their UI state |
| R2 tools changed | standard MCP `tools/list_changed` over the same channel | no NAVI extension |
| R3 approval | base ACP `session/request_permission` | v2 restructures params (required `title`, optional `subject`) |
| R3 structured question | base ACP **Elicitation** | v2 has an elicitation surface; NAVI must use it rather than inventing `_navi.dev/question` |
| R3 plan review | base ACP `plan_update` + `session/request_permission` | evaluate before extending; only if the standard pair cannot express review-with-feedback |
| R3 secret | `_navi.dev/secret` | only if elicitation cannot mark a response non-persistable; never logged, never persisted |
| R4 one channel | inherent to ACP | bidirectional JSON-RPC |
| R5 app profile | `initialize.params._meta["navi.dev"].app` | §5 |
| R6 transport | stdio (local) and WebSocket (remote) | schema-invariant; §6 |
| R7 skew | ACP `protocolVersion` + capability negotiation + `_`-variant rules | §7 |
| R8 languages | upstream ACP schema + committed `navi.dev` extension schema → codegen | §8 |
| R9 no rebuild | harness packs + MCP + WASM plugins, all data under `{data_dir}` | unchanged |

Engine-specific `session/update` variants, if ever needed, use `_`-prefixed
`sessionUpdate` values per the enum-extension rule. Preference is to avoid them and
carry engine-specific detail in the `_meta` of existing variants.

### 5. App profile

```jsonc
// initialize.params._meta["navi.dev"].app
{
  "id": "organize-anything",
  "version": "0.3.1",
  "requiresEngine": ">=1.2 <2.0",
  "harness":  { "ref": "organize-anything/install" },   // or { "inline": { …pack } }
  "prompt":   { "text": "…" },
  "tools":    { "profile": "app_only", "allow": [], "deny": [] },
  "storage":  { "namespace": "organize-anything", "shareCredentials": true },
  "model":    { "provider": "…", "name": "…", "userOverridable": true }
}
```

`harness` maps onto the existing declarative pack format
(`{data_dir}/harnesses/<id>/` with `loop.toml` / `graph.toml` / `CAPABILITY.md`,
`crates/navi-core/src/harness_pack/`), so a profile is data, not code (R9).

The engine echoes the **resolved** profile in
`initialize.result.capabilities._meta["navi.dev"].app`, which may differ from what
was requested when the installed engine is older. If `requiresEngine` is
unsatisfiable, `initialize` fails with a typed error carrying the engine version so
a launcher can fall back to a bundled binary instead of proceeding on wrong
assumptions.

`storage.namespace` scopes sessions, memory and app config under
`{data_dir}/apps/<namespace>/` while `shareCredentials` keeps the user's API keys
and registry cache shared. **This depends on data-dir namespacing, which does not
exist today** (`data_dir` is engine-scoped and flat) — specified in ADR 0019, see §10.

Where the profile comes from, what it may and may not override, and the app bundle
layout are specified in ADR 0020. `requiresEngine` handling and the bundled-engine
fallback are specified in ADR 0021.

### 6. Transports

| Transport | Use | Framing |
|---|---|---|
| **stdio** | local sidecar (desktop apps) | newline-delimited JSON |
| **WebSocket** | remote / multi-tenant (SaaS) | one duplex connection, many sessions; auth at handshake |

Plain HTTP + SSE is **not** conforming: R2 and R3 require an Agent → Client request
path with responses, which SSE cannot provide.

WebSocket is **not** a NAVI invention here: upstream has a
[Streamable HTTP & WebSocket Transport RFD](https://agentclientprotocol.com/rfds/streamable-http-websocket-transport)
in flight. NAVI should track it rather than define its own remote framing, and
should treat any interim implementation as provisional.

Consequence for server-rendered stacks (Rails/ERB): the **application server**
holds the connection; the browser never speaks ACP and receives updates through the
framework's own channel (Turbo/ActionCable). This supersedes an earlier suggestion
that browsers consume the boundary over SSE directly.

### 7. Versioning and forward compatibility

- Base version discipline is ACP's (`protocolVersion`, currently 2).
- Extension capabilities are advertised, never assumed. Unrecognised `_navi.dev/*`
  request → `-32601`; unrecognised notification → ignore.
- Unknown tagged-union values: `_`-prefixed are foreign extensions, non-prefixed are
  future ACP variants. NAVI must not treat the latter as extensions.
- **Unknown payloads must be preserved** when storing, replaying, proxying or
  forwarding. This is a live conformance gap: `SessionUpdate`, `ContentBlock` and
  `StopReason` in `navi-acp/src/types.rs` use `#[serde(other)]`, whose fieldless
  `Other` variant **discards** the payload. Since NAVI persists and replays sessions,
  this loses data across an engine upgrade and must be fixed to capture the raw value.

### 8. Schema, codegen, conformance

- **Base schema** comes from upstream ACP — not NAVI's to maintain.
- **Extension schema** (`navi.dev` `_meta` payloads and `_navi.dev/*` methods) is
  defined in Rust, exported to JSON Schema (`schemars`), and committed. Drift fails CI.
- **Generated per language:** types, framing, dispatch. Existing ACP client libraries
  cover the base; only the extension layer is NAVI-specific.
- **Hand-written per language:** an ergonomic facade only (target ~200 lines), where
  `navi.systemPrompt(…)`-style sugar lives. Small and stable; the generated layer
  absorbs release churn.
- **Conformance corpus:** language-agnostic JSON transcripts under
  `tests/acp-conformance/`, covering base ACP and the extension profile. Every client
  SDK ships a runner; CI gates on it. This replaces `engine_api.rs`'s string arrays
  with behavioural checks.

Codegen is not free — a build step, generator bugs, unpleasant generated code. It is
justified only because the client-language count is ≥ 2; with one client language
hand-writing would be cheaper.

### 9. Two stability tiers

| Tier | Surface | Stability |
|---|---|---|
| App | base ACP + app profile `_meta` (+ `_navi.dev/secret` if §4 requires it) | stable; additive only |
| Admin | `_navi.dev/engine/*` — model catalog, model selection, credentials, usage, plugin/MCP management | **explicitly volatile**; minor versions may change it |

**Admin extensions must be checked against in-flight RFDs before being written.**
Upstream already has proposals covering much of this territory — Configurable LLM
Providers, Session Compaction, End-Turn Token Usage, Session Notices, Session Config
Options. Building `_navi.dev/engine/*` for a surface that is weeks from
standardisation is the exact waste principle 4 exists to prevent. The default is to
wait or to participate in the RFD; extend only where no proposal exists.

Admin is behind its own advertised capability. An app that wants a model picker opts
into that churn knowingly; apps that never request it are structurally immune — the
property the current single-tier API cannot provide.

### 10. Dependency

Storage namespacing (§5) is a prerequisite. Without it an application shares
`{data_dir}` flatly with the user's TUI: the app's harness appears in
`navi harness list`, its sessions pollute the TUI history, and its memory mixes with
the coding agent's. Credentials and the registry cache **should** be shared; nothing
else should. Specified in **ADR 0019**; it blocks a usable app profile.

Full dependency set for this ADR:

| ADR | Supplies | Blocking? |
|---|---|---|
| 0023 | ACP v2 support alongside v1 | **yes** — §3; supplies both the extensibility rules and R2 |
| 0019 | app-scoped `{data_dir}` namespacing | **yes** — §5 has nowhere to land without it |
| 0020 | app profile provenance, powers/limits, bundle format | **yes** — §5 has no source without it |
| 0021 | `requiresEngine` evaluation, discovery, bundled fallback | no — §5 degrades to "engine as found" |
| 0022 | schema, codegen, conformance corpus | no — §8 can start hand-written for one language |

## Consequences

Positive:
- Satisfies R1–R9 with **no new protocol** — every mechanism is ACP-specified
- Base ACP interop preserved by construction; NAVI remains drivable from ACP editors
- Inherits ACP's versioning discipline, schema, docs and client libraries
- Application domain tools live in the application process, beside their UI state
- Collapses the per-language binding tax: `navi-napi`, `navi-dart` and
  `navi-server`'s hand-written routes reduce to one generated transport
- Both parity arrays in `engine_api.rs` become unnecessary — the schema is the list
- App surface and admin surface separated by construction
- One boundary serves desktop (stdio) and SaaS (WebSocket)
- Apps remain data + domain tools + UI; no engine rebuild (R9)
- App tools flow through the existing approval ladder rather than around it
- If client-declared tools are accepted upstream, the extension disappears into the
  spec at zero migration cost

Negative:
- Tracks an upstream spec NAVI does not control; ACP v3 would require adaptation
- `navi-acp` must migrate v1 → v2 before any of this lands (§3)
- `_meta`-nested payloads are more verbose and less self-documenting than root fields
- Extension methods are not portable to other ACP agents — acceptable, since NAVI is
  the only agent these applications target
- Codegen infrastructure is new build surface and a new failure mode
- Storage namespacing is a prerequisite, not a consequence (§10)
- WebSocket-only remote transport is harder to operate behind proxies and CDNs than
  plain HTTP
- Discipline risk: enough accumulated extensions become a de-facto fork with none of
  a fork's clarity. Mitigation is principle 4 — upstream anything generally useful

## Alternatives rejected

| Alternative | Why rejected |
|---|---|
| New protocol (NEP, earlier revision of this ADR) | ACP v2 extensibility closes the gap; a third protocol adds fragmentation, zero ecosystem, and full spec ownership for no capability gain |
| Base ACP unchanged | No client-declared tools (R2); no profile negotiation (R5) |
| MCP alone | Not an agent-driving protocol (R1); roles inverted (R4) |
| ACP + MCP composed | Adequate for desktop-only; rejected as the general pattern because app tools are severed from UI state and there is no profile negotiation |
| Fork ACP | Only warranted to change base semantics; adding does not require it |
| Custom root fields on ACP types | Prohibited by the spec — all root names reserved for future versions |
| gRPC | Meets the matrix; rejected on operability |
| REST + SSE | Cannot express Agent → Client requests |
| Hand-written SDK per language | Reproduces the existing 8 000-line marshalling defect at higher multiplicity |

## References
- ADR 0023 — ACP v2 support alongside v1 (blocking dependency, §3)
- ADR 0019 — App-scoped `{data_dir}` namespacing (blocking dependency, §5/§10)
- ADR 0020 — App profile config tier and bundle format (blocking dependency, §5)
- ADR 0021 — Engine discovery, version contract, bundled fallback (§5 `requiresEngine`)
- ADR 0022 — Generated client SDKs and conformance corpus (§8)
- [ACP v2 — Extensibility](https://agentclientprotocol.com/protocol/v2/extensibility) — `_meta`, `_`-prefixed methods and variants, capability advertisement, the no-root-fields constraint
- [ACP v2 — Migrating from v1](https://agentclientprotocol.com/protocol/v2/migration) — removal of `fs/*`/`terminal/*` and the client-provided-MCP replacement (footnote 6)
- [RFD — MCP-over-ACP](https://agentclientprotocol.com/rfds/mcp-over-acp) — `type: "acp"` transport that retires this ADR's original objection to composition
- [RFD — Streamable HTTP & WebSocket Transport](https://agentclientprotocol.com/rfds/streamable-http-websocket-transport) — §6 remote transport
- `crates/navi-acp` — hand-written ACP implementation; v1 today, v2 required (§3)
- `crates/navi-mcp` — MCP client; retained for third-party tool provision
- `crates/navi-core/src/harness_pack/` — declarative harness format used by §5
- `crates/navi-sdk/src/engine_api.rs` — the parity arrays this ADR makes obsolete
- ADR 0013 — WASM-Only Plugins (third-party extension path)
- ADR 0014 — Host-Mediated TUI Extensions (precedent for state-not-markup)
- `AGENTS.md` non-negotiable #2 (surface sync) — the cost this ADR removes
- `AGENTS.md` non-negotiable #3 (no worktree agent state) — §5 storage namespacing
  lands under `{data_dir}`, never in a project directory
