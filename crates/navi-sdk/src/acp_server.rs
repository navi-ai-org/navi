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
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::broadcast;

use crate::{NaviEngine, NaviError, NaviSessionRequest, NaviTurnRequest};

/// Format a Unix timestamp (seconds) as an RFC 3339 UTC string without pulling
/// in a chrono/time dependency. Used for the v2 `session/list` `updated_at`.
#[cfg(feature = "acp-v2")]
fn rfc3339_utc(secs: u64) -> String {
    // Days since the Unix epoch → (year, month, day) via civil-from-days.
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3_600, (rem % 3_600) / 60, rem % 60);

    // Howard Hinnant's civil_from_days algorithm.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if mth <= 2 { y + 1 } else { y };

    format!("{year:04}-{mth:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Per-turn state for v2 event projection (ADR 0023 Phase 4).
///
/// Tracks stable message IDs for assistant messages and thoughts, so that
/// streamed chunks can be upserted by the client. Tool call IDs come from
/// the engine's `ToolInvocation::id`; plan IDs come from the engine's plan
/// store.
#[cfg(feature = "acp-v2")]
struct V2TurnState {
    /// Monotonic counter for generating stable message IDs within a turn.
    message_counter: u64,
    /// Current assistant message ID (lazily assigned on first delta).
    current_message_id: Option<String>,
    /// Current thought message ID (lazily assigned on first thinking delta).
    current_thought_id: Option<String>,
    /// Monotonic counter for stable plan IDs within a turn.
    plan_counter: u64,
}

#[cfg(feature = "acp-v2")]
impl V2TurnState {
    fn new() -> Self {
        Self {
            message_counter: 0,
            current_message_id: None,
            current_thought_id: None,
            plan_counter: 0,
        }
    }

    /// Get or assign the current assistant message ID.
    fn message_id(&mut self) -> String {
        if let Some(id) = &self.current_message_id {
            return id.clone();
        }
        self.message_counter += 1;
        let id = format!("msg-{}", self.message_counter);
        self.current_message_id = Some(id.clone());
        id
    }

    /// Get or assign the current thought message ID.
    fn thought_id(&mut self) -> String {
        if let Some(id) = &self.current_thought_id {
            return id.clone();
        }
        self.message_counter += 1;
        let id = format!("thought-{}", self.message_counter);
        self.current_thought_id = Some(id.clone());
        id
    }

    /// Get or assign the next stable plan ID within a turn.
    fn plan_id(&mut self) -> String {
        self.plan_counter += 1;
        format!("plan-{}", self.plan_counter)
    }
}

/// NAVI engine exposed as an ACP agent server.
#[derive(Clone)]
pub struct NaviAcpServer {
    engine: NaviEngine,
    agent_name: String,
    agent_version: String,
    app_scope: Arc<std::sync::RwLock<navi_core::AppScope>>,
}

impl NaviAcpServer {
    /// Create a server from an existing [`NaviEngine`].
    pub fn new(engine: NaviEngine) -> Self {
        Self {
            engine,
            agent_name: "navi".into(),
            agent_version: env!("CARGO_PKG_VERSION").into(),
            app_scope: Arc::new(std::sync::RwLock::new(navi_core::AppScope::none())),
        }
    }

    /// Returns the active [`AppScope`] for this ACP connection (ADR 0019).
    pub fn app_scope(&self) -> navi_core::AppScope {
        self.app_scope.read().unwrap().clone()
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

    /// Project a `RuntimeEvent` into v2 `SessionUpdate` notifications (ADR 0023
    /// Phase 4).
    ///
    /// This is the v2 equivalent of `process_event`. It uses stable message IDs
    /// from `V2TurnState` so the client can upsert streamed chunks. Tool calls
    /// use `tool_call_update` (not the v1 `tool_call` variant). Permission
    /// requests use v2 `request_permission` with a required `title`.
    #[cfg(feature = "acp-v2")]
    async fn process_event_v2(
        &self,
        handle: &AcpServerHandle,
        session_id: &str,
        event: RuntimeEvent,
        state: &mut V2TurnState,
    ) -> navi_acp::Result<()> {
        use navi_acp::v2::{
            ContentBlock as V2ContentBlock, ContentChunk, MessageId, PlanEntry, PlanEntryPriority,
            PlanEntryStatus, PlanId, PlanItems, PlanUpdate, PlanUpdateContent, SessionUpdate,
            TextContent, ToolCallId, ToolCallStatus, ToolCallUpdate,
        };

        match event.kind {
            RuntimeEventKind::AssistantDelta { text } => {
                let mid = state.message_id();
                handle
                    .send_update_v2(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                        V2ContentBlock::Text(TextContent::new(text)),
                        MessageId::new(mid),
                    )))
                    .await?;
            }
            RuntimeEventKind::AssistantThinkingDelta { text } => {
                let tid = state.thought_id();
                handle
                    .send_update_v2(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                        V2ContentBlock::Text(TextContent::new(text)),
                        MessageId::new(tid),
                    )))
                    .await?;
            }
            RuntimeEventKind::ToolRequested(inv) => {
                handle
                    .send_update_v2(SessionUpdate::ToolCallUpdate(
                        tool_call_update_v2_from_invocation(&inv, ToolCallStatus::Pending),
                    ))
                    .await?;
            }
            RuntimeEventKind::ToolStarted(inv) => {
                handle
                    .send_update_v2(SessionUpdate::ToolCallUpdate(
                        tool_call_update_v2_from_invocation(&inv, ToolCallStatus::InProgress),
                    ))
                    .await?;
            }
            RuntimeEventKind::ToolCompleted(result) => {
                let status = if result.ok {
                    ToolCallStatus::Completed
                } else {
                    ToolCallStatus::Failed
                };
                let mut update = ToolCallUpdate::new(ToolCallId::new(result.invocation_id.clone()));
                update.status = navi_acp::v2::MaybeUndefined::Value(status);
                update.raw_output = navi_acp::v2::MaybeUndefined::Value(result.output.clone());
                handle
                    .send_update_v2(SessionUpdate::ToolCallUpdate(update))
                    .await?;

                // Project a completed background `run` tool as an agent-owned
                // terminal (ADR 0023 Phase 4). The discriminator is the
                // `BackgroundCommandSnapshot` shape, which only background runs
                // emit.
                if let Some(snap) = navi_core::BackgroundCommandSnapshot::from_json(&result.output)
                {
                    use base64::Engine as _;
                    use navi_acp::v2::{
                        TerminalExitStatus, TerminalId, TerminalOutputChunk, TerminalUpdate,
                    };

                    let terminal_id = snap.task_id.clone();
                    let exit_status = TerminalExitStatus::new()
                        .exit_code(snap.exit_code.and_then(|c| u32::try_from(c).ok()));
                    handle
                        .send_update_v2(SessionUpdate::TerminalUpdate(
                            TerminalUpdate::new(TerminalId::new(terminal_id.as_str()))
                                .command(snap.command.clone())
                                .exit_status(exit_status),
                        ))
                        .await?;

                    if !snap.stdout.is_empty() {
                        let data = base64::engine::general_purpose::STANDARD
                            .encode(snap.stdout.as_bytes());
                        handle
                            .send_update_v2(SessionUpdate::TerminalOutputChunk(
                                TerminalOutputChunk::new(
                                    TerminalId::new(terminal_id.as_str()),
                                    data,
                                ),
                            ))
                            .await?;
                    }
                    if !snap.stderr.is_empty() {
                        let data = base64::engine::general_purpose::STANDARD
                            .encode(snap.stderr.as_bytes());
                        let mut meta = serde_json::Map::new();
                        meta.insert(
                            "stream".to_string(),
                            serde_json::Value::String("stderr".into()),
                        );
                        handle
                            .send_update_v2(SessionUpdate::TerminalOutputChunk(
                                TerminalOutputChunk::new(
                                    TerminalId::new(terminal_id.as_str()),
                                    data,
                                )
                                .meta(meta),
                            ))
                            .await?;
                    }
                }
            }
            RuntimeEventKind::ApprovalRequired(req) => {
                self.handle_approval_v2(handle, session_id, req).await?;
            }
            RuntimeEventKind::QuestionRequired(req) => {
                self.handle_question_v2(handle, session_id, req).await?;
            }
            RuntimeEventKind::PlanReviewRequired(req) => {
                self.handle_plan_review_v2(handle, session_id, req).await?;
            }
            RuntimeEventKind::SudoPasswordRequired(req) => {
                self.handle_sudo_password_v2(handle, session_id, req)
                    .await?;
            }
            RuntimeEventKind::TokensUpdated {
                input_tokens,
                output_tokens,
                ..
            } => {
                handle
                    .send_update_v2(SessionUpdate::UsageUpdate(navi_acp::v2::UsageUpdate::new(
                        input_tokens + output_tokens,
                        1,
                    )))
                    .await?;
            }
            RuntimeEventKind::TurnCompleted { text, .. } => {
                if !text.is_empty() {
                    let mid = state.message_id();
                    handle
                        .send_update_v2(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                            V2ContentBlock::Text(TextContent::new(text)),
                            MessageId::new(mid),
                        )))
                        .await?;
                }
            }
            RuntimeEventKind::Error { message } => {
                let mid = state.message_id();
                handle
                    .send_update_v2(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                        V2ContentBlock::Text(TextContent::new(format!("Error: {message}"))),
                        MessageId::new(mid),
                    )))
                    .await?;
            }
            RuntimeEventKind::HarnessStopped { message, .. } => {
                if !message.is_empty() {
                    let mid = state.message_id();
                    handle
                        .send_update_v2(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                            V2ContentBlock::Text(TextContent::new(message)),
                            MessageId::new(mid),
                        )))
                        .await?;
                }
            }
            RuntimeEventKind::PlanProposed { steps, .. } => {
                // NAVI's plan mode is a markdown document; project it as a
                // structured itemized plan (ADR 0023 Phase 4). Each step is
                // a pending entry with medium priority.
                let plan_id = state.plan_id();
                let entries = steps
                    .into_iter()
                    .map(|content| {
                        PlanEntry::new(content, PlanEntryPriority::Medium, PlanEntryStatus::Pending)
                    })
                    .collect::<Vec<_>>();
                let plan = PlanUpdate::new(PlanUpdateContent::Items(PlanItems::new(
                    PlanId::new(plan_id),
                    entries,
                )));
                handle
                    .send_update_v2(SessionUpdate::PlanUpdate(plan))
                    .await?;
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

    // -----------------------------------------------------------------
    // v2 permission/interaction handlers (ADR 0023 Phase 4).
    // -----------------------------------------------------------------

    #[cfg(feature = "acp-v2")]
    async fn handle_approval_v2(
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
        // v2 requires a `title`; map from the approval summary.
        let title = req.summary.clone();
        let decision = match handle.request_permission_v2(title, options, None).await {
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

    #[cfg(feature = "acp-v2")]
    async fn handle_question_v2(
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
        let title = req.question.clone();
        let response = match handle.request_permission_v2(title, options, None).await {
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

    #[cfg(feature = "acp-v2")]
    async fn handle_plan_review_v2(
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
        let title = req.title.clone();
        let decision = match handle.request_permission_v2(title, options, None).await {
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

    #[cfg(feature = "acp-v2")]
    async fn handle_sudo_password_v2(
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
        let title = format!("Sudo password required: {}", req.command_summary);
        let _ = handle.request_permission_v2(title, options, None).await;
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
                            stop_reason = StopReason::Other("turn_failed".into());
                        }
                        Err(join_err) => {
                            let text = format!("Turn task panicked: {join_err}");
                            let _ = handle
                                .send_update(SessionUpdate::AgentMessageChunk {
                                    message_id: None,
                                    content: ContentBlock::text(text),
                                })
                                .await;
                            stop_reason = StopReason::Other("turn_panicked".into());
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

    // -----------------------------------------------------------------
    // v2 handler overrides (ADR 0023). Only compiled when `acp-v2` is
    // enabled on `navi-acp`. The defaults on the trait already delegate to
    // the v1 methods, so these overrides exist only to advertise the v2
    // capability shape (session capability, agent info) correctly.
    // -----------------------------------------------------------------

    #[cfg(feature = "acp-v2")]
    async fn initialize_v2(
        &self,
        params: navi_acp::v2::InitializeRequest,
    ) -> navi_acp::Result<navi_acp::v2::InitializeResponse> {
        use navi_acp::v2::{
            AgentCapabilities, CrateProtocolVersion, Implementation, InitializeResponse,
            SessionCapabilities, attach_app_profile_meta, extract_app_profile_request,
            resolve_app_profile,
        };

        // Advertise baseline session capability ({}). NAVI supports the
        // baseline session methods: session/new, session/prompt,
        // session/cancel, session/update.
        let mut capabilities = AgentCapabilities::new().session(SessionCapabilities::new());
        let info =
            Implementation::new(self.agent_name.clone(), self.agent_version.clone()).title("NAVI");

        // Extract _meta["navi.dev"].app request if present (ADR 0018 §5)
        let raw_value = serde_json::to_value(&params).unwrap_or_default();
        if let Some(app_meta) = extract_app_profile_request(&raw_value) {
            // Evaluate requiresEngine constraint (ADR 0018 §5).
            if let Err(msg) = navi_acp::v2::evaluate_requires_engine(
                &app_meta.requires_engine,
                &self.agent_version,
            ) {
                return Err(navi_acp::AcpError::Protocol(msg));
            }
            // Resolve defaults: namespace -> id, shareCredentials -> true.
            let resolved = resolve_app_profile(&app_meta);
            let namespace = resolved
                .storage
                .as_ref()
                .and_then(|s| s.namespace.clone())
                .unwrap_or_else(|| resolved.id.clone());
            let share_credentials = resolved
                .storage
                .as_ref()
                .and_then(|s| s.share_credentials)
                .unwrap_or(true);
            // Validate app ID via AppScope (ADR 0019 §2) + namespace.
            let scope = navi_core::AppScope::with_namespace(&resolved.id, &namespace)
                .map_err(navi_acp::AcpError::Other)?
                .with_share_credentials(share_credentials);
            // Activate the scope on the engine so all stores use scoped paths.
            self.engine
                .set_app_scope(scope.clone())
                .map_err(|e| navi_acp::AcpError::Other(anyhow::Error::new(e)))?;
            if let Ok(mut guard) = self.app_scope.write() {
                *guard = scope;
            }
            // Echo the **resolved** app profile in capabilities._meta["navi.dev"].app
            if let Some(meta_map) = capabilities.meta.as_mut() {
                attach_app_profile_meta(meta_map, resolved);
            } else {
                let mut map = serde_json::Map::new();
                attach_app_profile_meta(&mut map, resolved);
                capabilities.meta = Some(map);
            }
        }

        Ok(InitializeResponse::new(CrateProtocolVersion::V2, info).capabilities(capabilities))
    }

    /// v2 `session/prompt` — immediate ack + async completion via
    /// `state_update` (ADR 0023 Phase 4).
    ///
    /// Unlike v1 (where the prompt response arrives after the turn completes),
    /// v2 returns an empty ack immediately and streams completion through a
    /// `state_update` notification transitioning to `idle` with a stop reason.
    #[cfg(feature = "acp-v2")]
    async fn prompt_v2(
        &self,
        handle: AcpServerHandle,
        params: navi_acp::v2::PromptRequest,
    ) -> navi_acp::Result<navi_acp::v2::PromptResponse> {
        use navi_acp::v2::{
            ContentBlock as V2ContentBlock, ContentChunk, IdleStateUpdate, MessageId,
            RunningStateUpdate, SessionUpdate, StateUpdate, StopReason as V2StopReason,
            TextContent,
        };

        let session_id = params.session_id.0.to_string();
        let mut message = String::new();
        for block in &params.prompt {
            if let navi_acp::ContentBlock::Text { text, .. } =
                navi_acp::v2::content_block_v2_to_v1(block)
            {
                if !message.is_empty() {
                    message.push('\n');
                }
                message.push_str(&text);
            }
        }

        // Empty prompt: ack immediately with idle + end_turn.
        if message.is_empty() {
            handle
                .send_state_update_v2(StateUpdate::Idle(
                    IdleStateUpdate::new().stop_reason(V2StopReason::EndTurn),
                ))
                .await?;
            return Ok(navi_acp::v2::PromptResponse::new());
        }

        // Signal that work is starting.
        handle
            .send_state_update_v2(StateUpdate::Running(RunningStateUpdate::new()))
            .await?;

        // Subscribe to events before spawning the turn so we don't miss any.
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

        let mut state = V2TurnState::new();
        let mut stop_reason = V2StopReason::EndTurn;

        loop {
            tokio::select! {
                result = &mut turn_fut => {
                    match result {
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => {
                            let text = format!("Turn failed: {e}");
                            let mid = state.message_id();
                            let _ = handle
                                .send_update_v2(SessionUpdate::AgentMessageChunk(
                                    ContentChunk::new(
                                        V2ContentBlock::Text(TextContent::new(text)),
                                        MessageId::new(mid),
                                    ),
                                ))
                                .await;
                            stop_reason = V2StopReason::Other("turn_failed".into());
                        }
                        Err(join_err) => {
                            let text = format!("Turn task panicked: {join_err}");
                            let mid = state.message_id();
                            let _ = handle
                                .send_update_v2(SessionUpdate::AgentMessageChunk(
                                    ContentChunk::new(
                                        V2ContentBlock::Text(TextContent::new(text)),
                                        MessageId::new(mid),
                                    ),
                                ))
                                .await;
                            stop_reason = V2StopReason::Other("turn_panicked".into());
                        }
                    }
                    break;
                }
                event = events.recv() => {
                    match event {
                        Ok(ev) => {
                            if handle.is_cancelled() {
                                let _ = self.engine.cancel_turn(&session_id).await;
                                stop_reason = V2StopReason::Cancelled;
                                break;
                            }
                            if let Err(e) = self.process_event_v2(&handle, &session_id, ev, &mut state).await {
                                tracing::warn!(error = %e, "ACP v2 event processing failed");
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }

        // Drain any remaining events.
        while let Ok(ev) = events.try_recv() {
            let _ = self
                .process_event_v2(&handle, &session_id, ev, &mut state)
                .await;
        }

        // Send the completion signal: state_update → idle with stop reason.
        handle
            .send_state_update_v2(StateUpdate::Idle(
                IdleStateUpdate::new().stop_reason(stop_reason),
            ))
            .await?;

        Ok(navi_acp::v2::PromptResponse::new())
    }

    /// v2 `session/list` — map persisted sessions to [`SessionInfo`] entries.
    ///
    /// Cursor pagination is not supported yet: the full list is returned and
    /// `nextCursor` is always `None`.
    #[cfg(feature = "acp-v2")]
    async fn list_sessions(
        &self,
        _params: navi_acp::v2::ListSessionsRequest,
    ) -> navi_acp::Result<navi_acp::v2::ListSessionsResponse> {
        use navi_acp::v2::{AbsolutePath, SessionId, SessionInfo};

        let saved = self
            .engine
            .list_saved_sessions_async()
            .await
            .map_err(Self::to_acp_error)?;

        let sessions = saved
            .into_iter()
            .map(|s| {
                SessionInfo::new(SessionId::new(s.id), AbsolutePath::new(s.project))
                    .title(s.title)
                    .updated_at(rfc3339_utc(s.updated_at))
            })
            .collect();

        Ok(navi_acp::v2::ListSessionsResponse::new(sessions))
    }

    /// v2 `session/close` — cancel any in-flight turn, then close the session.
    #[cfg(feature = "acp-v2")]
    async fn close_session(
        &self,
        params: navi_acp::v2::CloseSessionRequest,
    ) -> navi_acp::Result<navi_acp::v2::CloseSessionResponse> {
        let session_id = params.session_id.0.to_string();
        let _ = self.cancel_session(&session_id).await;
        self.engine
            .close_session(&session_id)
            .await
            .map_err(Self::to_acp_error)?;
        Ok(navi_acp::v2::CloseSessionResponse::new())
    }

    /// v2 `session/resume` — reopen a persisted session snapshot.
    ///
    /// `replayFrom`, `mcpServers` and `additionalDirectories` are accepted but
    /// not applied yet; the snapshot is reopened wholesale via
    /// [`NaviEngine::start_session_from_snapshot`].
    #[cfg(feature = "acp-v2")]
    async fn resume_session(
        &self,
        params: navi_acp::v2::ResumeSessionRequest,
    ) -> navi_acp::Result<navi_acp::v2::ResumeSessionResponse> {
        let session_id = params.session_id.0.to_string();
        let snapshot = self
            .engine
            .load_saved_session_async(&session_id)
            .await
            .map_err(Self::to_acp_error)?;
        self.engine
            .start_session_from_snapshot(&snapshot)
            .await
            .map_err(Self::to_acp_error)?;
        Ok(navi_acp::v2::ResumeSessionResponse::new())
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

/// Build a v2 `ToolCallUpdate` from a `ToolInvocation` with a given status
/// (ADR 0023 Phase 4).
#[cfg(feature = "acp-v2")]
fn tool_call_update_v2_from_invocation(
    inv: &ToolInvocation,
    status: navi_acp::v2::ToolCallStatus,
) -> navi_acp::v2::ToolCallUpdate {
    use navi_acp::v2::{MaybeUndefined, ToolCallId, ToolCallUpdate, ToolKind};

    let mut update = ToolCallUpdate::new(ToolCallId::new(inv.id.clone()));
    update.title = MaybeUndefined::Value(inv.tool_name.clone());
    update.kind = MaybeUndefined::Value(ToolKind::Other);
    update.status = MaybeUndefined::Value(status);
    update.raw_input = MaybeUndefined::Value(inv.input.clone());
    update
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
            app_scope: navi_core::app_scope::AppScope::none(),
            global_config_mtime: None,
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
    async fn navi_acp_server_v1_initialize_ignores_app_profile() {
        // ADR 0019/0023: the v1 initialize path must not inspect `_meta` or
        // activate any app scope. This is a regression guard for the
        // feature-flag-off / v1 dual-stack behaviour.
        let (engine, _temp) = test_engine();
        let server = NaviAcpServer::new(engine);

        let (client_read, server_write) = duplex(4096);
        let (server_read, client_write) = duplex(4096);
        let acp_server = AcpServer::new(server.clone());
        tokio::spawn(async move {
            let _ = acp_server.serve(server_read, server_write).await;
        });

        let mut lines = BufReader::new(client_read).lines();
        let mut writer = client_write;

        write_line(
            &mut writer,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "clientInfo": { "name": "test" },
                    "_meta": {
                        "navi.dev": {
                            "app": { "id": "organize-anything", "version": "0.3.1" }
                        }
                    }
                }
            }),
        )
        .await;
        let resp = read_line(&mut lines).await;
        assert_eq!(resp["result"]["agentInfo"]["name"], "navi");
        assert!(
            resp["result"]["capabilities"]["_meta"].is_null()
                || resp["result"]["capabilities"]["_meta"]
                    .get("navi.dev")
                    .is_none(),
            "v1 initialize must not echo an app profile: {resp}"
        );
        assert!(
            !server.app_scope().is_active(),
            "v1 initialize must not activate an app scope"
        );
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

    #[cfg(feature = "acp-v2")]
    #[tokio::test]
    async fn navi_acp_server_v2_app_profile_negotiation() {
        let (engine, _temp) = test_engine();
        let server = NaviAcpServer::new(engine);

        let (client_read, server_write) = duplex(4096);
        let (server_read, client_write) = duplex(4096);
        let acp_server = AcpServer::new(server.clone());
        tokio::spawn(async move {
            let _ = acp_server.serve(server_read, server_write).await;
        });

        let mut lines = BufReader::new(client_read).lines();
        let mut writer = client_write;

        let init_req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": 2,
                "capabilities": {},
                "info": { "name": "test-app", "version": "0.1.0" },
                "_meta": {
                    "navi.dev": {
                        "app": {
                            "id": "organize-anything",
                            "version": "0.3.1"
                        }
                    }
                }
            }
        });
        write_line(&mut writer, &init_req).await;
        let resp = read_line(&mut lines).await;
        assert_eq!(resp["result"]["protocolVersion"], 2);
        assert_eq!(
            resp["result"]["capabilities"]["_meta"]["navi.dev"]["app"]["id"]
                .as_str()
                .unwrap(),
            "organize-anything"
        );
        assert_eq!(server.app_scope().id(), Some("organize-anything"));
    }
}
