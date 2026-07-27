//! Agent Client Protocol (ACP) server surface.
//!
//! `AcpServer` listens on a line-delimited JSON-RPC transport and dispatches the
//! core ACP lifecycle methods (`initialize`, `authenticate`, `session/new`,
//! `session/prompt`, `session/cancel`) to a user-supplied [`AcpAgentHandler`].
//!
//! This is intentionally generic: the crate does not hardcode any one ACP client.
//! Applications that embed NAVI (TUI, Desktop, bindings, or SDK users) supply the
//! handler and decide which ACP clients they want to support.

use crate::error::{AcpError, Result};
use crate::transport::{InboundMessage, JsonRpcTransport};
use crate::types::{
    AuthenticateParams, CancelParams, InitializeParams, InitializeResult, NewSessionParams,
    NewSessionResult, PermissionOption, PermissionOutcome, PromptParams, PromptResult,
    RequestPermissionParams, RequestPermissionResult, SessionNotification, SessionUpdate,
};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;

/// Handle passed to [`AcpAgentHandler::prompt`] so the handler can stream ACP
/// `session/update` notifications and ask the connected client for permission.
#[derive(Clone)]
pub struct AcpServerHandle {
    transport: Arc<JsonRpcTransport>,
    session_id: String,
    cancelled: Arc<AtomicBool>,
}

impl AcpServerHandle {
    pub fn new(
        transport: Arc<JsonRpcTransport>,
        session_id: String,
        cancelled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            transport,
            session_id,
            cancelled,
        }
    }

    /// ACP session id for this prompt turn.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Returns `true` when a `session/cancel` notification was received for this
    /// session. The handler should stop streaming and return as soon as possible.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Send an ACP `session/update` notification to the client.
    pub async fn send_update(&self, update: SessionUpdate) -> Result<()> {
        let note = SessionNotification {
            session_id: self.session_id.clone(),
            update,
        };
        self.transport
            .notify("session/update", Some(serde_json::to_value(note)?))
            .await
    }

    /// Ask the client to resolve a permission-style prompt and return the chosen
    /// option. If the client does not support `session/request_permission`, the
    /// outcome is treated as cancelled.
    pub async fn request_permission(
        &self,
        tool_call: Value,
        options: Vec<PermissionOption>,
    ) -> Result<PermissionOutcome> {
        let params = RequestPermissionParams {
            session_id: self.session_id.clone(),
            tool_call,
            options,
        };
        let value = self
            .transport
            .request(
                "session/request_permission",
                Some(serde_json::to_value(params)?),
            )
            .await?;
        let result: RequestPermissionResult = serde_json::from_value(value)
            .map_err(|e| AcpError::Protocol(format!("invalid permission response: {e}")))?;
        Ok(result.outcome)
    }

    /// Send a raw JSON-RPC notification to the client.
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.transport.notify(method, Some(params)).await
    }

    /// Send a raw JSON-RPC request to the client and await its response.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        self.transport.request(method, Some(params)).await
    }
}

/// Application-defined ACP agent behavior.
///
/// Implement this trait to expose NAVI (or any other runtime) as an ACP server.
/// The trait is object-safe and is invoked from an `AcpServer` run loop.
#[async_trait]
pub trait AcpAgentHandler: Send + Sync + 'static {
    /// Respond to the client's `initialize` request.
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult>;

    /// Respond to the client's `authenticate` request.
    ///
    /// The default implementation returns an empty object and accepts all clients.
    async fn authenticate(&self, _params: AuthenticateParams) -> Result<Value> {
        Ok(Value::Object(Default::default()))
    }

    /// Create a new ACP session and return its identifier.
    async fn new_session(&self, params: NewSessionParams) -> Result<NewSessionResult>;

    /// Run one prompt turn for the given ACP session, streaming updates through
    /// `handle` and returning the final stop reason.
    async fn prompt(&self, handle: AcpServerHandle, params: PromptParams) -> Result<PromptResult>;

    /// Cancel any in-flight turn for the given ACP session.
    ///
    /// The default implementation is a no-op; the server already marks the
    /// session as cancelled, which `handle.is_cancelled()` exposes.
    async fn cancel_session(&self, _session_id: &str) -> Result<()> {
        Ok(())
    }
}

/// Generic ACP server over a line-delimited JSON-RPC transport.
pub struct AcpServer<H: AcpAgentHandler> {
    handler: Arc<H>,
}

impl<H: AcpAgentHandler> AcpServer<H> {
    /// Create a new server from the given handler.
    pub fn new(handler: H) -> Self {
        Self {
            handler: Arc::new(handler),
        }
    }

    /// Run the server over the provided read/write halves until the transport
    /// closes. This future completes when the peer disconnects.
    pub async fn serve<R, W>(&self, reader: R, writer: W) -> Result<()>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (transport, mut inbound) = JsonRpcTransport::new(reader, writer);
        let transport = Arc::new(transport);
        let cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        while let Some(msg) = inbound.recv().await {
            match msg {
                InboundMessage::Request { id, method, params } => {
                    if let Err(e) = self
                        .handle_request(
                            id,
                            &method,
                            params,
                            transport.clone(),
                            cancellations.clone(),
                        )
                        .await
                    {
                        tracing::warn!(method = %method, error = %e, "ACP server request failed");
                    }
                }
                InboundMessage::Notification { method, params } => {
                    if let Err(e) = self
                        .handle_notification(&method, params, cancellations.clone())
                        .await
                    {
                        tracing::warn!(method = %method, error = %e, "ACP server notification failed");
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_request(
        &self,
        id: Value,
        method: &str,
        params: Value,
        transport: Arc<JsonRpcTransport>,
        cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    ) -> Result<()> {
        match method {
            "initialize" => {
                let params: InitializeParams = parse_params(params)?;
                match self.handler.initialize(params).await {
                    Ok(result) => transport.respond(id, serde_json::to_value(result)?).await?,
                    Err(e) => {
                        transport
                            .respond_error(id, -32603, format!("initialize failed: {e}"))
                            .await?
                    }
                }
            }
            "authenticate" => {
                let params: AuthenticateParams = parse_params(params)?;
                match self.handler.authenticate(params).await {
                    Ok(result) => transport.respond(id, result).await?,
                    Err(e) => {
                        transport
                            .respond_error(id, -32603, format!("authenticate failed: {e}"))
                            .await?
                    }
                }
            }
            "session/new" => {
                let params: NewSessionParams = parse_params(params)?;
                match self.handler.new_session(params).await {
                    Ok(result) => transport.respond(id, serde_json::to_value(result)?).await?,
                    Err(e) => {
                        transport
                            .respond_error(id, -32603, format!("session/new failed: {e}"))
                            .await?
                    }
                }
            }
            "session/prompt" => {
                let params: PromptParams = parse_params(params)?;
                let session_id = params.session_id.clone();
                let cancel = {
                    let mut c = cancellations.lock().await;
                    let flag = Arc::new(AtomicBool::new(false));
                    c.insert(session_id.clone(), flag.clone());
                    flag
                };
                let handle = AcpServerHandle::new(transport.clone(), session_id, cancel);
                let handler = self.handler.clone();
                let transport2 = transport.clone();

                tokio::spawn(async move {
                    match handler.prompt(handle, params).await {
                        Ok(result) => {
                            let value = serde_json::to_value(result).unwrap_or(Value::Null);
                            let _ = transport2.respond(id, value).await;
                        }
                        Err(e) => {
                            let _ = transport2
                                .respond_error(id, -32603, format!("prompt failed: {e}"))
                                .await;
                        }
                    }
                });
            }
            "session/cancel" => {
                let params: CancelParams = parse_params(params)?;
                {
                    let c = cancellations.lock().await;
                    if let Some(cancel) = c.get(&params.session_id) {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
                match self.handler.cancel_session(&params.session_id).await {
                    Ok(()) => transport.respond(id, Value::Null).await?,
                    Err(e) => {
                        transport
                            .respond_error(id, -32603, format!("session/cancel failed: {e}"))
                            .await?
                    }
                }
            }
            _ => {
                transport
                    .respond_error(id, -32601, format!("method not found: {method}"))
                    .await?;
            }
        }
        Ok(())
    }

    async fn handle_notification(
        &self,
        method: &str,
        params: Value,
        cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    ) -> Result<()> {
        if method == "session/cancel" {
            let params: CancelParams = parse_params(params)?;
            {
                let c = cancellations.lock().await;
                if let Some(cancel) = c.get(&params.session_id) {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            self.handler.cancel_session(&params.session_id).await?;
        }
        Ok(())
    }
}

fn parse_params<T: DeserializeOwned>(params: Value) -> Result<T> {
    if params.is_null() {
        serde_json::from_value(Value::Object(Default::default()))
    } else {
        serde_json::from_value(params)
    }
    .map_err(|e| AcpError::Protocol(format!("invalid params: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ContentBlock, ImplementationInfo, InitializeParams, InitializeResult, NewSessionParams,
        NewSessionResult, PROTOCOL_VERSION, PermissionOption, PermissionOutcome, PromptParams,
        PromptResult, SessionUpdate, StopReason,
    };
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, duplex};

    struct MockAgent;

    #[async_trait]
    impl AcpAgentHandler for MockAgent {
        async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
            Ok(InitializeResult {
                protocol_version: PROTOCOL_VERSION,
                agent_capabilities: Default::default(),
                auth_methods: Vec::new(),
                agent_info: Some(ImplementationInfo {
                    name: "mock".into(),
                    title: None,
                    version: Some("0.0.1".into()),
                }),
                meta: None,
            })
        }

        async fn new_session(&self, _params: NewSessionParams) -> Result<NewSessionResult> {
            Ok(NewSessionResult {
                session_id: "sess_mock".into(),
                extra: Default::default(),
            })
        }

        async fn prompt(
            &self,
            handle: AcpServerHandle,
            _params: PromptParams,
        ) -> Result<PromptResult> {
            handle
                .send_update(SessionUpdate::AgentMessageChunk {
                    message_id: None,
                    content: ContentBlock::text("Hello "),
                })
                .await?;
            handle
                .send_update(SessionUpdate::AgentMessageChunk {
                    message_id: None,
                    content: ContentBlock::text("world"),
                })
                .await?;
            Ok(PromptResult {
                stop_reason: StopReason::EndTurn,
                extra: Default::default(),
            })
        }
    }

    async fn write_line(w: &mut (impl AsyncWriteExt + Unpin), value: &Value) {
        let mut bytes = serde_json::to_vec(value).unwrap();
        bytes.push(b'\n');
        w.write_all(&bytes).await.unwrap();
        w.flush().await.unwrap();
    }

    #[tokio::test]
    async fn server_full_lifecycle_over_duplex() {
        let (client_read, server_write) = duplex(4096);
        let (server_read, client_write) = duplex(4096);

        let server = AcpServer::new(MockAgent);
        tokio::spawn(async move {
            let _ = server.serve(server_read, server_write).await;
        });

        let mut lines = BufReader::new(client_read).lines();
        let mut writer = client_write;

        let init_req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": { "fs": { "readTextFile": true, "writeTextFile": false }, "terminal": false },
                "clientInfo": { "name": "test-client", "version": "0.0.1" }
            }
        });
        write_line(&mut writer, &init_req).await;

        let line = tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
            .await
            .expect("init timeout")
            .expect("init read")
            .expect("init line");
        let resp: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["agentInfo"]["name"], "mock");

        let new_req = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/new",
            "params": { "cwd": "/tmp", "mcpServers": [] }
        });
        write_line(&mut writer, &new_req).await;

        let line = tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
            .await
            .expect("session/new timeout")
            .expect("session/new read")
            .expect("session/new line");
        let resp: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(resp["id"], 2);
        assert_eq!(resp["result"]["sessionId"], "sess_mock");

        let prompt_req = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/prompt",
            "params": {
                "sessionId": "sess_mock",
                "prompt": [{ "type": "text", "text": "hi" }]
            }
        });
        write_line(&mut writer, &prompt_req).await;

        let mut collected_text = String::new();
        let mut prompt_done = false;
        while let Ok(Ok(Some(line))) =
            tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line()).await
        {
            let v: Value = serde_json::from_str(&line).unwrap();
            if v.get("id").is_some() && v["id"] == 3 {
                assert_eq!(v["result"]["stopReason"], "end_turn");
                prompt_done = true;
                break;
            }
            if let Some(text) = v
                .pointer("/params/update/content/text")
                .and_then(|x| x.as_str())
            {
                collected_text.push_str(text);
            }
        }
        assert!(prompt_done, "prompt response never arrived");
        assert_eq!(collected_text, "Hello world");
    }

    #[tokio::test]
    async fn server_permission_request_roundtrip() {
        let (client_read, server_write) = duplex(4096);
        let (server_read, client_write) = duplex(4096);

        struct PermissionAgent;

        #[async_trait]
        impl AcpAgentHandler for PermissionAgent {
            async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
                Ok(InitializeResult {
                    protocol_version: PROTOCOL_VERSION,
                    agent_capabilities: Default::default(),
                    auth_methods: Vec::new(),
                    agent_info: Some(ImplementationInfo {
                        name: "perm-mock".into(),
                        title: None,
                        version: None,
                    }),
                    meta: None,
                })
            }

            async fn new_session(&self, _params: NewSessionParams) -> Result<NewSessionResult> {
                Ok(NewSessionResult {
                    session_id: "s1".into(),
                    extra: Default::default(),
                })
            }

            async fn prompt(
                &self,
                handle: AcpServerHandle,
                _params: PromptParams,
            ) -> Result<PromptResult> {
                let options = vec![
                    PermissionOption {
                        option_id: "yes".into(),
                        name: "Yes".into(),
                        kind: "allow_once".into(),
                    },
                    PermissionOption {
                        option_id: "no".into(),
                        name: "No".into(),
                        kind: "reject_once".into(),
                    },
                ];
                let outcome = handle
                    .request_permission(json!({"toolCallId": "c1"}), options)
                    .await?;
                let text = match outcome {
                    PermissionOutcome::Selected { option_id } if option_id == "yes" => "approved",
                    _ => "rejected",
                };
                handle
                    .send_update(SessionUpdate::AgentMessageChunk {
                        message_id: None,
                        content: ContentBlock::text(text),
                    })
                    .await?;
                Ok(PromptResult {
                    stop_reason: StopReason::EndTurn,
                    extra: Default::default(),
                })
            }
        }

        let server = AcpServer::new(PermissionAgent);
        tokio::spawn(async move {
            let _ = server.serve(server_read, server_write).await;
        });

        let mut lines = BufReader::new(client_read).lines();
        let mut writer = client_write;

        // initialize
        write_line(
            &mut writer,
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion": PROTOCOL_VERSION,"clientCapabilities":{},"clientInfo":{"name":"test"}}}),
        )
        .await;
        let _ = lines.next_line().await;

        // session/new
        write_line(
            &mut writer,
            &json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp"}}),
        )
        .await;
        let _ = lines.next_line().await;

        // prompt
        write_line(
            &mut writer,
            &json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"s1","prompt":[{"type":"text","text":"go"}]}}),
        )
        .await;

        // Expect a session/request_permission request from the server.
        let perm_req_line =
            tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
                .await
                .expect("permission request timeout")
                .expect("permission read")
                .expect("permission line");
        let perm_req: Value = serde_json::from_str(&perm_req_line).unwrap();
        assert_eq!(perm_req["method"], "session/request_permission");
        let perm_id = perm_req["id"].clone();

        // Approve it.
        let perm_resp = json!({
            "jsonrpc": "2.0",
            "id": perm_id,
            "result": { "outcome": { "outcome": "selected", "optionId": "yes" } }
        });
        write_line(&mut writer, &perm_resp).await;

        // Now expect the text update and the prompt response.
        let mut got_text = String::new();
        let mut prompt_done = false;
        while let Ok(Ok(Some(line))) =
            tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line()).await
        {
            let v: Value = serde_json::from_str(&line).unwrap();
            if v.get("id").is_some() && v["id"] == 3 {
                prompt_done = true;
                break;
            }
            if let Some(text) = v
                .pointer("/params/update/content/text")
                .and_then(|x| x.as_str())
            {
                got_text.push_str(text);
            }
        }
        assert!(prompt_done);
        assert_eq!(got_text, "approved");
    }

    #[tokio::test]
    async fn server_handles_unknown_method() {
        let (client_read, server_write) = duplex(4096);
        let (server_read, client_write) = duplex(4096);

        let server = AcpServer::new(MockAgent);
        tokio::spawn(async move {
            let _ = server.serve(server_read, server_write).await;
        });

        let mut lines = BufReader::new(client_read).lines();
        let mut writer = client_write;

        write_line(
            &mut writer,
            &json!({"jsonrpc":"2.0","id":99,"method":"frobnicate","params":{}}),
        )
        .await;

        let line = tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
            .await
            .expect("timeout")
            .expect("read")
            .expect("line");
        let resp: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(resp["id"], 99);
        assert_eq!(resp["error"]["code"], -32601);
    }
}
