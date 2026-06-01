use std::path::PathBuf;
use std::time::Instant;

use navi_sdk::{AgentMode, ModelMessage, NaviEngine, NaviSessionRequest, NaviTurnRequest};
use tokio::sync::mpsc;

use crate::app::TuiApp;
use crate::dispatch::AsyncEvent;
use crate::notifications::push_diagnostic;
use crate::providers::selected_provider_label;
use crate::runtime::forward_runtime_event_to_tui;
use crate::state::{ChatMessage, ChatRole};

pub(crate) fn start_streaming_request(app: &mut TuiApp) {
    if !app.provider_configured {
        tracing::warn!(provider = %app.loaded_config.config.model.provider, "cannot start stream without API key");
        push_diagnostic(app, "No API key configured for selected provider.");
        app.messages.push(ChatMessage {
            status: Some("missing key".to_string()),
            ..ChatMessage::new(
                ChatRole::Assistant,
                "⚠ No API key configured. Press ctrl+m, choose a protocol, then enter its key."
                    .to_string(),
            )
        });
        return;
    }

    app.is_loading = true;
    app.loading_start = Some(Instant::now());
    tracing::info!(
        provider = %app.loaded_config.config.model.provider,
        model = %app.loaded_config.config.model.name,
        history = app.conversation_history.len(),
        "TUI model stream started"
    );

    let model_label = app.loaded_config.config.model.name.clone();
    let provider_label = selected_provider_label(app).to_string();
    app.messages.push(ChatMessage {
        model_label: Some(model_label.clone()),
        provider_label: Some(provider_label),
        status: Some("thinking".to_string()),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });

    let mut initial_messages = app.conversation_history.clone();
    let user_prompt = initial_messages
        .pop()
        .map(|last| last.content)
        .unwrap_or_default();

    let tx = app.async_sender();
    let engine = app.engine();
    let project_dir = app.project_dir.clone();
    let session_id = app.session_id.as_str().to_string();
    let agent_mode = app.selected_agent;
    let active_skills = app.active_skills.clone();

    app.set_stream_task(tokio::spawn(async move {
        let result = run_sdk_turn(
            engine,
            session_id.clone(),
            project_dir,
            agent_mode,
            initial_messages,
            user_prompt,
            tx.clone(),
            active_skills,
        )
        .await;
        let _ = tx.send(AsyncEvent::TurnCompleted(result));
    }));
}

async fn run_sdk_turn(
    engine: NaviEngine,
    session_id: String,
    project_dir: PathBuf,
    agent_mode: Option<AgentMode>,
    initial_messages: Vec<ModelMessage>,
    user_prompt: String,
    tx: mpsc::UnboundedSender<AsyncEvent>,
    active_skills: Vec<String>,
) -> std::result::Result<String, String> {
    engine
        .start_session(NaviSessionRequest {
            project_dir: Some(project_dir),
            session_id: Some(session_id.clone()),
            agent_mode,
            context_packets: Vec::new(),
            active_skills,
            initial_messages,
        })
        .await
        .map_err(|err| format!("{err:#}"))?;

    let mut events = engine
        .subscribe_events(&session_id)
        .map_err(|err| format!("{err:#}"))?;
    let turn = engine.send_turn(NaviTurnRequest {
        session_id: session_id.clone(),
        message: user_prompt,
        context_packets: Vec::new(),
    });
    tokio::pin!(turn);

    let result = loop {
        tokio::select! {
            response = &mut turn => {
                break response.map(|response| response.text).map_err(|err| format!("{err:#}"));
            }
            event = events.recv() => {
                if let Ok(event) = event {
                    forward_runtime_event_to_tui(event, &tx);
                }
            }
        }
    };

    while let Ok(event) = events.try_recv() {
        forward_runtime_event_to_tui(event, &tx);
    }
    let _ = engine.snapshot_session(&session_id).await;
    result
}
