# navi-mcp

[![Crates.io](https://img.shields.io/crates/v/navi-mcp)](https://crates.io/crates/navi-mcp)
[![License](https://img.shields.io/crates/l/navi-mcp)](../LICENSE)

MCP (Model Context Protocol) client integration for [NAVI](https://github.com/navi-ai-org/navi).

`navi-mcp` connects to configured stdio MCP servers and maps their remote tools into NAVI's `ToolExecutor`, making them available to the agent alongside built-in and plugin tools.

## What it does

1. Spawns MCP server processes as child processes via stdio transport
2. Discovers available tools from each server
3. Wraps each remote tool as a `navi_core::Tool` with proper definitions and schemas
4. Registers them with the engine's `ToolExecutor`
5. Handles graceful shutdown of server connections

## Configuration

MCP servers are configured in `config.toml`:

```toml
[[mcp.servers]]
id = "memory"
command = "mcp-server-memory"
args = ["--port", "3000"]
tool_prefix = "mem"
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
