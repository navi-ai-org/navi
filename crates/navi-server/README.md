# navi-server

HTTP/WebSocket server exposing the NAVI agent engine for remote clients (e.g. navi_mobile).

## Overview

`navi-server` wraps `navi-sdk`'s `NaviEngine` and exposes it over a simple HTTP+WebSocket protocol that can be consumed by the Flutter mobile app (or any other remote client) over Tailscale.

## Usage

### systemd (recommended) — via `navi` CLI

```bash
# Build binaries once
cargo build -p navi-cli -p navi-server --release
# put both on PATH, or install next to each other

# Install user service (default project = cwd)
navi server install --project /path/to/project --port 9800

# Start / stop / status / logs
navi server start
navi server status
navi server logs -f
navi server stop

# System-wide unit (root)
sudo navi server install --system --project /path/to/project --force
sudo navi server start --system
```

Unit files:

| Scope | Unit | Env (secret) |
|---|---|---|
| user (default) | `~/.config/systemd/user/navi-server.service` | `~/.config/navi/server.env` |
| system (`--system`) | `/etc/systemd/system/navi-server.service` | `/etc/navi/server.env` |

Keep a user service after logout: `loginctl enable-linger $USER`.

### Foreground (debug)

```bash
# Start with a shared secret
cargo run -p navi-server -- --secret my-secret --project /path/to/project

# Custom bind address and port
cargo run -p navi-server -- --bind 0.0.0.0 --port 9800 --secret my-secret

# Or use environment variable for secret
NAVI_SERVER_SECRET=my-secret cargo run -p navi-server
```

## API

All endpoints (except `/health`) require `X-Navi-Secret` header.
Bodies accept both `snake_case` and `camelCase` field names where aliases are documented.

### Core session

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Health check (no auth) |
| GET | `/config` | Loaded config snapshot |
| GET | `/models` | List available models |
| GET | `/usage` | Usage / rate-limit report |
| GET | `/sessions` | List active session IDs |
| POST | `/sessions` | Start a session |
| GET | `/sessions/:id` | Session info |
| POST | `/sessions/:id/turns` | Send a user turn |
| POST | `/sessions/:id/close` | Close a session |
| POST | `/sessions/:id/cancel` | Cancel active turn |
| POST | `/sessions/:id/approve` | Resolve tool approval |
| POST | `/sessions/:id/deny` | Deny tool approval |
| POST | `/sessions/:id/question` | Resolve interactive question |
| GET/POST/DELETE | `/sessions/:id/goal` | Get / set / clear goal |
| GET | `/sessions/:id/snapshot` | Session snapshot |
| POST | `/sessions/:id/model` | Set session model |
| POST | `/sessions/:id/skills` | Set session skills |
| GET | `/sessions/:id/mcp` | Live MCP servers for session |
| GET | `/sessions/:id/background` | List background commands |
| GET | `/sessions/saved` | List saved sessions |
| POST | `/sessions/load/:id` | Load + resume saved session |
| POST | `/sessions/:id/delete` | Delete saved session |
| POST | `/model/select` | Global model select |
| POST | `/providers/sync` | Sync one provider's models |
| POST | `/providers/sync-all` | Sync all providers |

### Session ops (`routes/session_ops.rs`)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/sessions/:id/mode` | Agent mode (`default` / `plan`) |
| POST | `/sessions/:id/plan/enter` | Enter plan mode |
| POST | `/sessions/:id/plan/exit` | Exit plan mode |
| POST | `/sessions/:id/plan/review` | Resolve plan review |
| POST | `/sessions/:id/sudo` | Resolve sudo password (secret never logged) |
| POST | `/sessions/:id/context` | Add context packet |
| POST | `/sessions/:id/rewind` | Rewind live history (`keepUserTurns`) |
| POST | `/sessions/:id/goal/status` | Update goal status |
| POST | `/sessions/:id/goal/checklist` | Replace goal checklist |
| POST | `/sessions/:id/goal/tasks/:taskId` | Update checklist task status |
| GET | `/sessions/:id/background/:taskId` | Poll background command |
| POST | `/sessions/:id/background/:taskId/cancel` | Cancel background command |
| POST | `/sessions/:id/rename` | Rename saved session |
| GET/POST | `/permission-mode` | Get / set global permission mode |

### Memory (`routes/memory.rs`)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/memory/status` | Memory system status |
| GET | `/memory/doctor` | Diagnostics |
| POST | `/memory/init` | Init DB (+ optional embeddings) |
| GET | `/memory` | List memories (`?status=`) |
| POST | `/memory` | Write memory |
| GET | `/memory/count` | Active count |
| GET | `/memory/index` | Prompt index markdown |
| GET | `/memory/search` | Search (`?q=` `&limit=`) |
| GET | `/memory/:id` | Read one |
| PATCH | `/memory/:id` | Update fields / status |
| DELETE | `/memory/:id` | Delete |
| GET | `/memory/history` | Search session history |
| POST | `/memory/dream` | Dream consolidation |
| POST | `/memory/distill` | Distill maintenance |
| POST | `/memory/checkpoint` | Manual checkpoint |
| GET | `/memory/rebuild-preview` | Rebuild context preview |

### Voice (`routes/voice.rs`)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/voice/status` | Voice config / install status |
| GET | `/voice/doctor` | Mic + model diagnostics |
| GET | `/voice/providers` | Transcription providers |
| GET | `/voice/installed` | Engine installed? (`?engine=`) |
| POST | `/voice/init` | Download engine package |
| POST | `/voice/transcribe` | Transcribe WAV file |
| POST | `/voice/stream/start` | Start PCM stream |
| POST | `/voice/stream/pcm` | Push 16 kHz mono f32 samples |
| POST | `/voice/stream/end` | End stream → final text |
| POST | `/voice/stream/cancel` | Cancel stream |
| WS | `/voice/events?secret=` | Stream `VoiceEvent` JSON |

### Plugins (`routes/plugins.rs`)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/plugins` | List installed |
| GET | `/plugins/search` | Marketplace search (`?q=`) |
| GET | `/plugins/:id` | Plugin info |
| POST | `/plugins/install/path` | Install from path (`confirm: true`) |
| POST | `/plugins/install/marketplace` | Install from marketplace |
| POST | `/plugins/update/path` | Update from path |
| POST | `/plugins/update/marketplace` | Update from marketplace |
| DELETE | `/plugins/:id` | Remove plugin |
| POST | `/plugins/reload-wasm` | Reload WASM plugins |

### Credentials / OAuth (`routes/auth.rs`)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/credentials` | List provider accounts |
| GET | `/credentials/:providerId` | Status + multi-accounts |
| PUT | `/credentials/:providerId` | Set API key |
| DELETE | `/credentials/:providerId` | Delete API key |
| GET | `/credentials/:providerId/accounts` | List accounts |
| POST | `/credentials/:providerId/accounts` | Add account |
| POST | `/credentials/:providerId/accounts/:accountId/select` | Select account |
| DELETE | `/credentials/:providerId/accounts/:accountId` | Delete account |
| GET | `/oauth/:providerId/supports` | Device OAuth supported? |
| POST | `/oauth/:providerId` | Run device OAuth (blocking) |

### Skills / MCP / routing (`routes/skills_mcp.rs`)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/skills` | List skills |
| GET | `/skills/:id` | Get skill (full body) |
| POST | `/skills` | Create/update skill |
| DELETE | `/skills/:id` | Delete skill |
| GET | `/mcp` | MCP config snapshot |
| PUT | `/mcp` | Replace full MCP config |
| POST | `/mcp/enabled` | Enable/disable MCP |
| POST | `/mcp/servers` | Upsert MCP server |
| DELETE | `/mcp/servers/:id` | Remove MCP server |
| GET | `/sessions/:id/mcp/tools` | Session MCP tool names |
| GET | `/routing` | Attachment + background models |
| POST | `/routing/attachment` | Set attachment model |
| DELETE | `/routing/attachment/:modality` | Clear attachment model |
| POST | `/routing/background` | Set background-task model |
| DELETE | `/routing/background/:task` | Clear background-task model |

### Registry (`routes/registry_models.rs`)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/registry` | Provider/model catalog summary |
| POST | `/registry/sync` | Sync registry cache (`force?`) |

### WebSocket

| Path | Description |
|------|-------------|
| `/sessions/:id/events?secret=...` | Stream session `RuntimeEvent`s as JSON |
| `/voice/events?secret=...` | Stream engine-global `VoiceEvent`s as JSON |

WebSocket auth uses `?secret=` query parameter (browsers cannot set custom headers).

## Authentication

All requests must include the `X-Navi-Secret` header with the shared secret configured at startup. WebSocket connections use the `?secret=` query parameter instead.

## Layout

```
src/
  server.rs          # core session / turn / WS wiring
  state.rs           # SharedState, auth filters, reply helpers
  routes/
    auth.rs          # credentials + OAuth
    memory.rs        # auto-memory CRUD + maintenance
    voice.rs         # voice / dictation
    plugins.rs       # plugin lifecycle
    session_ops.rs   # plan, sudo, permission, rewind, goals, bg
    skills_mcp.rs    # skills CRUD, MCP config, model routing
    registry_models.rs
```

## Product limits (by design)

- Single project (`--project`) and single shared secret per process
- No TLS termination (use Tailscale / reverse proxy)
- Not multi-tenant; not an OpenAI-compatible proxy
