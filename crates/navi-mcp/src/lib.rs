use anyhow::{Context, Result};
use async_trait::async_trait;
use navi_core::{
    McpConfig, McpFirewallPolicy, McpServerConfig, Tool, ToolDefinition, ToolInvocation, ToolKind,
    ToolMetadata, ToolResult, ToolRisk,
};

use rmcp::{
    ClientCacheConfig, ClientLifecycleMode, ClientServiceExt, RoleClient,
    model::{
        CallToolRequestParams, CallToolResponse, ClientInfo, ProtocolVersion, RequestMetaObject,
        SubscriptionFilter,
    },
    service::{Peer, RunningService, RunningServiceCancellationToken},
    transport::TokioChildProcess,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    pub id: String,
    pub tools: Vec<String>,
    #[serde(default)]
    pub transport: String,
    #[serde(default)]
    pub legacy: bool,
    #[serde(default)]
    pub protocol_version: Option<String>,
    #[serde(default)]
    pub supported_protocol_versions: Vec<String>,
    #[serde(default)]
    pub capabilities: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_info: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Clone)]
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
    _service: RunningService<RoleClient, ClientInfo>,
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

pub async fn load_configured_mcp_servers(
    config: &McpConfig,
    allowed_servers: &[String],
) -> LoadedMcpServers {
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
    'servers: for server in config.servers.iter().filter(|server| server.enabled) {
        // Allowlist enforcement: if allowed_servers is non-empty, the server
        // id must be present.
        if !allowed_servers.is_empty() && !allowed_servers.contains(&server.id) {
            tracing::warn!(
                server = %server.id,
                "MCP server not in allowlist; skipping"
            );
            continue 'servers;
        }
        match connect_server(server).await {
            Ok((connection, mut server_info, server_tools)) => {
                let connection = Arc::new(connection);
                let mut tool_names = Vec::new();
                for remote_tool in server_tools {
                    let Some(definition) = tool_definition(server, &remote_tool) else {
                        continue;
                    };
                    tool_names.push(definition.name.clone());
                    tools.push(Arc::new(McpTool {
                        definition,
                        remote_name: remote_tool.name.to_string(),
                        server_id: connection.server_id.clone(),
                        peer: connection.peer.clone(),
                        timeout_ms: server.timeout_ms.unwrap_or(30_000),
                    }) as Arc<dyn Tool>);
                }
                server_info.tools = tool_names;
                servers.push(server_info);
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

fn mcp_client_lifecycle() -> ClientLifecycleMode {
    ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        legacy_version: Some(ProtocolVersion::V_2025_11_25),
    }
}

async fn connect_server(
    server: &McpServerConfig,
) -> Result<(McpConnection, McpServerInfo, Vec<rmcp::model::Tool>)> {
    let server_id = server.id.clone();
    let timeout = Duration::from_millis(server.timeout_ms.unwrap_or(30_000));

    // Outer deadline covers the whole spawn → serve → list pipeline. Any
    // sub-step that hangs (spawn pre-exec, MCP handshake, JSON-RPC read) is
    // bounded by this single timer so a misbehaving server can't stall NAVI
    // startup indefinitely.
    let connect = async {
        match (&server.url, &server.command) {
            (Some(_), Some(_)) => {
                anyhow::bail!("MCP server `{server_id}` must define only one of `url` or `command`")
            }
            (None, None) => {
                anyhow::bail!("MCP server `{server_id}` must define either `url` or `command`")
            }
            _ => {}
        }
        if let Some(url) = &server.url {
            let url_parsed = reqwest::Url::parse(url)
                .with_context(|| format!("invalid url for MCP server `{server_id}`: {url}"))?;
            if url_parsed.path().trim_end_matches('/').ends_with("/sse") {
                tracing::warn!(
                    server = %server_id,
                    "MCP URL looks like a deprecated HTTP+SSE endpoint; prefer a Streamable HTTP `/mcp` endpoint"
                );
            }
            let transport =
                rmcp::transport::StreamableHttpClientTransport::from_uri(url_parsed.as_str());
            let client_info = ClientInfo::default();
            let service = tokio::time::timeout(
                timeout,
                client_info
                    .clone()
                    .serve_with_lifecycle(transport, mcp_client_lifecycle()),
            )
            .await
            .with_context(|| format!("timed out initializing MCP server `{server_id}`"))?
            .with_context(|| format!("failed to initialize MCP server `{server_id}`"))?;
            finish_connection(
                server_id.clone(),
                "streamable_http",
                service,
                timeout,
                client_info,
            )
            .await
        } else if let Some(cmd) = &server.command {
            let mut command = tokio::process::Command::new(cmd);
            command.args(&server.args);
            command.envs(&server.env);
            if let Some(cwd) = &server.cwd {
                command.current_dir(cwd);
            }

            // Pipe stderr instead of inheriting it. rmcp's `TokioChildProcess::new`
            // defaults to `Stdio::inherit()` for stderr, which would write MCP
            // server output (banners, warnings, CrUX/usage-stat notices, panics)
            // directly to the controlling terminal. In TUI mode the terminal is
            // in raw mode under ratatui, so any byte the MCP child writes there
            // interleaves with the rendered frame and visibly corrupts the UI.
            // Capture stderr here and route it through `tracing` instead.
            let (transport, stderr) = tokio::task::block_in_place(|| {
                TokioChildProcess::builder(command)
                    .stderr(Stdio::piped())
                    .spawn()
            })
            .with_context(|| format!("failed to spawn MCP server `{server_id}`"))?;
            if let Some(stderr) = stderr {
                spawn_stderr_drain(server_id.clone(), stderr);
            }
            let client_info = ClientInfo::default();
            let service = tokio::time::timeout(
                timeout,
                client_info
                    .clone()
                    .serve_with_lifecycle(transport, mcp_client_lifecycle()),
            )
            .await
            .with_context(|| format!("timed out initializing MCP server `{server_id}`"))?
            .with_context(|| format!("failed to initialize MCP server `{server_id}`"))?;
            finish_connection(server_id.clone(), "stdio", service, timeout, client_info).await
        } else {
            anyhow::bail!("MCP server `{server_id}` must define either `url` or `command`");
        }
    };

    tokio::time::timeout(timeout, connect)
        .await
        .with_context(|| format!("timed out connecting MCP server `{server_id}` (overall)"))?
}

fn spawn_change_subscription(
    peer: Peer<RoleClient>,
    server_id: String,
    peer_info: Option<&rmcp::model::ServerPeerInfo>,
) {
    let Some(info) = peer_info else {
        return;
    };
    if info.protocol_version < ProtocolVersion::V_2026_07_28 {
        return;
    }
    let mut filter_builder = SubscriptionFilter::builder();
    let mut wants_notifications = false;
    if info
        .capabilities
        .tools
        .as_ref()
        .and_then(|capability| capability.list_changed)
        == Some(true)
    {
        filter_builder = filter_builder.tools_list_changed();
        wants_notifications = true;
    }
    if info
        .capabilities
        .prompts
        .as_ref()
        .and_then(|capability| capability.list_changed)
        == Some(true)
    {
        filter_builder = filter_builder.prompts_list_changed();
        wants_notifications = true;
    }
    if info
        .capabilities
        .resources
        .as_ref()
        .and_then(|capability| capability.list_changed)
        == Some(true)
    {
        filter_builder = filter_builder.resources_list_changed();
        wants_notifications = true;
    }
    if !wants_notifications {
        return;
    }
    let filter = filter_builder.build();
    tokio::spawn(async move {
        let mut subscription = match peer.listen(filter).await {
            Ok(subscription) => subscription,
            Err(error) => {
                tracing::debug!(server = %server_id, error = %error, "MCP change subscription unavailable");
                return;
            }
        };
        loop {
            match subscription.next().await {
                Ok(Some(_)) => {
                    tracing::debug!(server = %server_id, "MCP change notification received")
                }
                Ok(None) => return,
                Err(error) => {
                    tracing::debug!(server = %server_id, error = %error, "MCP change subscription ended");
                    return;
                }
            }
        }
    });
}

async fn finish_connection(
    server_id: String,
    transport: &str,
    service: RunningService<RoleClient, ClientInfo>,
    timeout: Duration,
    client_info: ClientInfo,
) -> Result<(McpConnection, McpServerInfo, Vec<rmcp::model::Tool>)> {
    let peer = service.peer().clone();
    peer.set_response_cache_config(
        ClientCacheConfig::default()
            .with_default_ttl(Duration::from_secs(30))
            .with_max_ttl(Duration::from_secs(3600))
            .with_max_entries(1024)
            .with_serve_stale_on_error(false),
    )
    .await;
    let peer_info = peer.peer_info();
    spawn_change_subscription(peer.clone(), server_id.clone(), peer_info.as_deref());
    let supports_tools = peer_info
        .as_ref()
        .map(|info| info.capabilities.tools.is_some())
        .unwrap_or(true);
    let (legacy, protocol_version, capabilities, server_info, instructions) =
        if let Some(info) = peer_info.as_deref() {
            (
                info.protocol_version < ProtocolVersion::V_2026_07_28,
                Some(info.protocol_version.to_string()),
                serde_json::to_value(&info.capabilities).unwrap_or_else(|_| json!({})),
                info.server_info
                    .as_ref()
                    .and_then(|value| serde_json::to_value(value).ok()),
                info.instructions.clone(),
            )
        } else {
            (true, None, json!({}), None, None)
        };
    let supported_protocol_versions = if !legacy {
        let meta = RequestMetaObject::with_client_context(
            ProtocolVersion::V_2026_07_28,
            client_info.client_info.clone(),
            client_info.capabilities.clone(),
        );
        match tokio::time::timeout(timeout, peer.discover(meta)).await {
            Ok(Ok(result)) => result
                .supported_versions
                .into_iter()
                .map(|version| version.to_string())
                .collect(),
            Ok(Err(error)) => {
                tracing::debug!(server = %server_id, error = %error, "MCP discovery metadata refresh failed");
                Vec::new()
            }
            Err(_) => {
                tracing::debug!(server = %server_id, "MCP discovery metadata refresh timed out");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let tools = if supports_tools {
        tokio::time::timeout(timeout, peer.list_all_tools())
            .await
            .with_context(|| format!("timed out listing MCP tools for `{server_id}`"))?
            .with_context(|| format!("failed to list MCP tools for `{server_id}`"))?
    } else {
        Vec::new()
    };
    let cancel_token = service.cancellation_token();
    let info = McpServerInfo {
        id: server_id.clone(),
        tools: Vec::new(),
        transport: transport.to_string(),
        legacy,
        protocol_version,
        supported_protocol_versions,
        capabilities,
        server_info,
        instructions,
    };
    Ok((
        McpConnection {
            server_id,
            peer,
            cancel_token: Mutex::new(Some(cancel_token)),
            _service: service,
        },
        info,
        tools,
    ))
}

const MAX_MCP_SCHEMA_BYTES: usize = 256 * 1024;
const MAX_MCP_SCHEMA_DEPTH: usize = 64;

fn validate_mcp_input_schema(schema: &Value) -> std::result::Result<(), String> {
    if !schema.is_object() {
        return Err("inputSchema must be a JSON object".to_string());
    }
    if schema.to_string().len() > MAX_MCP_SCHEMA_BYTES {
        return Err(format!(
            "inputSchema exceeds the {} byte safety limit",
            MAX_MCP_SCHEMA_BYTES
        ));
    }
    if schema_depth(schema, 0) > MAX_MCP_SCHEMA_DEPTH {
        return Err(format!(
            "inputSchema exceeds the {} level nesting limit",
            MAX_MCP_SCHEMA_DEPTH
        ));
    }
    if let Some(reference) = external_schema_reference(schema) {
        return Err(format!(
            "inputSchema contains external $ref `{reference}`; only local fragment references are allowed"
        ));
    }
    jsonschema::validator_for(schema)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn schema_depth(value: &Value, depth: usize) -> usize {
    match value {
        Value::Object(object) => object
            .values()
            .map(|child| schema_depth(child, depth + 1))
            .max()
            .unwrap_or(depth),
        Value::Array(array) => array
            .iter()
            .map(|child| schema_depth(child, depth + 1))
            .max()
            .unwrap_or(depth),
        _ => depth,
    }
}

fn external_schema_reference(value: &Value) -> Option<String> {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str)
                && !reference.starts_with('#')
            {
                return Some(reference.to_string());
            }
            object.values().find_map(external_schema_reference)
        }
        Value::Array(array) => array.iter().find_map(external_schema_reference),
        _ => None,
    }
}

fn tool_definition(server: &McpServerConfig, tool: &rmcp::model::Tool) -> Option<ToolDefinition> {
    let raw_description = tool
        .description
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("MCP tool `{}` from server `{}`.", tool.name, server.id));
    let firewall = McpFirewallPolicy::inspect_tool_description(
        &server.id,
        tool.name.as_ref(),
        &raw_description,
    );
    for warning in &firewall.warnings {
        tracing::warn!(server = %server.id, tool = %tool.name, warning = %warning);
    }
    let description = firewall.sanitized_description;

    let input_schema = Value::Object((*tool.input_schema).clone());
    if let Err(error) = validate_mcp_input_schema(&input_schema) {
        tracing::warn!(
            server = %server.id,
            tool = %tool.name,
            error = %error,
            "MCP tool has an unsafe or invalid JSON Schema; skipping tool"
        );
        return None;
    }

    Some(ToolDefinition {
        name: prefixed_tool_name(server, &tool.name),
        description,
        kind: ToolKind::Custom,
        input_schema,
        metadata: ToolMetadata::deferred("mcp", ToolRisk::Medium, &["mcp", &server.id])
            .with_capability(&[&McpFirewallPolicy::capability_for_tool(
                &server.id,
                tool.name.as_ref(),
            )]),
    })
}

/// Sanitize a tool description for safe display: truncate at 1024 chars,
/// strip control characters (keep newlines and tabs).
#[cfg(test)]
fn sanitize_description(desc: &str) -> String {
    let sanitized: String = desc
        .chars()
        .filter(|&ch| {
            ch == '\n'
                || ch == '\t'
                || ch.is_alphabetic()
                || ch.is_ascii_digit()
                || ch.is_whitespace()
                || ch.is_ascii_punctuation()
        })
        .collect();
    if sanitized.len() <= 1024 {
        sanitized
    } else {
        let mut truncated: String = sanitized.chars().take(1024).collect();
        // Ensure we end at a char boundary (already guaranteed by .take() on chars)
        truncated.push_str("\n[description truncated]");
        truncated
    }
}

fn prefixed_tool_name(server: &McpServerConfig, remote_name: &str) -> String {
    let prefix = server.tool_prefix.as_deref().unwrap_or(&server.id);
    format!("{prefix}__{remote_name}")
}

/// Drains an MCP server's stderr and routes each line through `tracing`.
///
/// This is spawned on the tokio runtime and runs until the child closes its
/// stderr (EOF) or the runtime shuts down. Lines are logged at `warn` level
/// because they almost always indicate either a noisy banner from a
/// well-behaved server (chrome-devtools-mcp prints CrUX/usage-stat notices)
/// or a real misbehavior (stack traces, protocol errors). We deliberately
/// never write to stdout/stderr here so the TUI remains untouched.
///
/// We also drop trailing content without a newline: the child may exit
/// mid-line and we'd rather surface what we have than swallow it.
fn spawn_stderr_drain(server_id: String, stderr: tokio::process::ChildStderr) {
    tokio::spawn(async move {
        // `next_line` returns `Ok(None)` on EOF (child closed stderr) and
        // `Err(_)` on read failure; both stop the loop. Dropping `stderr`
        // closes the pipe. We log at `warn` because MCP server stderr almost
        // always carries either noisy banners (chrome-devtools-mcp prints
        // CrUX/usage-stat notices) or real misbehavior (stack traces,
        // protocol errors). The TUI never sees any of these bytes.
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::warn!(server = %server_id, "mcp server stderr: {line}");
        }
    });
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
        let response = tokio::time::timeout(timeout, self.peer.call_tool_once(params))
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
        let result = match response {
            CallToolResponse::Complete(result) => result,
            CallToolResponse::InputRequired(result) => {
                let input_request_keys = result
                    .input_requests
                    .as_ref()
                    .map(|requests| requests.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();
                return Ok(ToolResult {
                    invocation_id: invocation.id,
                    ok: false,
                    output: McpFirewallPolicy::wrap_output(
                        &self.server_id,
                        &self.remote_name,
                        json!({
                            "error": "MCP tool requires additional client input; NAVI cannot complete this round yet",
                            "result_type": "input_required",
                            "input_request_keys": input_request_keys,
                        }),
                    ),
                });
            }
            CallToolResponse::Task(_) => {
                return Ok(ToolResult {
                    invocation_id: invocation.id,
                    ok: false,
                    output: McpFirewallPolicy::wrap_output(
                        &self.server_id,
                        &self.remote_name,
                        json!({
                            "error": "MCP server returned a Tasks result, but Tasks support is not enabled in NAVI",
                            "result_type": "task",
                        }),
                    ),
                });
            }
            _ => {
                return Ok(ToolResult {
                    invocation_id: invocation.id,
                    ok: false,
                    output: McpFirewallPolicy::wrap_output(
                        &self.server_id,
                        &self.remote_name,
                        json!({ "error": "MCP server returned an unsupported tool result type" }),
                    ),
                });
            }
        };
        let ok = !result.is_error.unwrap_or(false);
        let output = result
            .structured_content
            .unwrap_or_else(|| serde_json::to_value(result.content).unwrap_or_else(|_| json!([])));

        // MCP output truncation and provenance/taint wrapping to prevent
        // untrusted content from entering context as trusted instructions.
        let output = McpFirewallPolicy::wrap_output(&self.server_id, &self.remote_name, output);

        Ok(ToolResult {
            invocation_id: invocation.id,
            ok,
            output,
        })
    }
}

/// Truncate MCP tool output to 64 KB, wrapping large values with a
/// `truncated` marker to prevent runaway context usage.
#[cfg(test)]
const MCP_OUTPUT_MAX_BYTES: usize = 64 * 1024;

#[cfg(test)]
fn truncate_mcp_output(value: Value) -> Value {
    let serialized = value.to_string();
    if serialized.len() <= MCP_OUTPUT_MAX_BYTES {
        return value;
    }
    let mut truncated = serialized;
    truncated.truncate(MCP_OUTPUT_MAX_BYTES);
    // Re-truncate at a char boundary so we never emit invalid UTF-8.
    while !truncated.is_char_boundary(truncated.len()) {
        truncated.pop();
    }
    truncated.push_str("\n[MCP output truncated]");
    json!({
        "truncated": true,
        "content": truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use navi_core::ToolExposure;
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
    fn maps_remote_tool_to_core_definition_with_metadata() {
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
        let definition = tool_definition(&server, &remote).expect("valid mcp tool");

        assert_eq!(definition.name, "n__lookup");
        assert_eq!(definition.kind, ToolKind::Custom);
        assert!(definition.description.contains("Lookup"));

        // -- Sprint P9: deferred metadata checks --
        assert_eq!(definition.metadata.namespace, "mcp");
        assert_eq!(definition.metadata.risk, ToolRisk::Medium);
        assert_eq!(definition.metadata.exposure, ToolExposure::Deferred);
        assert!(definition.metadata.tags.contains(&"mcp".to_string()));
        assert!(definition.metadata.tags.contains(&"notes".to_string()));
    }

    #[test]
    fn mcp_tool_gets_deferred_metadata() {
        let server = McpServerConfig {
            id: "fs".to_string(),
            command: Some("fs-mcp".to_string()),
            url: None,
            args: Vec::new(),
            env: Default::default(),
            cwd: None,
            enabled: true,
            tool_prefix: None,
            timeout_ms: None,
        };
        let remote = McpRemoteTool::new(
            "read_file",
            Cow::Borrowed("Read a file"),
            serde_json::Map::new(),
        );
        let def = tool_definition(&server, &remote).expect("valid mcp tool");
        assert_eq!(def.metadata.namespace, "mcp");
        assert_eq!(def.metadata.risk, ToolRisk::Medium);
        assert_eq!(def.metadata.exposure, ToolExposure::Deferred);
        assert!(def.metadata.tags.contains(&"mcp".to_string()));
        assert!(def.metadata.tags.contains(&"fs".to_string()));
    }

    #[test]
    fn invalid_mcp_schema_does_not_crash() {
        // A schema that is not a valid JSON Schema object should not panic
        // when tool_definition validates it; it logs a warning and proceeds.
        let server = McpServerConfig {
            id: "bad".to_string(),
            command: Some("bad-mcp".to_string()),
            url: None,
            args: Vec::new(),
            env: Default::default(),
            cwd: None,
            enabled: true,
            tool_prefix: None,
            timeout_ms: None,
        };
        // An empty Map is a valid JSON Schema (accepts anything).
        // Use something actually invalid: a non-JSON Schema object.
        let invalid_schema = {
            let mut m = serde_json::Map::new();
            m.insert(
                "type".to_string(),
                serde_json::Value::String("not-a-valid-type-that-json-schema-rejects".to_string()),
            );
            m.insert(
                "additionalProperties".to_string(),
                serde_json::Value::Bool(false),
            );
            m
        };
        let remote = rmcp::model::Tool::new(
            "bad_tool",
            Cow::Borrowed("A tool with a dodgy schema"),
            invalid_schema,
        );
        assert!(tool_definition(&server, &remote).is_none());
    }

    #[test]
    fn mcp_output_truncation_works() {
        // Output under 64KB is unchanged
        let small = json!({"key": "value"});
        let result = truncate_mcp_output(small.clone());
        assert_eq!(result, small);

        // Output over 64KB gets wrapped
        let large_string = "x".repeat(70 * 1024);
        let large = json!({"data": large_string});
        let truncated = truncate_mcp_output(large);
        assert_eq!(truncated["truncated"], true);
        let content = truncated["content"].as_str().unwrap();
        assert!(content.ends_with("[MCP output truncated]"));
        // Content should be roughly 64KB + suffix
        assert!(content.len() <= MCP_OUTPUT_MAX_BYTES + "\n[MCP output truncated]".len());
        // The content should still contain the original data (truncated)
        assert!(content.starts_with("{\"data\":\"x"));
    }

    #[test]
    fn sanitize_description_truncates_long_description() {
        let long = "a".repeat(2000);
        let result = sanitize_description(&long);
        assert_eq!(result.len(), 1024 + "\n[description truncated]".len());
        assert!(result.ends_with("[description truncated]"));
    }

    #[test]
    fn sanitize_description_strips_control_chars() {
        let input = "hello\x00world\x01test\nnewline\ttab";
        let result = sanitize_description(input);
        // Control chars null and SOH should be stripped
        assert_eq!(result, "helloworldtest\nnewline\ttab");
    }

    #[test]
    fn sanitize_description_preserves_short_text() {
        let input = "Simple description.";
        let result = sanitize_description(input);
        assert_eq!(result, "Simple description.");
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
        let loaded = load_configured_mcp_servers(&config, &[]).await;
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
        let loaded = load_configured_mcp_servers(&config, &[]).await;
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

    #[tokio::test]
    async fn server_allowlist_blocks_disallowed_servers() {
        // A server not in the allowlist should be skipped without attempting
        // to connect (no "nope" binary required).
        let config = McpConfig {
            enabled: true,
            servers: vec![McpServerConfig {
                id: "not-allowed".to_string(),
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
        let allowed = vec!["allowed-server".to_string()];
        let loaded = load_configured_mcp_servers(&config, &allowed).await;
        assert!(loaded.tools.is_empty());
        assert!(loaded.servers.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn server_allowlist_allows_allowed_servers() {
        // A server on the allowlist is processed normally (will fail to connect
        // since "nope" doesn't exist, but that's a connect error, not an allowlist
        // skip).
        let config = McpConfig {
            enabled: true,
            servers: vec![McpServerConfig {
                id: "allowed-server".to_string(),
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
        let allowed = vec!["allowed-server".to_string()];
        let loaded = load_configured_mcp_servers(&config, &allowed).await;
        // The server is allowed, so it will attempt to connect and fail.
        // It should still return empty tools (connect fails), but not panic.
        assert!(loaded.tools.is_empty());
        assert!(loaded.servers.is_empty());
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
        let loaded = load_configured_mcp_servers(&config, &[]).await;
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
        let loaded = load_configured_mcp_servers(&config, &[]).await;
        let elapsed = start.elapsed();

        assert!(loaded.tools.is_empty());
        assert!(loaded.servers.is_empty());
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "load_configured_mcp_servers hung on missing command (took {elapsed:?})"
        );
    }

    #[test]
    fn modern_lifecycle_prefers_latest_with_legacy_fallback() {
        match mcp_client_lifecycle() {
            ClientLifecycleMode::Auto {
                preferred_versions,
                legacy_version,
            } => {
                assert_eq!(preferred_versions, vec![ProtocolVersion::V_2026_07_28]);
                assert_eq!(legacy_version, Some(ProtocolVersion::V_2025_11_25));
            }
            _ => panic!("MCP client must use automatic modern/legacy negotiation"),
        }
    }

    #[test]
    fn rejects_external_schema_references() {
        let error = validate_mcp_input_schema(&json!({
            "type": "object",
            "$ref": "https://attacker.example/schema.json"
        }))
        .expect_err("external references must be rejected");
        assert!(error.contains("external $ref"));
    }

    #[test]
    fn rejects_overly_deep_schemas() {
        let mut schema = json!({"type": "string"});
        for _ in 0..=MAX_MCP_SCHEMA_DEPTH {
            schema = json!({"allOf": [schema]});
        }
        let error = validate_mcp_input_schema(&schema).expect_err("deep schemas must be bounded");
        assert!(error.contains("nesting limit"));
    }

    #[test]
    fn server_info_remains_backward_compatible() {
        let info: McpServerInfo = serde_json::from_value(json!({
            "id": "legacy",
            "tools": ["legacy__tool"]
        }))
        .expect("legacy server info should deserialize");
        assert_eq!(info.id, "legacy");
        assert_eq!(info.tools, vec!["legacy__tool"]);
        assert!(info.protocol_version.is_none());
        assert!(!info.legacy);
    }

    #[tokio::test]
    async fn rejects_server_with_two_transports() {
        let server = McpServerConfig {
            id: "invalid".to_string(),
            command: Some("ignored".to_string()),
            url: Some("http://127.0.0.1:1/mcp".to_string()),
            args: Vec::new(),
            env: Default::default(),
            cwd: None,
            enabled: true,
            tool_prefix: None,
            timeout_ms: Some(100),
        };
        match connect_server(&server).await {
            Ok(_) => panic!("a server cannot define both command and url"),
            Err(error) => assert!(error.to_string().contains("only one")),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn modern_stdio_fixture_negotiates_and_invokes_tool() {
        let fixture_source = r##"
use std::io::{self, BufRead, Write};

fn request_id(line: &str) -> String {
    let marker = "\"id\":";
    let Some(start) = line.find(marker) else {
        return "1".to_string();
    };
    let rest = line[start + marker.len()..].trim_start();
    if let Some(rest) = rest.strip_prefix('\"') {
        rest.split('\"').next().unwrap_or("1").to_string()
    } else {
        rest.chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<String>()
    }
}

fn response(id: &str, result: &str) -> String {
    let mut value = String::from("{\"jsonrpc\":\"2.0\",\"id\":");
    value.push_str(id);
    value.push_str(",\"result\":");
    value.push_str(result);
    value.push_str("}");
        value.push(10 as char);
    value
}

fn error_response(id: &str) -> String {
    let mut value = String::from("{\"jsonrpc\":\"2.0\",\"id\":");
    value.push_str(id);
    value.push_str(",\"error\":{\"code\":-32601,\"message\":\"Method not found\"}}");
    value.push(10 as char);
    value
}

fn main() {
    let legacy = std::env::args().any(|arg| arg == "legacy");
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.contains("\"notifications/initialized\"") {
            continue;
        }
        let id = request_id(&line);
        let output = if legacy && line.contains("\"server/discover\"") {
            error_response(&id)
        } else {
            let result = if line.contains("\"server/discover\"") {
                "{\"resultType\":\"complete\",\"supportedVersions\":[\"2026-07-28\"],\"capabilities\":{\"tools\":{}},\"ttlMs\":0,\"cacheScope\":\"private\",\"_meta\":{\"io.modelcontextprotocol/serverInfo\":{\"name\":\"fixture\",\"version\":\"1.0.0\"}}}"
            } else if line.contains("\"initialize\"") {
                "{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{\"tools\":{}},\"serverInfo\":{\"name\":\"fixture\",\"version\":\"1.0.0\"}}"
            } else if line.contains("\"tools/list\"") {
                "{\"resultType\":\"complete\",\"tools\":[{\"name\":\"echo\",\"description\":\"Echo value\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"value\":{\"type\":\"string\"}}}}],\"ttlMs\":0,\"cacheScope\":\"private\"}"
            } else if line.contains("\"tools/call\"") {
                "{\"resultType\":\"complete\",\"content\":[{\"type\":\"text\",\"text\":\"echo\"}],\"isError\":false}"
            } else {
                "{\"resultType\":\"complete\"}"
            };
            response(&id, result)
        };
        print!("{}", output);
        let _ = io::stdout().flush();
    }
}
"##;
        let temp = tempfile::tempdir().expect("fixture tempdir");
        let source_path = temp.path().join("mcp_fixture.rs");
        let executable_path = temp
            .path()
            .join(format!("mcp_fixture{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&source_path, fixture_source).expect("fixture source");
        let compile = std::process::Command::new("rustc")
            .arg("--edition=2021")
            .arg(&source_path)
            .arg("-o")
            .arg(&executable_path)
            .output();
        let compile = match compile {
            Ok(output) => output,
            Err(error) => {
                eprintln!("skipping MCP fixture: {error}");
                return;
            }
        };
        assert!(
            compile.status.success(),
            "MCP fixture failed to compile: {}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let mut smoke = std::process::Command::new(&executable_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("start MCP fixture");
        use std::io::Write as _;
        let request = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"server/discover"}}{}"#,
            '\n'
        );
        smoke
            .stdin
            .as_mut()
            .expect("fixture stdin")
            .write_all(request.as_bytes())
            .expect("write fixture request");
        drop(smoke.stdin.take());
        let smoke_output = smoke.wait_with_output().expect("wait for MCP fixture");
        assert!(
            smoke_output.status.success(),
            "MCP fixture failed: {}",
            String::from_utf8_lossy(&smoke_output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&smoke_output.stdout).contains("serverInfo"),
            "MCP fixture response: {}",
            String::from_utf8_lossy(&smoke_output.stdout)
        );

        let config = McpConfig {
            enabled: true,
            servers: vec![McpServerConfig {
                id: "fixture".to_string(),
                command: Some(executable_path.to_string_lossy().to_string()),
                url: None,
                args: Vec::new(),
                env: Default::default(),
                cwd: None,
                enabled: true,
                tool_prefix: None,
                timeout_ms: Some(10_000),
            }],
        };
        let loaded = load_configured_mcp_servers(&config, &[]).await;
        let info = loaded
            .servers
            .iter()
            .find(|server| server.id == "fixture")
            .expect("modern fixture should connect");
        assert_eq!(info.protocol_version.as_deref(), Some("2026-07-28"));
        assert!(!info.legacy);
        assert_eq!(loaded.tools.len(), 1);

        let tool = loaded.tools.first().expect("fixture tool");
        let invocation = ToolInvocation {
            id: "fixture-call".to_string(),
            tool_name: tool.definition().name,
            input: json!({"value": "ok"}),
        };
        let result = tool.invoke(invocation).await.expect("fixture call");
        assert!(result.ok);
        assert!(result.output.to_string().contains("echo"));
        loaded.shutdown();

        let mut legacy_config = config.clone();
        legacy_config.servers[0].args = vec!["legacy".to_string()];
        let legacy_loaded = load_configured_mcp_servers(&legacy_config, &[]).await;
        let legacy_info = legacy_loaded
            .servers
            .iter()
            .find(|server| server.id == "fixture")
            .expect("legacy fixture should connect through Auto fallback");
        assert_eq!(legacy_info.protocol_version.as_deref(), Some("2025-11-25"));
        assert!(legacy_info.legacy);
        assert_eq!(legacy_loaded.tools.len(), 1);
        legacy_loaded.shutdown();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn modern_streamable_http_uses_post_headers_and_invokes_tool() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("HTTP fixture bind");
        let address = listener.local_addr().expect("HTTP fixture address");
        let server = tokio::spawn(async move {
            for _ in 0..4 {
                let (mut stream, _) =
                    tokio::time::timeout(Duration::from_secs(10), listener.accept())
                        .await
                        .expect("HTTP fixture accept timeout")
                        .expect("HTTP fixture accept");
                let mut request = Vec::new();
                let mut chunk = [0_u8; 4096];
                let header_end = loop {
                    let read = stream.read(&mut chunk).await.expect("HTTP fixture read");
                    assert!(read > 0, "HTTP fixture closed before request headers");
                    request.extend_from_slice(&chunk[..read]);
                    if let Some(position) = request
                        .windows(4)
                        .position(|window| window == [13_u8, 10, 13, 10])
                    {
                        break position;
                    }
                };
                let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")?
                            .trim()
                            .parse::<usize>()
                            .ok()
                    })
                    .unwrap_or(0);
                while request.len() < header_end + 4 + content_length {
                    let read = stream
                        .read(&mut chunk)
                        .await
                        .expect("HTTP fixture body read");
                    assert!(read > 0, "HTTP fixture closed before request body");
                    request.extend_from_slice(&chunk[..read]);
                }
                assert!(headers.starts_with("post /mcp "));
                assert!(headers.contains("mcp-protocol-version: 2026-07-28"));
                let body = &request[header_end + 4..header_end + 4 + content_length];
                let message: Value = serde_json::from_slice(body).expect("MCP HTTP JSON");
                let method = message["method"].as_str().unwrap_or_default();
                assert!(headers.contains(&format!("mcp-method: {method}")));
                if method == "tools/call" {
                    assert!(headers.contains("mcp-name: echo"));
                }
                let result = match method {
                    "server/discover" => json!({
                        "resultType": "complete",
                        "supportedVersions": ["2026-07-28"],
                        "capabilities": {"tools": {}},
                        "ttlMs": 0,
                        "cacheScope": "private"
                    }),
                    "tools/list" => json!({
                        "resultType": "complete",
                        "tools": [{
                            "name": "echo",
                            "description": "Echo value",
                            "inputSchema": {"type": "object"}
                        }],
                        "ttlMs": 0,
                        "cacheScope": "private"
                    }),
                    "tools/call" => json!({
                        "resultType": "complete",
                        "content": [{"type": "text", "text": "http-echo"}],
                        "isError": false
                    }),
                    _ => json!({"resultType": "complete"}),
                };
                let response_body = json!({
                    "jsonrpc": "2.0",
                    "id": message["id"],
                    "result": result
                })
                .to_string();
                let crlf = String::from_utf8(vec![13, 10]).expect("HTTP CRLF");
                let response = format!(
                    "HTTP/1.1 200 OK{crlf}Content-Type: application/json{crlf}Content-Length: {}{crlf}Connection: close{crlf}{crlf}{}",
                    response_body.len(),
                    response_body
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("HTTP fixture response");
            }
        });

        let config = McpConfig {
            enabled: true,
            servers: vec![McpServerConfig {
                id: "http-fixture".to_string(),
                command: None,
                url: Some(format!("http://{address}/mcp")),
                args: Vec::new(),
                env: Default::default(),
                cwd: None,
                enabled: true,
                tool_prefix: None,
                timeout_ms: Some(10_000),
            }],
        };
        let loaded = load_configured_mcp_servers(&config, &[]).await;
        let info = loaded
            .servers
            .iter()
            .find(|server| server.id == "http-fixture")
            .expect("HTTP fixture should connect");
        assert_eq!(info.transport, "streamable_http");
        assert_eq!(info.protocol_version.as_deref(), Some("2026-07-28"));
        let tool = loaded.tools.first().expect("HTTP fixture tool");
        let result = tool
            .invoke(ToolInvocation {
                id: "http-call".to_string(),
                tool_name: tool.definition().name,
                input: json!({"value": "ok"}),
            })
            .await
            .expect("HTTP fixture call");
        assert!(result.ok);
        assert!(result.output.to_string().contains("http-echo"));
        loaded.shutdown();
        server.await.expect("HTTP fixture server");
    }
}
