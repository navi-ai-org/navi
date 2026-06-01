use agent_client_protocol::schema::{
    AgentCapabilities, CancelNotification, Implementation, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, StopReason,
};
use agent_client_protocol::{Client, ConnectionTo, Result as AcpResult};
use navi_sdk::{NaviSessionRequest, SessionStore};

use crate::acp::events::send_text_update;
use crate::acp::prompt_runner::run_prompt_task;
use crate::acp::schema::prompt_to_text;
use crate::acp::state::{AcpSession, AcpState, ActivePrompt};

pub(crate) async fn handle_initialize(
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

pub(crate) async fn handle_new_session(
    state: AcpState,
    request: NewSessionRequest,
) -> AcpResult<NewSessionResponse> {
    let cwd = if request.cwd.is_absolute() {
        request.cwd
    } else {
        state.default_project_dir.join(request.cwd)
    };
    let session_id = SessionStore::create_id().into_inner();
    state.with_sessions_mut(|sessions| {
        sessions.insert(
            session_id.clone(),
            AcpSession {
                project_dir: cwd,
                sdk_started: false,
                task: None,
            },
        );
    });
    Ok(NewSessionResponse::new(session_id))
}

pub(crate) async fn handle_prompt(
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
        let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
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

    let should_start_sdk = state.with_sessions(|sessions| {
        sessions
            .get(&session_id)
            .map(|session| !session.sdk_started)
            .unwrap_or(true)
    });
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
        state.with_sessions_mut(|sessions| {
            if let Some(session) = sessions.get_mut(&session_id) {
                session.sdk_started = true;
            }
        });
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
        if let Some(session) = sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&session_id_for_task)
        {
            session.task = None;
        }
    });

    state.with_sessions_mut(|sessions| {
        if let Some(session) = sessions.get_mut(&session_id) {
            session.task = Some(ActivePrompt { cancel_tx });
        }
    });
    Ok(())
}

pub(crate) async fn handle_cancel(
    state: AcpState,
    notification: CancelNotification,
) -> AcpResult<()> {
    let session_id = notification.session_id.0.to_string();
    let active = state.with_sessions_mut(|sessions| {
        sessions
            .get_mut(&session_id)
            .and_then(|session| session.task.take())
    });
    if let Some(active) = active {
        let _ = active.cancel_tx.send(());
        let _ = state.engine.cancel_turn(&session_id).await;
    }
    Ok(())
}
