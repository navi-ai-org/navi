use anyhow::{Context, Result};
use async_trait::async_trait;
use navi_core::{
    McpConfig, McpServerConfig, Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult,
};
use rmcp::{
    RoleClient, ServiceExt,
    model::CallToolRequestParams,
    service::{Peer, RunningService},
    transport::TokioChildProcess,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
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

struct McpConnection {
    server_id: String,
    peer: Peer<RoleClient>,
    _service: RunningService<RoleClient, ()>,
}

pub async fn load_configured_mcp_servers(config: &McpConfig) -> LoadedMcpServers {
    if !config.enabled {
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
    let mut command = tokio::process::Command::new(&server.command);
    command.args(&server.args);
    command.envs(&server.env);
    if let Some(cwd) = &server.cwd {
        command.current_dir(cwd);
    }

    let transport = TokioChildProcess::new(command)
        .with_context(|| format!("failed to spawn MCP server `{}`", server.id))?;
    let timeout = Duration::from_millis(server.timeout_ms.unwrap_or(30_000));
    let service = tokio::time::timeout(timeout, ().serve(transport))
        .await
        .with_context(|| format!("timed out initializing MCP server `{}`", server.id))?
        .with_context(|| format!("failed to initialize MCP server `{}`", server.id))?;
    let tools = tokio::time::timeout(timeout, service.peer().list_all_tools())
        .await
        .with_context(|| format!("timed out listing MCP tools for `{}`", server.id))?
        .with_context(|| format!("failed to list MCP tools for `{}`", server.id))?;
    let peer = service.peer().clone();
    Ok((
        McpConnection {
            server_id: server.id.clone(),
            peer,
            _service: service,
        },
        tools,
    ))
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
            command: "mcp-memory".to_string(),
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
            command: "notes-mcp".to_string(),
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
}
