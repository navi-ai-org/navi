# Discord (NAVI marketplace package)

**Kind:** `mcp`  
**Runtime:** WASM package shell + MCP stdio server  
**MCP command:** `npx -y @iqai/mcp-discord`

## Flow (end-to-end)

```text
Discord Developer Portal
  → create Application + Bot
  → copy token, enable Message Content Intent
  → invite bot to server
       ↓
navi plugin install-marketplace discord --yes
  → files under {data_dir}/plugins/discord/
  → optional: confirm merge of mcp.json → ~/.config/navi/config.toml
       ↓
edit config: set DISCORD_TOKEN under [mcp.servers] env
       ↓
start NAVI session
  → navi-mcp spawns npx @iqai/mcp-discord with env
  → agent sees Discord tools (send/read messages, etc.)
```

## Install

```bash
# Local catalog (dev):
# [plugin_marketplace]
# registry_url = "file:///ABS/PATH/navi/marketplace/catalog.json"

navi plugin search discord
navi plugin install-marketplace discord --yes
# When prompted, merge mcp.json into global config (or confirm in TUI).
```

Path install (LocalDev):

```bash
navi plugin install marketplace/artifacts/discord/0.1.0 --yes
```

## Configure token

After install, ensure global config contains something like:

```toml
[mcp]
enabled = true

[[mcp.servers]]
id = "discord"
command = "npx"
args = ["-y", "@iqai/mcp-discord"]
enabled = true
tool_prefix = "discord"
timeout_ms = 60000

[mcp.servers.env]
DISCORD_TOKEN = "YOUR_BOT_TOKEN_HERE"
```

**Never commit the token.** Prefer env substitution only if your deploy injects secrets; by default put the token only in the user-global config.

## Discord portal checklist

1. https://discord.com/developers/applications → New Application  
2. **Bot** → Add Bot → Reset Token  
3. **Privileged Gateway Intents** → enable **Message Content Intent** (required to read message text)  
4. **OAuth2 → URL Generator** → scope `bot` → permissions (e.g. Send Messages, Read Message History, View Channels)  
5. Open the generated URL and add the bot to your server  

Official docs: [Gateway Intents](https://discord.com/developers/docs/topics/gateway#gateway-intents).

## Requirements

- Node.js 18+ (for `npx`)
- Network access to Discord API and npm registry
- NAVI MCP client enabled

## What this package is / is not

| Is | Is not |
|----|--------|
| Marketplace package that wires MCP Discord | A full Discord gateway inside WASM |
| Documented bot-token setup | Hosting your bot for you |
| Mergeable `mcp.json` for global config | Hardcoded secrets |

Remote tools come from `@iqai/mcp-discord` at runtime. The WASM entry is a signed package shell + optional setup hint tool.
