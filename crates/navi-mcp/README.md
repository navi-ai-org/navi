# navi-mcp

[![Crates.io](https://img.shields.io/crates/v/navi-mcp)](https://crates.io/crates/navi-mcp)
[![License](https://img.shields.io/crates/l/navi-mcp)](../LICENSE)

MCP (Model Context Protocol) client integration for [NAVI](https://github.com/navi-ai-org/navi).

`navi-mcp` connects to configured stdio and Streamable HTTP MCP servers and maps their remote tools into NAVI's `ToolExecutor`, making them available to the agent alongside built-in and plugin tools.

## What it does

1. Connects to MCP servers through stdio or Streamable HTTP
2. Negotiates modern MCP `2026-07-28` with legacy fallback
3. Discovers server capabilities and available tools
4. Wraps each remote tool as a `navi_core::Tool` with proper definitions and schemas
5. Registers them with the engine's `ToolExecutor`
6. Handles graceful shutdown of server connections

## Configuration

MCP servers are configured in `config.toml`:

```toml
[[mcp.servers]]
id = "memory"
command = "mcp-server-memory"
tool_prefix = "mem"

[[mcp.servers]]
id = "remote"
url = "http://127.0.0.1:8080/mcp"
tool_prefix = "remote"
```

## Architecture

```text
┌──────────────┐     stdio      ┌──────────────────┐
│  NAVI Engine │ ◄────────────► │  MCP Server       │
│  ToolExecutor│                │  (child process)  │
│              │   JSON-RPC     │                   │
│  "mem__get"  │ ◄────────────► │  "get" tool       │
└──────────────┘                └──────────────────┘
```

Each MCP tool is prefixed with the server's `tool_prefix` (e.g., `mem__get`) to avoid name collisions with built-in tools.

## Part of the NAVI workspace

This crate depends on [`navi-core`](https://crates.io/crates/navi-core) and the [`rmcp`](https://crates.io/crates/rmcp) MCP client library.

**Full project:** <https://github.com/navi-ai-org/navi>
**License:** Apache-2.0
