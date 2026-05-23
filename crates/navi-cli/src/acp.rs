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
    AgentEvent, ApprovalDecision, LoadedConfig, ModelProvider, SecurityPolicy,
    SessionId as NaviSessionId, SessionRuntime, SessionSnapshot, SessionStore, Submission,
    ToolExecutor, ToolResult, resolve_provider_config,
};
use navi_openai::OpenAiProvider;
use navi_plugin_host::load_configured_plugins;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct AcpState {
    loaded_config: LoadedConfig,
    default_project_dir: PathBuf,
    sessions: Arc<Mutex<HashMap<String, AcpSession>>>,
}

struct AcpSession {
    project_dir: PathBuf,
    task: Option<ActivePrompt>,
}

struct ActivePrompt {
    cancel_tx: tokio::sync::oneshot::Sender<()>,
}

pub async fn run_acp_server(
    loaded_config: LoadedConfig,
    default_project_dir: PathBuf,
) -> Result<()> {
    let state = AcpState {
        loaded_config,
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
                task: None,
            });
        if session.task.is_some() {
            return responder.respond_with_error(agent_client_protocol::util::internal_error(
                "session already has an active prompt",
            ));
        }
        session.project_dir.clone()
    };

    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
    let sessions = state.sessions.clone();
    let loaded_config = state.loaded_config.clone();
    let connection_for_task = connection.clone();
    let session_id_for_task = session_id.clone();

    tokio::spawn(async move {
        let run = run_prompt_task(
            loaded_config,
            project_dir,
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
            _ = cancel_rx => StopReason::Cancelled,
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
    if let Some(active) = state
        .sessions
        .lock()
        .unwrap()
        .get_mut(&session_id)
        .and_then(|session| session.task.take())
    {
        let _ = active.cancel_tx.send(());
    }
    Ok(())
}

async fn run_prompt_task(
    loaded_config: LoadedConfig,
    project_dir: PathBuf,
    acp_session_id: String,
    prompt: String,
    connection: ConnectionTo<Client>,
) -> Result<()> {
    let provider = model_provider_for_config(&loaded_config)?;
    let security_policy = SecurityPolicy::new(
        project_dir.clone(),
        loaded_config.data_dir.clone(),
        loaded_config.config.security.clone(),
    )?;
    let mut tool_executor = ToolExecutor::new(security_policy.clone());
    let plugin_report = load_configured_plugins(
        &loaded_config.config.plugins,
        &security_policy,
        &mut tool_executor,
    );
    for warning in &plugin_report.warnings {
        tracing::warn!(warning = %warning, "plugin load warning");
    }
    let _loaded_plugins = plugin_report.loaded_plugins;

    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = Arc::new(navi_core::turn::TurnContext {
        model_provider: provider,
        tool_executor: Arc::new(tool_executor),
        agent_control: navi_core::agent::AgentControl::new(),
        project_dir: project_dir.clone(),
        model_name: loaded_config.config.model.name.clone(),
        event_tx: Some(event_tx),
        pending_approvals: Arc::new(Mutex::new(HashMap::new())),
        compact_state: Arc::new(tokio::sync::Mutex::new(
            navi_core::compact::CompactState::new(navi_core::config::effective_context_window(
                &loaded_config.config,
            )),
        )),
        harness_config: loaded_config.config.harness.clone(),
        include_tool_prompt_manifest: navi_core::effective_tool_prompt_manifest(
            &loaded_config.config,
        ),
        agent_mode: None,
        context_packets: Vec::new(),
    });

    let policy = navi_core::harness::select_harness_policy(&loaded_config.config);
    let runtime = SessionRuntime::spawn(ctx.clone(), policy, Vec::new(), None);
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    runtime
        .submission_tx
        .send(Submission {
            task: prompt,
            response_tx,
        })
        .map_err(|e| anyhow::anyhow!("failed to submit ACP prompt: {e}"))?;

    let mut events = Vec::new();
    let mut response_rx = response_rx;
    let final_text = loop {
        tokio::select! {
            response = &mut response_rx => {
                break response??;
            }
            Some(event) = event_rx.recv() => {
                forward_event(&connection, &acp_session_id, &event)?;
                if let AgentEvent::ApprovalRequested(request) = &event {
                    let decision = request_permission(&connection, &acp_session_id, request).await?;
                    ctx.resolve_approval(decision);
                }
                events.push(event);
            }
        }
    };

    while let Ok(event) = event_rx.try_recv() {
        forward_event(&connection, &acp_session_id, &event)?;
        events.push(event);
    }

    if !events
        .iter()
        .any(|event| matches!(event, AgentEvent::ModelDelta { .. }))
        && !final_text.trim().is_empty()
    {
        send_text_update(&connection, &acp_session_id, final_text, false)?;
    }

    let store = SessionStore::with_redaction(
        loaded_config.data_dir,
        loaded_config.config.security.redact_secrets_in_sessions,
    );
    let now = navi_core::session::current_unix_timestamp();
    store.save(&SessionSnapshot {
        id: NaviSessionId(acp_session_id),
        title: None,
        project: project_dir,
        created_at: now,
        updated_at: now,
        events,
        memory: None,
    })?;

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

fn model_provider_for_config(loaded_config: &LoadedConfig) -> Result<Arc<dyn ModelProvider>> {
    let provider_config =
        resolve_provider_config(&loaded_config.config, &loaded_config.config.model.provider)
            .ok_or_else(|| {
                anyhow::anyhow!("unknown provider {}", loaded_config.config.model.provider)
            })?;

    Ok(Arc::new(OpenAiProvider::from_provider_config(
        &provider_config,
    )?))
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
