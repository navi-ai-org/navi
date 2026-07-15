use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use navi_sdk::{
    ContentPart, EngineDriver, ModelMessage, NaviSessionRequest, NaviTurnRequest, SessionGoal,
};
use tokio::sync::mpsc;

use crate::app::TuiApp;
use crate::dispatch::AsyncEvent;
use crate::notifications::push_diagnostic;
use crate::providers::selected_provider_label;
use crate::runtime::forward_runtime_event_to_tui_for_session;
use crate::state::{ChatMessage, ChatRole, ThinkingLevel};

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
    let last_message = initial_messages.pop();
    let user_prompt = last_message
        .as_ref()
        .map(|last| last.content.clone())
        .unwrap_or_default();
    let content_parts = last_message
        .map(|last| last.content_parts)
        .unwrap_or_default();

    let tx = app.async_sender();
    let engine = app.engine();
    let project_dir = app.project_dir.clone();
    let session_id = app.session_id.as_str().to_string();
    // block_in_place only on multi_thread runtimes (unit tests may use current_thread).
    let initial_goal = {
        let store = &app.session_store;
        let load = || {
            store
                .load(&session_id)
                .ok()
                .and_then(|snapshot| snapshot.goal)
        };
        match tokio::runtime::Handle::try_current() {
            Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(load)
            }
            _ => load(),
        }
    };
    let active_skills = app.active_skills.clone();
    let thinking_level = app.thinking_level;

    app.set_stream_task(tokio::spawn(async move {
        let result = run_sdk_turn(
            engine,
            session_id.clone(),
            project_dir,
            initial_messages,
            initial_goal,
            user_prompt,
            content_parts,
            tx.clone(),
            active_skills,
            thinking_level,
        )
        .await;
        let _ = tx.send(AsyncEvent::TurnCompletedForSession { session_id, result });
    }));
}

#[allow(clippy::too_many_arguments)]
async fn run_sdk_turn(
    engine: Arc<dyn EngineDriver>,
    session_id: String,
    project_dir: PathBuf,
    initial_messages: Vec<ModelMessage>,
    initial_goal: Option<SessionGoal>,
    user_prompt: String,
    content_parts: Vec<ContentPart>,
    tx: mpsc::UnboundedSender<AsyncEvent>,
    active_skills: Vec<String>,
    thinking_level: ThinkingLevel,
) -> std::result::Result<String, String> {
    engine
        .start_session(NaviSessionRequest {
            project_dir: Some(project_dir),
            session_id: Some(session_id.clone()),
            context_packets: Vec::new(),
            active_skills,
            initial_messages,
            initial_goal,
            ..NaviSessionRequest::default()
        })
        .await
        .map_err(|err| format!("{err:#}"))?;

    let mut events = engine
        .subscribe_events(&session_id)
        .map_err(|err| format!("{err:#}"))?;
    let turn = engine.send_turn(NaviTurnRequest {
        session_id: session_id.clone(),
        message: user_prompt,
        content_parts,
        context_packets: Vec::new(),
        thinking: Some(navi_sdk::ThinkingConfig::from(thinking_level)),
    });
    tokio::pin!(turn);

    let result = loop {
        tokio::select! {
            response = &mut turn => {
                break response.map(|response| response.text).map_err(|err| format!("{err:#}"));
            }
            event = events.recv() => {
                if let Ok(event) = event {
                    forward_runtime_event_to_tui_for_session(event, &session_id, &tx);
                }
            }
        }
    };

    while let Ok(event) = events.try_recv() {
        forward_runtime_event_to_tui_for_session(event, &session_id, &tx);
    }
    let _ = engine.snapshot_session(&session_id).await;
    result
}
