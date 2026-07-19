# navi-acp

Agent Client Protocol (ACP) **client** for NAVI.

Spawns external ACP agent servers (JSON-RPC over stdio), drives
`initialize` / `authenticate` / `session/new` / `session/prompt` /
`session/cancel`, and streams typed session updates.

This is **not** a `ModelProvider`. See [DESIGN.md](./DESIGN.md).

## Example config

```toml
[acp]
enabled = true

[[acp_agents]]
id = "devin"
command = "devin"
args = ["acp"]
api_key_env = "DEVIN_API_KEY"
```

## Smoke test

```bash
ACP_SMOKE_TEST=1 DEVIN_API_KEY=... cargo test -p navi-acp smoke -- --ignored --nocapture
```
