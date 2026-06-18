use navi_sdk::{
    AgentEvent, ModelMessage, SessionSnapshot, SessionStore, save_global_config,
    session_title_from_events,
};

use crate::app::TuiApp;
use crate::chat::reset_system_context;
use crate::session::session_created_at;
use crate::state::{ChatImage, ChatMessage, ChatRole};

pub(crate) fn save_current_session(app: &mut TuiApp) {
    if app.messages.is_empty() && app.events.is_empty() {
        return;
    }
    let now = navi_sdk::current_unix_timestamp();
    let snapshot = SessionSnapshot {
        version: SessionSnapshot::CURRENT_VERSION,
        id: app.session_id.clone(),
        title: session_title_from_events(&app.events),
        project: app.project_dir.clone(),
        created_at: session_created_at(&app.session_id).unwrap_or(now),
        updated_at: now,
        events: app.events.clone(),
        memory: None,
    };
    if let Err(err) = tokio::task::block_in_place(|| app.session_store.save(&snapshot)) {
        tracing::warn!(error = %err, "failed to save session");
    }
    if app.loaded_config.config.memory.session_memory_enabled
        && let Some(summary) = &app.compact_state.summary
        && let Err(err) = tokio::task::block_in_place(|| {
            app.session_store
                .add_memory_entry(&app.project_dir, &app.session_id, summary.clone())
        })
    {
        tracing::warn!("failed to save project memory: {err:#}");
    }
    app.session_id = SessionStore::create_id();
    app.events.clear();
    app.message_action_target = None;
    app.selected_message_action = 0;
    app.expanded_tool_results.clear();
    app.hovered_chat_source = None;
}

pub(crate) fn sync_preferences_to_config(app: &mut TuiApp) {
    app.loaded_config.config.model.name = app
        .models
        .get(app.selected_model)
        .map(|m| m.name.clone())
        .unwrap_or_else(|| app.loaded_config.config.model.name.clone());
    app.loaded_config.config.model.provider = app
        .models
        .get(app.selected_model)
        .map(|m| m.provider_id.clone())
        .unwrap_or_else(|| app.loaded_config.config.model.provider.clone());
    app.loaded_config.config.skills.active = app.active_skills.clone();
    let tui = &mut app.loaded_config.config.tui;
    tui.theme = app.theme_id.config_value().to_string();
    tui.show_thinking = app.show_thinking;
    tui.full_tool_view = app.full_tool_view;
    tui.compact_tool_visible_limit = app.compact_tool_visible_limit;
    tui.thinking_level = app.thinking_level.config_value().to_string();
    tui.yolo_mode = app.yolo_mode;
}

pub(crate) fn save_preferences(app: &mut TuiApp) {
    sync_preferences_to_config(app);

    let Some(global_path) = app.loaded_config.global_config_path.clone() else {
        tracing::warn!("skipping preferences save: global config path is not resolved");
        return;
    };
    if let Err(err) = save_global_config(&global_path, &app.loaded_config.config) {
        tracing::warn!(error = %err, "failed to save preferences");
    }
}

pub(crate) fn load_session(app: &mut TuiApp, snapshot: &SessionSnapshot) {
    app.messages.clear();
    reset_system_context(app);
    app.events.clear();

    let mut tool_invocations = std::collections::HashMap::new();

    for event in &snapshot.events {
        match event {
            AgentEvent::UserTaskSubmitted {
                text,
                content_parts,
            } => {
                let mut msg = ChatMessage::new(ChatRole::User, text.clone());
                for part in content_parts.iter() {
                    if let navi_core::model::ContentPart::Image { media_type, .. } = part {
                        let mime_short = media_type
                            .strip_prefix("image/")
                            .unwrap_or(media_type)
                            .to_uppercase();
                        msg.image_labels.push(mime_short.clone());
                        msg.images.push(ChatImage {
                            label: mime_short,
                            protocol: None,
                        });
                    }
                }
                app.messages.push(msg);

                if content_parts.is_empty() {
                    app.conversation_history
                        .push(ModelMessage::user(text.clone()));
                } else {
                    app.conversation_history.push(ModelMessage::user_multimodal(
                        text.clone(),
                        content_parts.clone(),
                    ));
                }
            }
            AgentEvent::ModelOutput { text, thinking } => {
                app.messages.push(ChatMessage {
                    thinking_content: thinking.clone().unwrap_or_default(),
                    ..ChatMessage::new(ChatRole::Assistant, text.clone())
                });
                app.conversation_history
                    .push(ModelMessage::assistant_with_thinking(
                        text.clone(),
                        thinking.clone(),
                    ));
            }
            AgentEvent::ToolRequested(invocation) => {
                tool_invocations.insert(invocation.id.clone(), invocation.clone());
            }
            AgentEvent::ToolCompleted(result) => {
                if let Some(invocation) = tool_invocations.get(&result.invocation_id) {
                    app.messages.push(ChatMessage {
                        status: Some("tool result".to_string()),
                        tool_invocation: Some(invocation.clone()),
                        tool_result: Some(result.clone()),
                        ..ChatMessage::new(ChatRole::Assistant, String::new())
                    });
                }
            }
            AgentEvent::UsageReported {
                input_tokens,
                output_tokens: _,
                ..
            } => {
                app.compact_state.update_usage(*input_tokens);
            }
            _ => {}
        }
        app.events.push(event.clone());
    }

    app.scroll_offset = 0;
    app.input.clear();
    app.input_cursor = 0;
    app.message_action_target = None;
    app.selected_message_action = 0;
    app.expanded_tool_results.clear();
    app.hovered_chat_source = None;
}
