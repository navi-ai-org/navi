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
id = "acme-agent"
command = "acme-agent"
args = ["acp"]
api_key_env = "ACP_AGENT_KEY"
```

## Smoke test

```bash
ACP_SMOKE_TEST=1 ACP_AGENT_KEY=... cargo test -p navi-acp smoke -- --ignored --nocapture
```
