//! NAVI as an ACP server (agent).
//!
//! `NaviAcpServer` implements the [`navi_acp::AcpAgentHandler`] trait and wires
//! ACP sessions to [`NaviEngine`] sessions. It is intentionally not hardcoded to
//! any one ACP client — the embedding app decides which ACP clients it supports.

use async_trait::async_trait;
use navi_acp::{
    AcpAgentHandler, AcpServer, AcpServerHandle, AuthenticateParams, ContentBlock,
    ImplementationInfo, InitializeParams, InitializeResult, NewSessionParams, NewSessionResult,
    PermissionOption, PermissionOutcome, PromptParams, PromptResult, SessionUpdate, StopReason,
};
use navi_core::{
    ApprovalDecision, ApprovalRequest, PlanReviewDecision, PlanReviewResponse, QuestionResponse,
    RuntimeEvent, RuntimeEventKind, SudoPasswordResponse, ToolInvocation, ToolResult,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::broadcast;

use crate::{NaviEngine, NaviError, NaviSessionRequest, NaviTurnRequest};

/// NAVI engine exposed as an ACP agent server.
#[derive(Clone)]
pub struct NaviAcpServer {
    engine: NaviEngine,
    agent_name: String,
    agent_version: String,
}

impl NaviAcpServer {
    /// Create a server from an existing [`NaviEngine`].
    pub fn new(engine: NaviEngine) -> Self {
        Self {
            engine,
            agent_name: "navi".into(),
            agent_version: env!("CARGO_PKG_VERSION").into(),
        }
    }

    /// Override the agent name/version advertised to ACP clients.
    pub fn with_agent_info(mut self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.agent_name = name.into();
        self.agent_version = version.into();
        self
    }

    /// Serve a single ACP connection over the provided read/write halves.
    pub async fn serve<R, W>(&self, reader: R, writer: W) -> Result<(), NaviError>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        AcpServer::new(self.clone())
            .serve(reader, writer)
            .await
            .map_err(|e| NaviError::from(anyhow::Error::new(e)))
    }

    /// Serve ACP over the process stdio streams.
    ///
    /// This is any ACP server sub-process mode.
    pub async fn serve_stdio(&self) -> Result<(), NaviError> {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        self.serve(stdin, stdout).await
    }

    fn to_acp_error(e: NaviError) -> navi_acp::AcpError {
        navi_acp::AcpError::Other(anyhow::Error::new(e))
    }

    async fn process_event(
        &self,
        handle: &AcpServerHandle,
        session_id: &str,
        event: RuntimeEvent,
    ) -> navi_acp::Result<()> {
        match event.kind {
            RuntimeEventKind::AssistantDelta { text } => {
                handle
                    .send_update(SessionUpdate::AgentMessageChunk {
                        message_id: None,
                        content: ContentBlock::text(text),
                    })
                    .await?;
            }
            RuntimeEventKind::AssistantThinkingDelta { text } => {
                handle
                    .send_update(SessionUpdate::AgentThoughtChunk {
                        message_id: None,
                        content: ContentBlock::text(text),
                    })
                    .await?;
            }
            RuntimeEventKind::ToolRequested(inv) => {
                handle.send_update(tool_call_from_invocation(&inv)).await?;
            }
            RuntimeEventKind::ToolStarted(inv) => {
                handle
                    .send_update(SessionUpdate::ToolCallUpdate {
                        tool_call_id: inv.id,
                        title: Some(inv.tool_name),
                        kind: None,
                        status: Some("running".into()),
                        content: Some(inv.input),
                        locations: None,
                        raw_input: None,
                        raw_output: None,
                    })
                    .await?;
            }
            RuntimeEventKind::ToolCompleted(result) => {
                handle
                    .send_update(tool_call_update_from_result(&result))
                    .await?;
            }
            RuntimeEventKind::ApprovalRequired(req) => {
                self.handle_approval(handle, session_id, req).await?;
            }
            RuntimeEventKind::QuestionRequired(req) => {
                self.handle_question(handle, session_id, req).await?;
            }
            RuntimeEventKind::PlanReviewRequired(req) => {
                self.handle_plan_review(handle, session_id, req).await?;
            }
            RuntimeEventKind::SudoPasswordRequired(req) => {
                self.handle_sudo_password(handle, session_id, req).await?;
            }
            RuntimeEventKind::TokensUpdated {
                input_tokens,
                output_tokens,
                ..
            } => {
                handle
                    .send_update(SessionUpdate::UsageUpdate {
                        used: input_tokens + output_tokens,
                        size: 1,
                        cost: None,
                    })
                    .await?;
            }
            RuntimeEventKind::TurnCompleted { text, .. } => {
                // Final assistant text is usually streamed as deltas; if not,
                // send it as a final chunk so the client has something to display.
                if !text.is_empty() {
                    handle
                        .send_update(SessionUpdate::AgentMessageChunk {
                            message_id: None,
                            content: ContentBlock::text(text),
                        })
                        .await?;
                }
            }
            RuntimeEventKind::Error { message } => {
                handle
                    .send_update(SessionUpdate::AgentMessageChunk {
                        message_id: None,
                        content: ContentBlock::text(format!("Error: {message}")),
                    })
                    .await?;
            }
            RuntimeEventKind::HarnessStopped { message, .. } => {
                if !message.is_empty() {
                    handle
                        .send_update(SessionUpdate::AgentMessageChunk {
                            message_id: None,
                            content: ContentBlock::text(message),
                        })
                        .await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_approval(
        &self,
        handle: &AcpServerHandle,
        session_id: &str,
        req: ApprovalRequest,
    ) -> navi_acp::Result<()> {
        let options = vec![
            PermissionOption {
                option_id: "allow-once".into(),
                name: "Allow".into(),
                kind: "allow_once".into(),
            },
            PermissionOption {
                option_id: "reject-once".into(),
                name: "Reject".into(),
                kind: "reject_once".into(),
            },
        ];
        let tool_call = json!({
            "approvalId": req.id,
            "summary": req.summary,
            "risk": req.risk,
        });
        let decision = match handle.request_permission(tool_call, options).await {
            Ok(PermissionOutcome::Selected { option_id }) if option_id == "allow-once" => {
                ApprovalDecision::Approved { id: req.id }
            }
            _ => ApprovalDecision::Denied { id: req.id },
        };
        self.engine
            .resolve_approval(session_id, decision)
            .await
            .map_err(Self::to_acp_error)?;
        Ok(())
    }

    async fn handle_question(
        &self,
        handle: &AcpServerHandle,
        session_id: &str,
        req: navi_core::QuestionRequest,
    ) -> navi_acp::Result<()> {
        let mut options: Vec<PermissionOption> = req
            .options
            .iter()
            .map(|o| PermissionOption {
                option_id: o.label.clone(),
                name: o.label.clone(),
                kind: "select".into(),
            })
            .collect();
        options.push(PermissionOption {
            option_id: "dismiss".into(),
            name: "Dismiss".into(),
            kind: "dismiss".into(),
        });
        let tool_call = json!({
            "questionId": req.id,
            "question": req.question,
            "multiple": req.multiple,
        });
        let response = match handle.request_permission(tool_call, options).await {
            Ok(PermissionOutcome::Selected { option_id }) if option_id == "dismiss" => {
                QuestionResponse::Dismissed { id: req.id.clone() }
            }
            Ok(PermissionOutcome::Selected { option_id }) => QuestionResponse::Answered {
                id: req.id.clone(),
                answers: vec![option_id],
            },
            _ => QuestionResponse::Dismissed { id: req.id.clone() },
        };
        self.engine
            .resolve_question(session_id, response)
            .await
            .map_err(Self::to_acp_error)?;
        Ok(())
    }

    async fn handle_plan_review(
        &self,
        handle: &AcpServerHandle,
        session_id: &str,
        req: navi_core::PlanReviewRequest,
    ) -> navi_acp::Result<()> {
        let options = vec![
            PermissionOption {
                option_id: "approve".into(),
                name: "Approve".into(),
                kind: "approve".into(),
            },
            PermissionOption {
                option_id: "request-changes".into(),
                name: "Request changes".into(),
                kind: "request_changes".into(),
            },
            PermissionOption {
                option_id: "quit".into(),
                name: "Quit".into(),
                kind: "quit".into(),
            },
        ];
        let tool_call = json!({"planId": req.plan_id, "title": req.title});
        let decision = match handle.request_permission(tool_call, options).await {
            Ok(PermissionOutcome::Selected { option_id }) if option_id == "approve" => {
                PlanReviewDecision::Approve
            }
            Ok(PermissionOutcome::Selected { option_id }) if option_id == "request-changes" => {
                PlanReviewDecision::RequestChanges
            }
            _ => PlanReviewDecision::Quit,
        };
        let response = PlanReviewResponse {
            id: req.id,
            plan_id: req.plan_id,
            decision,
            comments: Vec::new(),
            freeform: String::new(),
        };
        self.engine
            .resolve_plan_review(session_id, response)
            .await
            .map_err(Self::to_acp_error)?;
        Ok(())
    }

    async fn handle_sudo_password(
        &self,
        handle: &AcpServerHandle,
        session_id: &str,
        req: navi_core::SudoPasswordRequest,
    ) -> navi_acp::Result<()> {
        let options = vec![PermissionOption {
            option_id: "cancel".into(),
            name: "Cancel".into(),
            kind: "cancel".into(),
        }];
        let tool_call = json!({"sudoId": req.id, "command": req.command_summary});
        let _ = handle.request_permission(tool_call, options).await;
        self.engine
            .resolve_sudo_password(session_id, SudoPasswordResponse::Cancelled { id: req.id })
            .await
            .map_err(Self::to_acp_error)?;
        Ok(())
    }
}

#[async_trait]
impl AcpAgentHandler for NaviAcpServer {
    async fn initialize(&self, _params: InitializeParams) -> navi_acp::Result<InitializeResult> {
        Ok(InitializeResult {
            protocol_version: navi_acp::PROTOCOL_VERSION,
            agent_capabilities: navi_acp::AgentCapabilities::default(),
            auth_methods: Vec::new(),
            agent_info: Some(ImplementationInfo {
                name: self.agent_name.clone(),
                title: Some("NAVI".into()),
                version: Some(self.agent_version.clone()),
            }),
            meta: None,
        })
    }

    async fn authenticate(&self, _params: AuthenticateParams) -> navi_acp::Result<Value> {
        Ok(Value::Object(Default::default()))
    }

    async fn new_session(&self, params: NewSessionParams) -> navi_acp::Result<NewSessionResult> {
        let cwd = if params.cwd.is_empty() {
            self.engine.inner.project_dir.clone()
        } else {
            PathBuf::from(&params.cwd)
        };
        let info = self
            .engine
            .start_session(NaviSessionRequest {
                project_dir: Some(cwd),
                ..Default::default()
            })
            .await
            .map_err(Self::to_acp_error)?;
        Ok(NewSessionResult {
            session_id: info.id,
            extra: BTreeMap::new(),
        })
    }

    async fn prompt(
        &self,
        handle: AcpServerHandle,
        params: PromptParams,
    ) -> navi_acp::Result<PromptResult> {
        let session_id = params.session_id;
        let mut message = String::new();
        for block in &params.prompt {
            if let ContentBlock::Text { text, .. } = block {
                if !message.is_empty() {
                    message.push('\n');
                }
                message.push_str(text);
            }
        }
        if message.is_empty() {
            return Ok(PromptResult {
                stop_reason: StopReason::EndTurn,
                extra: BTreeMap::new(),
            });
        }

        let mut events = self
            .engine
            .subscribe_events(&session_id)
            .map_err(Self::to_acp_error)?;

        let engine = self.engine.clone();
        let turn_req = NaviTurnRequest {
            session_id: session_id.clone(),
            message,
            content_parts: Vec::new(),
            context_packets: Vec::new(),
            thinking: None,
        };
        let mut turn_fut = tokio::spawn(async move { engine.send_turn(turn_req).await });

        let mut stop_reason = StopReason::EndTurn;

        loop {
            tokio::select! {
                result = &mut turn_fut => {
                    match result {
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => {
                            let text = format!("Turn failed: {e}");
                            let _ = handle
                                .send_update(SessionUpdate::AgentMessageChunk {
                                    message_id: None,
                                    content: ContentBlock::text(text),
                                })
                                .await;
                            stop_reason = StopReason::Other;
                        }
                        Err(join_err) => {
                            let text = format!("Turn task panicked: {join_err}");
                            let _ = handle
                                .send_update(SessionUpdate::AgentMessageChunk {
                                    message_id: None,
                                    content: ContentBlock::text(text),
                                })
                                .await;
                            stop_reason = StopReason::Other;
                        }
                    }
                    break;
                }
                event = events.recv() => {
                    match event {
                        Ok(ev) => {
                            if handle.is_cancelled() {
                                let _ = self.engine.cancel_turn(&session_id).await;
                                stop_reason = StopReason::Cancelled;
                                break;
                            }
                            if let Err(e) = self.process_event(&handle, &session_id, ev).await {
                                tracing::warn!(error = %e, "ACP event processing failed");
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }

        while let Ok(ev) = events.try_recv() {
            let _ = self.process_event(&handle, &session_id, ev).await;
        }

        Ok(PromptResult {
            stop_reason,
            extra: BTreeMap::new(),
        })
    }

    async fn cancel_session(&self, session_id: &str) -> navi_acp::Result<()> {
        self.engine
            .cancel_turn(session_id)
            .await
            .map_err(Self::to_acp_error)?;
        Ok(())
    }
}

fn tool_call_from_invocation(inv: &ToolInvocation) -> SessionUpdate {
    SessionUpdate::ToolCall {
        tool_call_id: inv.id.clone(),
        title: Some(inv.tool_name.clone()),
        kind: Some(inv.tool_name.clone()),
        status: Some("pending".into()),
        content: Some(inv.input.clone()),
        locations: None,
        raw_input: Some(inv.input.clone()),
        raw_output: None,
    }
}

fn tool_call_update_from_result(result: &ToolResult) -> SessionUpdate {
    SessionUpdate::ToolCallUpdate {
        tool_call_id: result.invocation_id.clone(),
        title: None,
        kind: None,
        status: Some(if result.ok {
            "completed".into()
        } else {
            "failed".into()
        }),
        content: Some(result.output.clone()),
        locations: None,
        raw_input: None,
        raw_output: Some(result.output.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NaviEngineBuilder;
    use navi_acp::{AcpServer, AcpServerHandle, JsonRpcTransport};
    use navi_core::{
        LoadedConfig, NaviConfig, ProviderConfig, ProviderKind, RuntimeEvent, RuntimeEventKind,
        ToolInvocation, ToolResult,
    };
    use serde_json::Value;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, duplex};

    fn test_engine() -> (NaviEngine, tempfile::TempDir) {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let mut config = NaviConfig::default();
        config.providers.push(ProviderConfig {
            id: "test-provider".into(),
            label: "Test Provider".into(),
            description: String::new(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: "NAVI_SDK_TEST_NONEXISTENT_KEY".into(),
            base_url: Some("https://example.test/v1".into()),
            models: vec![navi_core::config::types::ProviderModelConfig {
                name: "test-model".into(),
                task_size: Some(navi_core::config::types::ModelTaskSize::Small),
                context_window_tokens: Some(8192),
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                supports_images: None,
                supports_audio: None,
                supports_video: None,
                supports_documents: None,
                tool_prompt_manifest: None,
                pricing_input_per_1m: None,
                pricing_output_per_1m: None,
                reasoning_levels: Vec::new(),
                default_reasoning_effort: None,
            }],
            ..Default::default()
        });
        config.model.provider = "test-provider".into();
        config.model.name = "test-model".into();
        config.registry.update_enabled = false;
        let loaded = LoadedConfig {
            config,
            global_config_path: Some(tempdir.path().join("config.toml")),
            project_config_path: None,
            data_dir: tempdir.path().to_path_buf(),
        };
        let engine = NaviEngineBuilder::from_project(tempdir.path())
            .loaded_config(loaded)
            .build()
            .expect("build engine");
        engine
            .set_provider_api_key("test-provider", "sk-test-key")
            .expect("set key");
        (engine, tempdir)
    }

    async fn write_line(w: &mut (impl AsyncWriteExt + Unpin), value: &Value) {
        let mut bytes = serde_json::to_vec(value).unwrap();
        bytes.push(b'\n');
        w.write_all(&bytes).await.unwrap();
        w.flush().await.unwrap();
    }

    async fn read_line(lines: &mut tokio::io::Lines<BufReader<tokio::io::DuplexStream>>) -> Value {
        let line = lines.next_line().await.expect("read").expect("line");
        serde_json::from_str(&line).unwrap()
    }

    #[tokio::test]
    async fn navi_acp_server_initialize_and_new_session() {
        let (engine, _temp) = test_engine();
        let server = NaviAcpServer::new(engine);

        let (client_read, server_write) = duplex(4096);
        let (server_read, client_write) = duplex(4096);
        let acp_server = AcpServer::new(server);
        tokio::spawn(async move {
            let _ = acp_server.serve(server_read, server_write).await;
        });

        let mut lines = BufReader::new(client_read).lines();
        let mut writer = client_write;

        write_line(
            &mut writer,
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"test"}}}),
        )
        .await;
        let resp = read_line(&mut lines).await;
        assert_eq!(resp["result"]["agentInfo"]["name"], "navi");

        write_line(
            &mut writer,
            &json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":""}}),
        )
        .await;
        let resp = read_line(&mut lines).await;
        assert!(resp["result"]["sessionId"].as_str().unwrap().len() > 0);
    }

    #[tokio::test]
    async fn navi_acp_server_maps_assistant_delta_to_update() {
        let (engine, _temp) = test_engine();
        let server = NaviAcpServer::new(engine);

        let (client_read, server_write) = duplex(4096);
        let (server_read, client_write) = duplex(4096);
        let (transport, _inbound) = JsonRpcTransport::new(server_read, server_write);
        let handle = AcpServerHandle::new(
            Arc::new(transport),
            "sess_1".into(),
            Arc::new(AtomicBool::new(false)),
        );

        let event = RuntimeEvent::new(RuntimeEventKind::AssistantDelta {
            text: "hello".into(),
        });
        server
            .process_event(&handle, "sess_1", event)
            .await
            .unwrap();

        // Keep client_write alive so the transport does not see EOF.
        let _keep = client_write;

        let mut lines = BufReader::new(client_read).lines();
        let line = tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
            .await
            .expect("timeout")
            .expect("read")
            .expect("line");
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["method"], "session/update");
        assert_eq!(
            v["params"]["update"]["content"]["text"].as_str().unwrap(),
            "hello"
        );
        assert_eq!(
            v["params"]["update"]["sessionUpdate"].as_str().unwrap(),
            "agent_message_chunk"
        );
    }

    #[tokio::test]
    async fn navi_acp_server_maps_tool_events_to_updates() {
        let (engine, _temp) = test_engine();
        let server = NaviAcpServer::new(engine);

        let (client_read, server_write) = duplex(4096);
        let (server_read, client_write) = duplex(4096);
        let (transport, _inbound) = JsonRpcTransport::new(server_read, server_write);
        let handle = AcpServerHandle::new(
            Arc::new(transport),
            "sess_1".into(),
            Arc::new(AtomicBool::new(false)),
        );

        let invocation = ToolInvocation {
            id: "call_1".into(),
            tool_name: "read".into(),
            input: serde_json::json!({"path": "/tmp/foo"}),
        };
        server
            .process_event(
                &handle,
                "sess_1",
                RuntimeEvent::new(RuntimeEventKind::ToolRequested(invocation.clone())),
            )
            .await
            .unwrap();
        server
            .process_event(
                &handle,
                "sess_1",
                RuntimeEvent::new(RuntimeEventKind::ToolCompleted(ToolResult {
                    invocation_id: "call_1".into(),
                    ok: true,
                    output: serde_json::json!({"content": "bar"}),
                })),
            )
            .await
            .unwrap();

        let _keep = client_write;

        let mut lines = BufReader::new(client_read).lines();
        let line1 = tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
            .await
            .expect("timeout")
            .expect("read")
            .expect("line");
        let v1: Value = serde_json::from_str(&line1).unwrap();
        assert_eq!(v1["params"]["update"]["sessionUpdate"], "tool_call");
        assert_eq!(v1["params"]["update"]["toolCallId"], "call_1");
        assert_eq!(v1["params"]["update"]["status"], "pending");

        let line2 = tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
            .await
            .expect("timeout")
            .expect("read")
            .expect("line");
        let v2: Value = serde_json::from_str(&line2).unwrap();
        assert_eq!(v2["params"]["update"]["sessionUpdate"], "tool_call_update");
        assert_eq!(v2["params"]["update"]["toolCallId"], "call_1");
        assert_eq!(v2["params"]["update"]["status"], "completed");
    }
}
