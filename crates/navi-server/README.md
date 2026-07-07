# navi-server

HTTP/WebSocket server exposing the NAVI agent engine for remote clients (e.g. navi_mobile).

## Overview

`navi-server` wraps `navi-sdk`'s `NaviEngine` and exposes it over a simple HTTP+WebSocket protocol that can be consumed by the Flutter mobile app (or any other remote client) over Tailscale.

## Usage

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

### HTTP

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Health check (no auth) |
| POST | `/sessions` | Start a session |
| GET | `/sessions` | List active session IDs |
| POST | `/sessions/:id/turns` | Send a user turn |
| POST | `/sessions/:id/close` | Close a session |
| POST | `/sessions/:id/cancel` | Cancel active turn |
| POST | `/sessions/:id/approve` | Resolve tool approval |
| POST | `/sessions/:id/question` | Resolve interactive question |
| GET | `/models` | List available models |
| GET | `/config` | Loaded config snapshot |

### WebSocket

| Path | Description |
|------|-------------|
| `/sessions/:id/events?secret=...` | Stream `RuntimeEvent`s as JSON |

WebSocket auth uses `?secret=` query parameter (since browsers can't set custom headers).

## Authentication

All requests must include the `X-Navi-Secret` header with the shared secret configured at startup. WebSocket connections use the `?secret=` query parameter instead.
