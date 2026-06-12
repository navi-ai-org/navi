use anyhow::{Context, Result};
use async_trait::async_trait;
use navi_core::{
    McpConfig, McpServerConfig, Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult,
};
use rmcp::{
    RoleClient, ServiceExt,
    model::CallToolRequestParams,
    service::{Peer, RunningService, RunningServiceCancellationToken},
    transport::TokioChildProcess,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    pub id: String,
    pub tools: Vec<String>,
}

pub struct LoadedMcpServers {
    pub tools: Vec<Arc<dyn Tool>>,
    pub servers: Vec<McpServerInfo>,
    _connections: Vec<Arc<McpConnection>>,
}

impl LoadedMcpServers {
    /// Sends a graceful shutdown signal to every active MCP server. Callers
    /// that want deterministic teardown of MCP child processes should invoke
    /// this before dropping the `LoadedMcpServers` value.
    pub fn shutdown(&self) {
        for connection in &self._connections {
            connection.request_shutdown();
        }
    }
}

struct McpConnection {
    server_id: String,
    peer: Peer<RoleClient>,
    /// Token from `RunningService::cancellation_token()`. Wrapped in a
    /// `Mutex<Option<...>>` so that `request_shutdown` can be called via
    /// `&self` (the connection lives behind an `Arc`). The `Option` makes
    /// the operation idempotent: only the first caller actually cancels.
    cancel_token: Mutex<Option<RunningServiceCancellationToken>>,
    _service: RunningService<RoleClient, ()>,
}

impl McpConnection {
    /// Requests a graceful shutdown of the MCP server child process. Safe to
    /// call multiple times concurrently; only the first caller actually
    /// triggers the cancel.
    fn request_shutdown(&self) {
        let mut guard = self.cancel_token.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(token) = guard.take() {
            token.cancel();
        }
    }
}

impl Drop for McpConnection {
    fn drop(&mut self) {
        // Best-effort: if the caller forgot to invoke
        // `LoadedMcpServers::shutdown` explicitly, still cancel so rmcp tears
        // down the child process promptly. The underlying `_service` is
        // dropped right after, which would also kill it but less gracefully.
        if self
            .cancel_token
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            tracing::debug!(
                server = %self.server_id,
                "McpConnection dropped without explicit shutdown; cancelling now"
            );
            self.request_shutdown();
        }
    }
}

pub async fn load_configured_mcp_servers(config: &McpConfig) -> LoadedMcpServers {
    if !config.enabled {
        tracing::debug!("MCP integration disabled by config; no servers will be loaded");
        return LoadedMcpServers {
            tools: Vec::new(),
            servers: Vec::new(),
            _connections: Vec::new(),
        };
    }

    let mut tools = Vec::new();
    let mut servers = Vec::new();
    let mut connections = Vec::new();
    for server in config.servers.iter().filter(|server| server.enabled) {
        match connect_server(server).await {
            Ok((connection, server_tools)) => {
                let connection = Arc::new(connection);
                let mut tool_names = Vec::new();
                for remote_tool in server_tools {
                    let definition = tool_definition(server, &remote_tool);
                    tool_names.push(definition.name.clone());
                    tools.push(Arc::new(McpTool {
                        definition,
                        remote_name: remote_tool.name.to_string(),
                        server_id: connection.server_id.clone(),
                        peer: connection.peer.clone(),
                        timeout_ms: server.timeout_ms.unwrap_or(30_000),
                    }) as Arc<dyn Tool>);
                }
                servers.push(McpServerInfo {
                    id: server.id.clone(),
                    tools: tool_names,
                });
                connections.push(connection);
            }
            Err(err) => {
                tracing::warn!(server = %server.id, error = %err, "failed to connect MCP server");
            }
        }
    }

    LoadedMcpServers {
        tools,
        servers,
        _connections: connections,
    }
}

async fn connect_server(
    server: &McpServerConfig,
) -> Result<(McpConnection, Vec<rmcp::model::Tool>)> {
    let server_id = server.id.clone();
    let timeout = Duration::from_millis(server.timeout_ms.unwrap_or(30_000));

    // Outer deadline covers the whole spawn → serve → list pipeline. Any
    // sub-step that hangs (spawn pre-exec, MCP handshake, JSON-RPC read) is
    // bounded by this single timer so a misbehaving server can't stall NAVI
    // startup indefinitely.
    let connect = async {
        if let Some(url) = &server.url {
            let url_parsed = reqwest::Url::parse(url)
                .with_context(|| format!("invalid url for MCP server `{server_id}`: {url}"))?;
            let transport =
                rmcp::transport::StreamableHttpClientTransport::from_uri(url_parsed.as_str());
            let service = tokio::time::timeout(timeout, ().serve(transport))
                .await
                .with_context(|| format!("timed out initializing MCP server `{server_id}`"))?
                .with_context(|| format!("failed to initialize MCP server `{server_id}`"))?;
            let tools = tokio::time::timeout(timeout, service.peer().list_all_tools())
                .await
                .with_context(|| format!("timed out listing MCP tools for `{server_id}`"))?
                .with_context(|| format!("failed to list MCP tools for `{server_id}`"))?;
            let peer = service.peer().clone();
            let cancel_token = service.cancellation_token();
            Ok((
                McpConnection {
                    server_id: server_id.clone(),
                    peer,
                    cancel_token: Mutex::new(Some(cancel_token)),
                    _service: service,
                },
                tools,
            ))
        } else if let Some(cmd) = &server.command {
            let mut command = tokio::process::Command::new(cmd);
            command.args(&server.args);
            command.envs(&server.env);
            if let Some(cwd) = &server.cwd {
                command.current_dir(cwd);
            }

            let transport = tokio::task::block_in_place(|| TokioChildProcess::new(command))
                .with_context(|| format!("failed to spawn MCP server `{server_id}`"))?;
            let service = tokio::time::timeout(timeout, ().serve(transport))
                .await
                .with_context(|| format!("timed out initializing MCP server `{server_id}`"))?
                .with_context(|| format!("failed to initialize MCP server `{server_id}`"))?;
            let tools = tokio::time::timeout(timeout, service.peer().list_all_tools())
                .await
                .with_context(|| format!("timed out listing MCP tools for `{server_id}`"))?
                .with_context(|| format!("failed to list MCP tools for `{server_id}`"))?;
            let peer = service.peer().clone();
            let cancel_token = service.cancellation_token();
            Ok((
                McpConnection {
                    server_id: server_id.clone(),
                    peer,
                    cancel_token: Mutex::new(Some(cancel_token)),
                    _service: service,
                },
                tools,
            ))
        } else {
            anyhow::bail!("MCP server `{server_id}` must define either `url` or `command`");
        }
    };

    tokio::time::timeout(timeout, connect)
        .await
        .with_context(|| format!("timed out connecting MCP server `{server_id}` (overall)"))?
}

fn tool_definition(server: &McpServerConfig, tool: &rmcp::model::Tool) -> ToolDefinition {
    ToolDefinition {
        name: prefixed_tool_name(server, &tool.name),
        description: tool
            .description
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("MCP tool `{}` from server `{}`.", tool.name, server.id)),
        kind: ToolKind::Custom,
        input_schema: Value::Object((*tool.input_schema).clone()),
    }
}

fn prefixed_tool_name(server: &McpServerConfig, remote_name: &str) -> String {
    let prefix = server.tool_prefix.as_deref().unwrap_or(&server.id);
    format!("{prefix}__{remote_name}")
}

struct McpTool {
    definition: ToolDefinition,
    remote_name: String,
    server_id: String,
    peer: Peer<RoleClient>,
    timeout_ms: u64,
}

#[async_trait]
impl Tool for McpTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let arguments = match invocation.input {
            Value::Object(map) => map,
            other => {
                return Ok(ToolResult {
                    invocation_id: invocation.id,
                    ok: false,
                    output: json!({
                        "error": "MCP tool input must be a JSON object",
                        "input": other,
                    }),
                });
            }
        };
        let params = CallToolRequestParams::new(self.remote_name.clone()).with_arguments(arguments);
        let timeout = Duration::from_millis(self.timeout_ms);
        let result = tokio::time::timeout(timeout, self.peer.call_tool(params))
            .await
            .with_context(|| {
                format!(
                    "timed out calling MCP tool `{}` on server `{}`",
                    self.remote_name, self.server_id
                )
            })?
            .with_context(|| {
                format!(
                    "failed to call MCP tool `{}` on server `{}`",
                    self.remote_name, self.server_id
                )
            })?;
        let ok = !result.is_error.unwrap_or(false);
        let output = result
            .structured_content
            .unwrap_or_else(|| serde_json::to_value(result.content).unwrap_or_else(|_| json!([])));

        Ok(ToolResult {
            invocation_id: invocation.id,
            ok,
            output,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::Tool as McpRemoteTool;
    use std::borrow::Cow;

    #[test]
    fn prefixes_mcp_tool_names() {
        let server = McpServerConfig {
            id: "memory".to_string(),
            command: Some("mcp-memory".to_string()),
            url: None,
            args: Vec::new(),
            env: Default::default(),
            cwd: None,
            enabled: true,
            tool_prefix: None,
            timeout_ms: None,
        };
        assert_eq!(prefixed_tool_name(&server, "search"), "memory__search");
    }

    #[test]
    fn maps_remote_tool_to_core_definition() {
        let server = McpServerConfig {
            id: "notes".to_string(),
            command: Some("notes-mcp".to_string()),
            url: None,
            args: Vec::new(),
            env: Default::default(),
            cwd: None,
            enabled: true,
            tool_prefix: Some("n".to_string()),
            timeout_ms: None,
        };
        let remote = McpRemoteTool::new(
            "lookup",
            Cow::Borrowed("Lookup a note"),
            serde_json::Map::new(),
        );
        let definition = tool_definition(&server, &remote);

        assert_eq!(definition.name, "n__lookup");
        assert_eq!(definition.kind, ToolKind::Custom);
        assert!(definition.description.contains("Lookup"));
    }

    #[tokio::test]
    async fn load_skips_disabled_servers() {
        let config = McpConfig {
            enabled: true,
            servers: vec![McpServerConfig {
                id: "disabled".to_string(),
                command: Some("nope".to_string()),
                url: None,
                args: Vec::new(),
                env: Default::default(),
                cwd: None,
                enabled: false,
                tool_prefix: None,
                timeout_ms: None,
            }],
        };
        let loaded = load_configured_mcp_servers(&config).await;
        assert!(loaded.tools.is_empty());
        assert!(loaded.servers.is_empty());
    }

    #[tokio::test]
    async fn load_returns_empty_when_disabled() {
        let config = McpConfig {
            enabled: false,
            servers: vec![McpServerConfig {
                id: "ignored".to_string(),
                command: Some("nope".to_string()),
                url: None,
                args: Vec::new(),
                env: Default::default(),
                cwd: None,
                enabled: true,
                tool_prefix: None,
                timeout_ms: None,
            }],
        };
        let loaded = load_configured_mcp_servers(&config).await;
        assert!(loaded.tools.is_empty());
        assert!(loaded.servers.is_empty());
    }

    #[test]
    fn shutdown_is_safe_with_no_connections() {
        // Construct a synthetic LoadedMcpServers with no connections and
        // confirm shutdown() doesn't panic. This guards against accidentally
        // dereferencing an empty list.
        let loaded = LoadedMcpServers {
            tools: Vec::new(),
            servers: Vec::new(),
            _connections: Vec::new(),
        };
        loaded.shutdown();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn load_times_out_when_server_hangs() {
        // `sleep` is a portable command that doesn't speak the MCP protocol,
        // so the connection will hang after spawn. With a tiny timeout, the
        // outer deadline in `connect_server` should fire and `load` should
        // return empty (since per-server failures are logged-and-skipped).
        let config = McpConfig {
            enabled: true,
            servers: vec![McpServerConfig {
                id: "hanging".to_string(),
                command: Some("sleep".to_string()),
                url: None,
                args: vec!["10".to_string()],
                env: Default::default(),
                cwd: None,
                enabled: true,
                tool_prefix: None,
                timeout_ms: Some(200),
            }],
        };
        let start = std::time::Instant::now();
        let loaded = load_configured_mcp_servers(&config).await;
        let elapsed = start.elapsed();

        // The load must complete (returning an empty LoadedMcpServers because
        // the per-server connect error is logged and skipped), and it must do
        // so within the configured timeout window plus a small grace period
        // (test runner overhead, scheduling, etc.).
        assert!(loaded.tools.is_empty());
        assert!(loaded.servers.is_empty());
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "load_configured_mcp_servers did not honor timeout (took {elapsed:?})"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn load_skips_missing_command_without_hanging() {
        // A non-existent command should fail fast (spawn returns an error
        // immediately, no timeout needed). This guards against a regression
        // where a missing command would hang the runtime.
        let config = McpConfig {
            enabled: true,
            servers: vec![McpServerConfig {
                id: "missing".to_string(),
                command: Some("this-binary-does-not-exist-navimcptest".to_string()),
                url: None,
                args: Vec::new(),
                env: Default::default(),
                cwd: None,
                enabled: true,
                tool_prefix: None,
                timeout_ms: Some(5_000),
            }],
        };
        let start = std::time::Instant::now();
        let loaded = load_configured_mcp_servers(&config).await;
        let elapsed = start.elapsed();

        assert!(loaded.tools.is_empty());
        assert!(loaded.servers.is_empty());
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "load_configured_mcp_servers hung on missing command (took {elapsed:?})"
        );
    }
}
