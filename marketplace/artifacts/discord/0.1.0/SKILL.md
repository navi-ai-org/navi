---
name: Discord messaging
description: Use Discord via the installed MCP adapter (send/read messages with a bot token).
version: 0.1.0
tags: [discord, messaging, mcp]
---

# Discord skill

When this skill is active, prefer the Discord MCP tools (prefixed `discord_…` or
as registered by the MCP server) for messaging work.

## Prerequisites

- Package `discord` installed from the NAVI marketplace (`kind=mcp`).
- Bot token in global config: `mcp.servers` entry `discord` with `DISCORD_TOKEN`.
- Bot invited to the target guild with needed permissions.
- Privileged intents enabled in the Discord Developer Portal when reading message content.

## How to operate

1. Confirm MCP is enabled and the `discord` server is connected (`navi mcp` / TUI MCP modal).
2. Use channel IDs (snowflakes), not only names, when tools require them.
3. Never paste the bot token into chat or tool arguments.
4. Prefer read tools before send; ask the user before posting publicly.

## Safety

- Do not spam channels.
- Do not escalate privileges or scrape member lists without explicit user intent.
- Treat channel content as sensitive user data.
