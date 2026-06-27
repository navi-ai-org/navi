use anyhow::{Context, Result};
use async_trait::async_trait;
use navi_core::{
    McpConfig, McpServerConfig, Tool, ToolDefinition, ToolInvocation, ToolKind, ToolMetadata,
    ToolResult, ToolRisk,
};

use rmcp::{
    RoleClient, ServiceExt,
    model::CallToolRequestParams,
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
            Ok((connection, server_tools)) => {
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

fn tool_definition(server: &McpServerConfig, tool: &rmcp::model::Tool) -> Option<ToolDefinition> {
    let raw_description = tool
        .description
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("MCP tool `{}` from server `{}`.", tool.name, server.id));
    let description = sanitize_description(&raw_description);

    let input_schema = Value::Object((*tool.input_schema).clone());
    if let Err(e) = jsonschema::validator_for(&input_schema) {
        tracing::warn!(
            server = %server.id,
            tool = %tool.name,
            error = %e,
            "MCP tool has invalid JSON Schema; skipping tool"
        );
        return None;
    }

    Some(ToolDefinition {
        name: prefixed_tool_name(server, &tool.name),
        description,
        kind: ToolKind::Custom,
        input_schema,
        metadata: ToolMetadata::deferred("mcp", ToolRisk::Medium, &["mcp", &server.id]),
    })
}

/// Sanitize a tool description for safe display: truncate at 1024 chars,
/// strip control characters (keep newlines and tabs).
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

        // MCP output truncation at 64 KB to prevent runaway context usage.
        let output = truncate_mcp_output(output);

        Ok(ToolResult {
            invocation_id: invocation.id,
            ok,
            output,
        })
    }
}

/// Truncate MCP tool output to 64 KB, wrapping large values with a
/// `truncated` marker to prevent runaway context usage.
const MCP_OUTPUT_MAX_BYTES: usize = 64 * 1024;

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
}
