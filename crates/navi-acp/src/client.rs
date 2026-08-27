use crate::error::{AcpError, Result};
use crate::event::AcpEvent;
use crate::process::AcpProcessConfig;
use crate::transport::{InboundMessage, JsonRpcTransport};
use crate::types::{
    AuthenticateParams, CancelParams, ClientCapabilities, ContentBlock, FsCapabilities,
    ImplementationInfo, InitializeParams, InitializeResult, NewSessionParams, NewSessionResult,
    PROTOCOL_VERSION, PermissionOutcome, PromptParams, PromptResult, RequestPermissionParams,
    RequestPermissionResult, SessionNotification, SessionUpdate,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use tokio::io::AsyncBufReadExt;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

/// Policy for answering `session/request_permission` from the agent.
#[derive(Clone, Default)]
pub enum PermissionHandler {
    /// Always pick the first allow_* option, else first option, else cancel.
    #[default]
    AutoApprove,
    /// Always cancel.
    AutoReject,
    /// Custom sync callback.
    Custom(Arc<dyn Fn(RequestPermissionParams) -> PermissionOutcome + Send + Sync>),
}

impl std::fmt::Debug for PermissionHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AutoApprove => write!(f, "AutoApprove"),
            Self::AutoReject => write!(f, "AutoReject"),
            Self::Custom(_) => write!(f, "Custom(...)"),
        }
    }
}

fn resolve_permission(
    permission: &PermissionHandler,
    req: &RequestPermissionParams,
) -> PermissionOutcome {
    match permission {
        PermissionHandler::AutoApprove => {
            let allow = req
                .options
                .iter()
                .find(|o| o.kind.starts_with("allow"))
                .or_else(|| req.options.first());
            match allow {
                Some(opt) => PermissionOutcome::Selected {
                    option_id: opt.option_id.clone(),
                },
                None => PermissionOutcome::Cancelled,
            }
        }
        PermissionHandler::AutoReject => PermissionOutcome::Cancelled,
        PermissionHandler::Custom(f) => f(req.clone()),
    }
}

/// Launch options for an ACP connection.
#[derive(Debug, Clone)]
pub struct AcpConnectOptions {
    pub process: AcpProcessConfig,
    pub client_name: String,
    pub client_version: String,
    pub api_key_env: Option<String>,
    pub api_key: Option<String>,
    pub auth_method_id: Option<String>,
    pub skip_auth: bool,
    pub permission: PermissionHandler,
    /// Protocol version to request during `initialize`. Defaults to v1.
    /// Set to 2 to negotiate v2 (requires the `acp-v2` feature on `navi-acp`
    /// and the agent to support v2; falls back to v1 if the agent replies
    /// with v1).
    pub protocol_version: u32,
}

impl Default for AcpConnectOptions {
    fn default() -> Self {
        Self {
            process: AcpProcessConfig {
                command: String::new(),
                args: Vec::new(),
                env: BTreeMap::new(),
                cwd: None,
            },
            client_name: "navi".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
            api_key_env: None,
            api_key: None,
            auth_method_id: None,
            skip_auth: false,
            permission: PermissionHandler::AutoApprove,
            protocol_version: PROTOCOL_VERSION,
        }
    }
}

/// Connected ACP client ready for session methods.
pub struct AcpClient {
    transport: Arc<JsonRpcTransport>,
    events: mpsc::UnboundedReceiver<AcpEvent>,
    init: InitializeResult,
    /// Negotiated protocol version (0 = not yet initialized). Shared with the
    /// pump so `session/update` notifications route to the right decoder.
    version: Arc<AtomicU16>,
    _child: Child,
    _pump: tokio::task::JoinHandle<()>,
}

impl AcpClient {
    /// Spawns the agent, initializes, and optionally authenticates.
    pub async fn connect(opts: AcpConnectOptions) -> Result<Self> {
        let mut command = Command::new(&opts.process.command);
        command
            .args(&opts.process.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &opts.process.env {
            command.env(k, v);
        }
        if let Some(cwd) = &opts.process.cwd {
            command.current_dir(cwd);
        }

        let mut child = command.spawn().map_err(|source| AcpError::Spawn {
            command: opts.process.command.clone(),
            source,
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AcpError::Protocol("child stdin missing".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AcpError::Protocol("child stdout missing".into()))?;
        if let Some(stderr) = child.stderr.take() {
            let cmd = opts.process.command.clone();
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(command = %cmd, "acp agent stderr: {line}");
                }
            });
        }

        let permission = opts.permission.clone();
        let protocol_version = opts.protocol_version;
        let (client, init) =
            Self::from_stdio(stdout, stdin, permission, true, protocol_version).await?;
        let mut client = client;
        client._child = child;
        client.init = init;
        client.finish_handshake(&opts).await?;
        Ok(client)
    }

    /// Builds a client over mock read/write halves, then runs initialize.
    pub async fn connect_io_for_test(
        reader: impl tokio::io::AsyncRead + Unpin + Send + 'static,
        writer: impl tokio::io::AsyncWrite + Unpin + Send + 'static,
        permission: PermissionHandler,
    ) -> Result<Self> {
        let (mut client, init) =
            Self::from_stdio(reader, writer, permission, true, PROTOCOL_VERSION).await?;
        client.init = init;
        Ok(client)
    }

    /// Builds a client over mock read/write halves with a specific protocol
    /// version, then runs initialize.
    pub async fn connect_io_for_test_v2(
        reader: impl tokio::io::AsyncRead + Unpin + Send + 'static,
        writer: impl tokio::io::AsyncWrite + Unpin + Send + 'static,
        permission: PermissionHandler,
    ) -> Result<Self> {
        let (mut client, init) = Self::from_stdio(
            reader,
            writer,
            permission,
            true,
            crate::types::PROTOCOL_VERSION_V2,
        )
        .await?;
        client.init = init;
        Ok(client)
    }

    async fn from_stdio(
        reader: impl tokio::io::AsyncRead + Unpin + Send + 'static,
        writer: impl tokio::io::AsyncWrite + Unpin + Send + 'static,
        permission: PermissionHandler,
        run_initialize: bool,
        protocol_version: u32,
    ) -> Result<(Self, InitializeResult)> {
        let (transport, mut inbound) = JsonRpcTransport::new(reader, writer);
        let transport = Arc::new(transport);
        let (event_tx, events) = mpsc::unbounded_channel();
        let pump_transport = transport.clone();
        let version = Arc::new(AtomicU16::new(0));
        #[cfg(feature = "acp-v2")]
        let pump_version = version.clone();
        let pump = tokio::spawn(async move {
            while let Some(msg) = inbound.recv().await {
                match msg {
                    InboundMessage::Notification { method, params } => {
                        if method == "session/update" {
                            #[cfg(feature = "acp-v2")]
                            if u32::from(pump_version.load(Ordering::Relaxed))
                                >= crate::types::PROTOCOL_VERSION_V2
                                && let Ok(note) = serde_json::from_value::<
                                    crate::v2::UpdateSessionNotification,
                                >(params.clone())
                            {
                                let _ = event_tx.send(AcpEvent::SessionUpdateV2 {
                                    session_id: note.session_id.0.to_string(),
                                    update: note.update,
                                });
                                continue;
                            }
                            if let Ok(note) =
                                serde_json::from_value::<SessionNotification>(params.clone())
                            {
                                let _ = event_tx.send(AcpEvent::SessionUpdate {
                                    session_id: note.session_id,
                                    update: note.update,
                                });
                                continue;
                            }
                        }
                        let _ = event_tx.send(AcpEvent::Notification { method, params });
                    }
                    InboundMessage::Request { id, method, params } => {
                        if method == "session/request_permission"
                            && let Ok(req) =
                                serde_json::from_value::<RequestPermissionParams>(params.clone())
                        {
                            let outcome = resolve_permission(&permission, &req);
                            let _ = pump_transport
                                .respond(
                                    id.clone(),
                                    serde_json::to_value(RequestPermissionResult { outcome })
                                        .unwrap_or(json!({})),
                                )
                                .await;
                            let _ = event_tx.send(AcpEvent::PermissionRequired {
                                request_id: id,
                                params: req,
                            });
                            continue;
                        }
                        let _ = pump_transport
                            .respond_error(
                                id.clone(),
                                -32601,
                                format!("method not supported by navi ACP client: {method}"),
                            )
                            .await;
                        let _ = event_tx.send(AcpEvent::ServerRequest {
                            request_id: id,
                            method,
                            params,
                        });
                    }
                }
            }
            let _ = event_tx.send(AcpEvent::TransportClosed);
        });

        // Placeholder child replaced by connect(). On Windows there is no
        // `true`; spawn the shell's builtin-like no-op via cmd so the
        // `_child` field has a valid process handle even in mock tests.
        #[cfg(windows)]
        let child = Command::new("cmd")
            .args(["/C", "exit 0"])
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| AcpError::Spawn {
                command: "cmd /C exit 0".into(),
                source,
            })?;
        #[cfg(not(windows))]
        let child = Command::new("true")
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| AcpError::Spawn {
                command: "true".into(),
                source,
            })?;

        let mut client = Self {
            transport: transport.clone(),
            events,
            init: InitializeResult {
                protocol_version: PROTOCOL_VERSION,
                agent_capabilities: Default::default(),
                auth_methods: Vec::new(),
                agent_info: None,
                meta: None,
            },
            version,
            _child: child,
            _pump: pump,
        };

        let init = if run_initialize {
            let init_params = InitializeParams {
                protocol_version,
                client_capabilities: ClientCapabilities {
                    fs: Some(FsCapabilities {
                        read_text_file: false,
                        write_text_file: false,
                    }),
                    terminal: Some(false),
                },
                client_info: Some(ImplementationInfo {
                    name: "navi".into(),
                    title: Some("NAVI".into()),
                    version: Some(env!("CARGO_PKG_VERSION").into()),
                }),
            };
            let init_value = transport
                .request("initialize", Some(serde_json::to_value(init_params)?))
                .await?;
            let init: InitializeResult = serde_json::from_value(init_value)?;
            client.init = init.clone();
            client.version.store(
                init.protocol_version.min(u16::MAX as u32) as u16,
                Ordering::Relaxed,
            );
            init
        } else {
            client.init.clone()
        };

        Ok((client, init))
    }

    async fn finish_handshake(&mut self, opts: &AcpConnectOptions) -> Result<()> {
        if self.init.protocol_version != PROTOCOL_VERSION {
            tracing::warn!(
                agent_version = self.init.protocol_version,
                client_version = PROTOCOL_VERSION,
                "ACP protocol version mismatch; continuing"
            );
        }

        if opts.skip_auth || self.init.auth_methods.is_empty() {
            return Ok(());
        }

        let method_id = opts
            .auth_method_id
            .clone()
            .or_else(|| self.init.auth_methods.first().map(|m| m.id.clone()))
            .ok_or_else(|| AcpError::Protocol("no auth method available".into()))?;

        let mut meta = BTreeMap::new();
        let api_key = opts.api_key.clone().or_else(|| {
            opts.api_key_env
                .as_ref()
                .and_then(|name| std::env::var(name).ok())
                .filter(|v| !v.is_empty())
        });
        if let Some(key) = api_key {
            meta.insert("api_key".into(), Value::String(key));
        }

        let auth = AuthenticateParams {
            method_id,
            meta: if meta.is_empty() { None } else { Some(meta) },
        };
        let _ = self
            .transport
            .request("authenticate", Some(serde_json::to_value(auth)?))
            .await?;
        Ok(())
    }

    pub fn initialize_result(&self) -> &InitializeResult {
        &self.init
    }

    /// Returns the negotiated protocol version for this connection.
    pub fn protocol_version(&self) -> u32 {
        self.init.protocol_version
    }

    /// Returns `true` if the connection negotiated v2.
    pub fn is_v2(&self) -> bool {
        self.init.protocol_version >= crate::types::PROTOCOL_VERSION_V2
    }

    pub async fn session_new(&self, cwd: impl AsRef<Path>) -> Result<NewSessionResult> {
        let params = NewSessionParams {
            cwd: cwd
                .as_ref()
                .to_str()
                .ok_or_else(|| AcpError::Protocol("cwd is not valid UTF-8".into()))?
                .to_string(),
            mcp_servers: Vec::new(),
        };
        let value = self
            .transport
            .request("session/new", Some(serde_json::to_value(params)?))
            .await?;
        Ok(serde_json::from_value(value)?)
    }

    pub async fn session_prompt(
        &self,
        session_id: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<PromptResult> {
        let params = PromptParams {
            session_id: session_id.into(),
            prompt: vec![ContentBlock::text(text)],
        };
        let value = self
            .transport
            .request("session/prompt", Some(serde_json::to_value(params)?))
            .await?;
        Ok(serde_json::from_value(value)?)
    }

    pub async fn session_cancel(&self, session_id: impl Into<String>) -> Result<()> {
        let params = CancelParams {
            session_id: session_id.into(),
        };
        self.transport
            .notify("session/cancel", Some(serde_json::to_value(params)?))
            .await
    }

    pub async fn recv_event(&mut self) -> Option<AcpEvent> {
        self.events.recv().await
    }

    pub fn try_recv_event(&mut self) -> Option<AcpEvent> {
        self.events.try_recv().ok()
    }

    pub fn drain_events(&mut self) -> Vec<AcpEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.events.try_recv() {
            out.push(ev);
        }
        out
    }

    /// Prompt and collect agent text deltas until the prompt RPC completes.
    pub async fn prompt_collect_text(
        &mut self,
        session_id: &str,
        text: &str,
    ) -> Result<(PromptResult, String)> {
        let transport = self.transport.clone();
        let params = PromptParams {
            session_id: session_id.into(),
            prompt: vec![ContentBlock::text(text)],
        };
        let params_value = serde_json::to_value(params)?;
        let prompt_fut = async move {
            let value = transport
                .request("session/prompt", Some(params_value))
                .await?;
            Ok::<PromptResult, AcpError>(serde_json::from_value(value)?)
        };
        tokio::pin!(prompt_fut);

        let mut collected = String::new();
        loop {
            tokio::select! {
                result = &mut prompt_fut => {
                    while let Ok(ev) = self.events.try_recv() {
                        if let AcpEvent::SessionUpdate {
                            update: SessionUpdate::AgentMessageChunk { content, .. },
                            ..
                        } = ev
                            && let Some(t) = content.as_text() {
                                collected.push_str(t);
                            }
                    }
                    return Ok((result?, collected));
                }
                ev = self.events.recv() => {
                    match ev {
                        Some(AcpEvent::SessionUpdate {
                            update: SessionUpdate::AgentMessageChunk { content, .. },
                            ..
                        }) => {
                            if let Some(t) = content.as_text() {
                                collected.push_str(t);
                            }
                        }
                        Some(AcpEvent::TransportClosed) | None => {
                            return Err(AcpError::TransportClosed);
                        }
                        Some(_) => {}
                    }
                }
            }
        }
    }

    /// Send a v2 `session/prompt` and await the immediate acknowledgment.
    ///
    /// Under v2 the prompt response does **not** end the turn; completion is
    /// delivered as a `state_update` notification to `idle`. Call
    /// [`prompt_collect_text_v2`](Self::prompt_collect_text_v2) to wait for
    /// completion while accumulating agent text.
    #[cfg(feature = "acp-v2")]
    pub async fn session_prompt_v2(
        &self,
        session_id: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<crate::v2::PromptResponse> {
        let params = crate::v2::PromptRequest::new(
            crate::v2::SessionId::new(session_id.into()),
            vec![crate::v2::ContentBlock::Text(crate::v2::TextContent::new(
                text.into(),
            ))],
        );
        let value = self
            .transport
            .request("session/prompt", Some(serde_json::to_value(params)?))
            .await?;
        Ok(serde_json::from_value(value)?)
    }

    /// v2 prompt lifecycle: send `session/prompt`, receive the empty ack, then
    /// consume `session/update` notifications until the agent transitions to
    /// `idle`. Returns the stop reason (from the idle state update) and the
    /// accumulated agent text chunks.
    #[cfg(feature = "acp-v2")]
    pub async fn prompt_collect_text_v2(
        &mut self,
        session_id: &str,
        text: &str,
    ) -> Result<(crate::v2::StopReason, String)> {
        use crate::v2::{ContentBlock as V2Block, SessionUpdate as V2Update};

        // Send the prompt and await the ack (empty result) without treating it
        // as terminal.
        let transport = self.transport.clone();
        let params = crate::v2::PromptRequest::new(
            crate::v2::SessionId::new(session_id.to_string()),
            vec![V2Block::Text(crate::v2::TextContent::new(text.to_string()))],
        );
        let params_value = serde_json::to_value(params)?;
        let ack_fut = async move {
            let value = transport
                .request("session/prompt", Some(params_value))
                .await?;
            Ok::<crate::v2::PromptResponse, AcpError>(serde_json::from_value(value)?)
        };
        tokio::pin!(ack_fut);

        let mut collected = String::new();
        let mut acked = false;
        loop {
            tokio::select! {
                result = &mut ack_fut, if !acked => {
                    // The v2 ack is an empty response; propagate transport/parse
                    // errors but do not treat it as the end of the turn.
                    result?;
                    acked = true;
                }
                ev = self.events.recv() => {
                    match ev {
                        Some(AcpEvent::SessionUpdateV2 { update, .. }) => {
                            match update {
                                V2Update::AgentMessageChunk(chunk) => {
                                    if let V2Block::Text(t) = chunk.content {
                                        collected.push_str(&t.text);
                                    }
                                }
                                V2Update::StateUpdate(crate::v2::StateUpdate::Idle(idle)) => {
                                    if let Some(reason) = idle.stop_reason {
                                        return Ok((reason, collected));
                                    }
                                }
                                _ => {}
                            }
                        }
                        Some(AcpEvent::TransportClosed) | None => {
                            return Err(AcpError::TransportClosed);
                        }
                        Some(_) => {}
                    }
                }
            }
        }
    }
}
