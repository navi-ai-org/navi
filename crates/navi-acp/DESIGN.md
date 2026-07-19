# ACP client design (v1)

Agent Client Protocol (ACP) is **agent-level** turn delegation over JSON-RPC
stdio, not model inference. The external agent owns its harness, tools, and
auth. navi is the ACP *client* that spawns the server subprocess, drives the
session lifecycle, and surfaces progress into navi’s runtime event stream.

## 1. Trait shape

**Decision: new peer surface, not `ModelProvider`.**

| Surface | Owns | Output |
|---|---|---|
| `ModelProvider` | Token stream for *one* model call | `ModelStreamEvent` |
| ACP peer | Full agent turn (tools, plans, auth) | `AcpEvent` → mapped `RuntimeEvent` |

ACP agents run their own agent loop. Forcing them into `ModelProvider` would
erase tool/permission/plan semantics and break the product boundary
(“external agent peer”).

`navi-acp` exposes:

- `AcpClient` — concrete stdio client (spawn, RPC methods, event stream)
- `ExternalAgentPeer` — thin async trait for “delegate a prompt turn”

Engine integration is `NaviEngine::delegate_acp_turn` (and related helpers),
not `build_provider_for_config`.

## 2. Event mapping

ACP `session/update` notifications become typed `AcpSessionUpdate` values on
an `AcpEvent` stream. The engine maps them onto existing runtime events where
there is a clear UI meaning, and adds peer-specific kinds for the rest:

| ACP update | navi `RuntimeEventKind` |
|---|---|
| `agent_message_chunk` (text) | `AssistantDelta` + `AcpPeerUpdate` |
| `agent_thought_chunk` | `AssistantThinkingDelta` + `AcpPeerUpdate` |
| `tool_call` / `tool_call_update` | `AcpPeerUpdate` (raw peer progress) |
| `plan` | `AcpPeerUpdate` |
| `usage_update` | `TokensUpdated` when mappable + `AcpPeerUpdate` |
| other / unknown | `AcpPeerUpdate` only |
| prompt `stopReason` | `TurnCompleted` |

`ModelStreamEvent` is **not** extended. Inference and peer delegation stay
separate channels; only the engine-level runtime bus is shared with TUI/SDK
subscribers.

## 3. Registry / config

**Decision: `[[acp_agents]]` in navi TOML config (global + project, with
project stripped like MCP for supply-chain safety).**

```toml
[acp]
enabled = true

[[acp_agents]]
id = "devin"
command = "devin"
args = ["acp"]
api_key_env = "DEVIN_API_KEY"   # optional; passed via authenticate _meta.api_key
auth_method_id = "devin-browser" # optional; defaults to first advertised method
auto_approve_permissions = true  # v1 default for headless/delegate path
```

Why not navi-registry `kind: "acp-agent"` in v1: agents are local process
launches (command + args + env), not model catalog entries. Registry can
come later for discovery; config is enough to target any ACP binary today
(Zed/JetBrains use the same command+args shape).

## 4. Auth vs CredentialStore

**Decision: navi does not own the ACP server’s long-lived credentials.**

- Primary path: optional `api_key_env` → read env at connect time →
  `authenticate` with `_meta.api_key` (Devin-compatible).
- Alternate: server-driven flows (browser PKCE / terminal login) when no key
  is supplied; navi only issues `authenticate` with the advertised method id.
- **CredentialStore is not used** as the source of truth for ACP agent
  accounts. Passing through a key is a client convenience, not account
  management for the peer.

## 5. Session lifecycle

**Decision: one ACP subprocess per agent connection; one or more ACP
sessions on that process; navi sessions map loosely.**

```
navi session (optional UI context)
   └── delegate_acp_turn(agent_id, prompt)
         └── AcpClient (child: command + args)
               initialize → authenticate? → session/new → session/prompt*
```

- Subprocess is created for the delegate call (or held for multi-prompt reuse
  on the same `AcpClient` handle).
- Each `session/new` yields an ACP `sessionId` independent of navi’s session
  id; when called from an active navi session, events are also published on
  that session’s broadcast bus under a generated turn id.
- `session/cancel` maps to navi cancel for the in-flight prompt.

One subprocess serving many concurrent navi sessions is out of scope for v1
(serialization / multi-session demux complexity).

## 6. Permission routing (`session/request_permission`)

ACP servers call back into the client for tool permission.

**v1:**

1. If `auto_approve_permissions = true` (default for configured agents),
   respond with the first `allow_*` option (or first option).
2. If false and a navi session is attached, emit
   `RuntimeEventKind::ApprovalRequired` and wait on
   `ApprovalResolver` (same path as native tools), then map approve/deny to
   the ACP permission outcome.
3. On turn cancel, respond with `{ outcome: { outcome: "cancelled" } }`.

Client-side `fs/*` and `terminal/*` methods are **not** fully implemented in
v1; client capabilities advertise `fs`/`terminal` false so agents that
respect capabilities avoid them. Permission handling alone unblocks many
agent-local tool turns.
