use agent_client_protocol::schema::{
    AgentCapabilities, CancelNotification, ContentBlock, ContentChunk, Implementation,
    InitializeRequest, InitializeResponse, NewSessionRequest, NewSessionResponse, PermissionOption,
    PermissionOptionKind, PromptRequest, PromptResponse, RequestPermissionOutcome,
    RequestPermissionRequest, SessionNotification, SessionUpdate, StopReason, TextContent,
    ToolCall, ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use agent_client_protocol::{
    Agent, Client, ConnectionTo, Dispatch, Result as AcpResult, Stdio, on_receive_dispatch,
    on_receive_notification, on_receive_request,
};
use anyhow::Result;
use navi_core::{
    AgentEvent, ApprovalDecision, LoadedConfig, RuntimeEvent, RuntimeEventKind, SessionStore,
    ToolInvocation, ToolResult,
};
use navi_sdk::{NaviEngine, NaviEngineBuilder, NaviSessionRequest, NaviTurnRequest};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct AcpState {
    engine: NaviEngine,
    default_project_dir: PathBuf,
    sessions: Arc<Mutex<HashMap<String, AcpSession>>>,
}

struct AcpSession {
    project_dir: PathBuf,
    sdk_started: bool,
    task: Option<ActivePrompt>,
}

struct ActivePrompt {
    cancel_tx: tokio::sync::oneshot::Sender<()>,
}

pub async fn run_acp_server(
    loaded_config: LoadedConfig,
    default_project_dir: PathBuf,
) -> Result<()> {
    let engine = NaviEngineBuilder::from_project(default_project_dir.clone())
        .loaded_config(loaded_config)
        .build()?;
    let state = AcpState {
        engine,
        default_project_dir,
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };

    Agent
        .builder()
        .name("navi")
        .on_receive_request(handle_initialize, on_receive_request!())
        .on_receive_request(
            {
                let state = state.clone();
                async move |request, responder, _connection| {
                    responder.respond(handle_new_session(state.clone(), request).await?)
                }
            },
            on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |request, responder, connection| {
                    handle_prompt(state.clone(), request, responder, connection).await
                }
            },
            on_receive_request!(),
        )
        .on_receive_notification(
            {
                let state = state.clone();
                async move |notification, _connection| {
                    handle_cancel(state.clone(), notification).await
                }
            },
            on_receive_notification!(),
        )
        .on_receive_dispatch(
            async move |message: Dispatch, cx: ConnectionTo<Client>| {
                message.respond_with_error(
                    agent_client_protocol::util::internal_error("unhandled ACP message"),
                    cx,
                )
            },
            on_receive_dispatch!(),
        )
        .connect_to(Stdio::new())
        .await
        .map_err(|e| anyhow::anyhow!("ACP server failed: {e:?}"))
}

async fn handle_initialize(
    initialize: InitializeRequest,
    responder: agent_client_protocol::Responder<InitializeResponse>,
    _connection: ConnectionTo<Client>,
) -> AcpResult<()> {
    let response = InitializeResponse::new(initialize.protocol_version)
        .agent_capabilities(AgentCapabilities::new())
        .agent_info(
            Implementation::new("navi", env!("CARGO_PKG_VERSION")).title("NAVI".to_string()),
        );
    responder.respond(response)
}

async fn handle_new_session(
    state: AcpState,
    request: NewSessionRequest,
) -> AcpResult<NewSessionResponse> {
    let cwd = if request.cwd.is_absolute() {
        request.cwd
    } else {
        state.default_project_dir.join(request.cwd)
    };
    let session_id = SessionStore::create_id().0;
    state.sessions.lock().unwrap().insert(
        session_id.clone(),
        AcpSession {
            project_dir: cwd,
            sdk_started: false,
            task: None,
        },
    );
    Ok(NewSessionResponse::new(session_id))
}

async fn handle_prompt(
    state: AcpState,
    request: PromptRequest,
    responder: agent_client_protocol::Responder<PromptResponse>,
    connection: ConnectionTo<Client>,
) -> AcpResult<()> {
    let session_id = request.session_id.0.to_string();
    let prompt = prompt_to_text(request.prompt);
    if prompt.trim().is_empty() {
        return responder.respond(PromptResponse::new(StopReason::EndTurn));
    }

    let project_dir = {
        let mut sessions = state.sessions.lock().unwrap();
        let session = sessions
            .entry(session_id.clone())
            .or_insert_with(|| AcpSession {
                project_dir: state.default_project_dir.clone(),
                sdk_started: false,
                task: None,
            });
        if session.task.is_some() {
            return responder.respond_with_error(agent_client_protocol::util::internal_error(
                "session already has an active prompt",
            ));
        }
        session.project_dir.clone()
    };

    let should_start_sdk = {
        let sessions = state.sessions.lock().unwrap();
        sessions
            .get(&session_id)
            .map(|session| !session.sdk_started)
            .unwrap_or(true)
    };
    if should_start_sdk {
        state
            .engine
            .start_session(NaviSessionRequest {
                project_dir: Some(project_dir.clone()),
                session_id: Some(session_id.clone()),
                agent_mode: None,
                context_packets: Vec::new(),
                active_skills: Vec::new(),
                initial_messages: Vec::new(),
            })
            .await
            .map_err(|error| {
                agent_client_protocol::util::internal_error(format!(
                    "failed to start NAVI runtime session: {error:#}"
                ))
            })?;
        if let Some(session) = state.sessions.lock().unwrap().get_mut(&session_id) {
            session.sdk_started = true;
        }
    }

    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
    let sessions = state.sessions.clone();
    let engine = state.engine.clone();
    let connection_for_task = connection.clone();
    let session_id_for_task = session_id.clone();

    tokio::spawn(async move {
        let run = run_prompt_task(
            engine.clone(),
            session_id_for_task.clone(),
            prompt,
            connection_for_task,
        );
        let stop_reason = tokio::select! {
            result = run => match result {
                Ok(()) => StopReason::EndTurn,
                Err(error) => {
                    let _ = send_text_update(
                        &connection,
                        &session_id_for_task,
                        format!("\n\nError: {error}"),
                        false,
                    );
                    StopReason::Refusal
                }
            },
            _ = cancel_rx => {
                let _ = engine.cancel_turn(&session_id_for_task).await;
                StopReason::Cancelled
            },
        };

        let _ = responder.respond(PromptResponse::new(stop_reason));
        if let Some(session) = sessions.lock().unwrap().get_mut(&session_id_for_task) {
            session.task = None;
        }
    });

    state
        .sessions
        .lock()
        .unwrap()
        .get_mut(&session_id)
        .unwrap()
        .task = Some(ActivePrompt { cancel_tx });
    Ok(())
}

async fn handle_cancel(state: AcpState, notification: CancelNotification) -> AcpResult<()> {
    let session_id = notification.session_id.0.to_string();
    let active = state
        .sessions
        .lock()
        .unwrap()
        .get_mut(&session_id)
        .and_then(|session| session.task.take());
    if let Some(active) = active {
        let _ = active.cancel_tx.send(());
        let _ = state.engine.cancel_turn(&session_id).await;
    }
    Ok(())
}

async fn run_prompt_task(
    engine: NaviEngine,
    acp_session_id: String,
    prompt: String,
    connection: ConnectionTo<Client>,
) -> Result<()> {
    let mut events = engine.subscribe_events(&acp_session_id)?;
    let turn = engine.send_turn(NaviTurnRequest {
        session_id: acp_session_id.clone(),
        message: prompt,
        context_packets: Vec::new(),
    });
    tokio::pin!(turn);

    let mut saw_text_delta = false;
    let final_text = loop {
        tokio::select! {
            response = &mut turn => {
                break response?.text;
            }
            event = events.recv() => {
                let Ok(event) = event else { continue };
                saw_text_delta |= forward_runtime_event(&connection, &acp_session_id, &event)?;
                if let RuntimeEventKind::ApprovalRequired(request) = &event.kind {
                    let decision = request_permission(&connection, &acp_session_id, request).await?;
                    engine.resolve_approval(&acp_session_id, decision).await?;
                }
            }
        }
    };

    while let Ok(event) = events.try_recv() {
        saw_text_delta |= forward_runtime_event(&connection, &acp_session_id, &event)?;
    }

    if !saw_text_delta && !final_text.trim().is_empty() {
        send_text_update(&connection, &acp_session_id, final_text, false)?;
    }

    engine.snapshot_session(&acp_session_id).await?;

    Ok(())
}

async fn request_permission(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    request: &navi_core::ApprovalRequest,
) -> Result<ApprovalDecision> {
    let tool_call = ToolCallUpdate::new(
        request.id.clone(),
        ToolCallUpdateFields::new()
            .title(request.summary.clone())
            .status(ToolCallStatus::Pending),
    );
    let response = connection
        .send_request(RequestPermissionRequest::new(
            session_id.to_string(),
            tool_call,
            vec![
                PermissionOption::new("allow_once", "Allow once", PermissionOptionKind::AllowOnce),
                PermissionOption::new("deny_once", "Deny", PermissionOptionKind::RejectOnce),
            ],
        ))
        .block_task()
        .await
        .map_err(acp_error_to_anyhow)?;

    match response.outcome {
        RequestPermissionOutcome::Selected(selected)
            if selected.option_id.0.as_ref() == "allow_once" =>
        {
            Ok(ApprovalDecision::Approved {
                id: request.id.clone(),
            })
        }
        _ => Ok(ApprovalDecision::Denied {
            id: request.id.clone(),
        }),
    }
}

fn forward_event(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    event: &AgentEvent,
) -> Result<bool> {
    match event {
        AgentEvent::ModelDelta { text } => {
            send_text_update(connection, session_id, text.clone(), false)?;
            Ok(true)
        }
        AgentEvent::ModelThinkingDelta { text } => {
            send_text_update(connection, session_id, text.clone(), true)?;
            Ok(false)
        }
        AgentEvent::ToolRequested(invocation) => {
            connection
                .send_notification(SessionNotification::new(
                    session_id.to_string(),
                    SessionUpdate::ToolCall(
                        ToolCall::new(invocation.id.clone(), invocation.tool_name.clone())
                            .kind(acp_tool_kind(&invocation.tool_name))
                            .status(ToolCallStatus::InProgress)
                            .raw_input(invocation.input.clone()),
                    ),
                ))
                .map_err(acp_error_to_anyhow)?;
            Ok(false)
        }
        AgentEvent::ToolCompleted(result) => {
            connection
                .send_notification(SessionNotification::new(
                    session_id.to_string(),
                    SessionUpdate::ToolCallUpdate(tool_result_update(result)),
                ))
                .map_err(acp_error_to_anyhow)?;
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn forward_runtime_event(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    event: &RuntimeEvent,
) -> Result<bool> {
    match &event.kind {
        RuntimeEventKind::AssistantDelta { text } => {
            send_text_update(connection, session_id, text.clone(), false)?;
            Ok(true)
        }
        RuntimeEventKind::AssistantThinkingDelta { text } => {
            send_text_update(connection, session_id, text.clone(), true)?;
            Ok(false)
        }
        RuntimeEventKind::ToolRequested(invocation) => {
            send_tool_requested(connection, session_id, invocation)?;
            Ok(false)
        }
        RuntimeEventKind::ToolCompleted(result) => {
            send_tool_completed(connection, session_id, result)?;
            Ok(false)
        }
        RuntimeEventKind::LegacyAgentEvent(event) => forward_event(connection, session_id, event),
        _ => Ok(false),
    }
}

fn send_tool_requested(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    invocation: &ToolInvocation,
) -> Result<()> {
    connection
        .send_notification(SessionNotification::new(
            session_id.to_string(),
            SessionUpdate::ToolCall(
                ToolCall::new(invocation.id.clone(), invocation.tool_name.clone())
                    .kind(acp_tool_kind(&invocation.tool_name))
                    .status(ToolCallStatus::InProgress)
                    .raw_input(invocation.input.clone()),
            ),
        ))
        .map_err(acp_error_to_anyhow)
}

fn send_tool_completed(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    result: &ToolResult,
) -> Result<()> {
    connection
        .send_notification(SessionNotification::new(
            session_id.to_string(),
            SessionUpdate::ToolCallUpdate(tool_result_update(result)),
        ))
        .map_err(acp_error_to_anyhow)
}

fn send_text_update(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    text: String,
    thinking: bool,
) -> Result<()> {
    let update = if thinking {
        SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        ))))
    } else {
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        ))))
    };
    connection
        .send_notification(SessionNotification::new(session_id.to_string(), update))
        .map_err(acp_error_to_anyhow)
}

fn tool_result_update(result: &ToolResult) -> ToolCallUpdate {
    let content = ToolCallContent::from(ContentBlock::Text(TextContent::new(tool_output_text(
        &result.output,
    ))));
    ToolCallUpdate::new(
        result.invocation_id.clone(),
        ToolCallUpdateFields::new()
            .status(if result.ok {
                ToolCallStatus::Completed
            } else {
                ToolCallStatus::Failed
            })
            .content(vec![content])
            .raw_output(serde_json::json!({
                "ok": result.ok,
                "output": result.output,
            })),
    )
}

fn tool_output_text(output: &serde_json::Value) -> String {
    match output {
        serde_json::Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

fn acp_tool_kind(tool_name: &str) -> ToolKind {
    match tool_name {
        "read_file" | "list_files" => ToolKind::Read,
        "write_file" | "apply_patch" => ToolKind::Edit,
        "grep" => ToolKind::Search,
        "bash" => ToolKind::Execute,
        _ => ToolKind::Other,
    }
}

fn prompt_to_text(blocks: Vec<ContentBlock>) -> String {
    blocks
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text),
            ContentBlock::ResourceLink(link) => Some(format!("Resource: {}", link.uri)),
            ContentBlock::Resource(resource) => Some(format!("Resource: {:?}", resource.resource)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn acp_error_to_anyhow(error: agent_client_protocol::Error) -> anyhow::Error {
    anyhow::anyhow!("{error:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_builtin_tools_to_acp_kinds() {
        assert_eq!(acp_tool_kind("read_file"), ToolKind::Read);
        assert_eq!(acp_tool_kind("write_file"), ToolKind::Edit);
        assert_eq!(acp_tool_kind("grep"), ToolKind::Search);
        assert_eq!(acp_tool_kind("bash"), ToolKind::Execute);
        assert_eq!(acp_tool_kind("custom"), ToolKind::Other);
    }

    #[test]
    fn extracts_text_prompt_blocks() {
        let prompt = prompt_to_text(vec![
            ContentBlock::Text(TextContent::new("first")),
            ContentBlock::Text(TextContent::new("second")),
        ]);

        assert_eq!(prompt, "first\n\nsecond");
    }

    #[test]
    fn serializes_tool_result_update() {
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!({"status": "ok"}),
        };

        let update = tool_result_update(&result);

        assert_eq!(update.tool_call_id.0.as_ref(), "call-1");
        assert_eq!(update.fields.status, Some(ToolCallStatus::Completed));
        assert!(update.fields.content.is_some());
        assert!(update.fields.raw_output.is_some());
    }
}
