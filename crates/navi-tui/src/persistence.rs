use std::time::{Duration, Instant};

use navi_sdk::{
    AgentEvent, CompactState, SessionSnapshot, SessionStore, SessionUsageSnapshot,
    effective_context_window, model_messages_from_agent_events, save_global_config,
    session_title_from_events,
};

use crate::app::TuiApp;
use crate::chat::reset_system_context;
use crate::session::session_created_at;
use crate::state::{ChatImage, ChatMessage, ChatRole};

/// Debounce for mid-turn checkpoints (tool batches, streaming finalize).
/// User prompts always flush immediately so a kill mid-agent still keeps the ask.
const CHECKPOINT_DEBOUNCE: Duration = Duration::from_millis(150);

/// Save current session snapshot without destroying in-memory state.
pub(crate) fn snapshot_current_session(app: &TuiApp) {
    // Persist only when the event log has content. UI messages alone after a
    // session-id rotation must not write a ghost empty snapshot under a new id.
    if app.events.is_empty() {
        return;
    }
    let now = navi_sdk::current_unix_timestamp();
    let title = app
        .session_title
        .clone()
        .or_else(|| session_title_from_events(&app.events));
    // Prefer the in-memory goal cache so frequent checkpoints do not re-read
    // the whole session JSON on every tool completion.
    let existing_goal = app.session_goal.clone().or_else(|| {
        tokio::task::block_in_place(|| {
            app.session_store
                .load(app.session_id.as_str())
                .ok()
                .and_then(|snapshot| snapshot.goal)
        })
    });
    let snapshot = SessionSnapshot {
        version: SessionSnapshot::CURRENT_VERSION,
        id: app.session_id.clone(),
        title,
        project: app.project_dir.clone(),
        created_at: session_created_at(&app.session_id).unwrap_or(now),
        updated_at: now,
        events: app.events.clone(),
        memory: None,
        goal: existing_goal,
        usage: Some(SessionUsageSnapshot {
            input_tokens: app.usage_state.session_input_tokens,
            output_tokens: app.usage_state.session_output_tokens,
            cost_usd: app.usage_state.session_cost_usd,
            cost_known: app.usage_state.session_cost_known,
            credits_spent: app.usage_state.session_credits_spent,
            credit_unit: app.usage_state.session_credit_unit.clone(),
        }),
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
}

/// Persist immediately and cancel any pending debounced checkpoint.
///
/// Call after accepting a user prompt (before the agent turn runs) so a
/// process kill still leaves a resumable session with that prompt on disk.
pub(crate) fn checkpoint_session_now(app: &mut TuiApp) {
    app.session_checkpoint_due = None;
    snapshot_current_session(app);
}

/// Schedule a debounced checkpoint (coalesces rapid tool/model events).
pub(crate) fn schedule_session_checkpoint(app: &mut TuiApp) {
    if app.events.is_empty() {
        return;
    }
    app.session_checkpoint_due = Some(Instant::now() + CHECKPOINT_DEBOUNCE);
}

/// Flush a due debounced checkpoint. Call from the TUI event loop tick.
pub(crate) fn flush_session_checkpoint_if_due(app: &mut TuiApp) {
    let Some(due) = app.session_checkpoint_due else {
        return;
    };
    if Instant::now() < due {
        return;
    }
    checkpoint_session_now(app);
}

/// Flush any pending checkpoint and force a final write (quit / signal path).
pub(crate) fn flush_session_checkpoint(app: &mut TuiApp) {
    if app.session_checkpoint_due.is_some() || !app.events.is_empty() {
        checkpoint_session_now(app);
    }
}

/// Destructive save: writes snapshot, then creates fresh session id and clears state.
/// Used for fork/session-switch operations.
pub(crate) fn save_current_session(app: &mut TuiApp) {
    flush_session_checkpoint(app);
    app.session_id = SessionStore::create_id();
    app.events.clear();
    app.session_goal = None;
    app.session_title = None;
    app.session_checkpoint_due = None;
    app.message_action_target = None;
    // Keep selected_message_action — last Message Actions choice is a preference.
    app.expanded_tool_results.clear();
    app.collapsed_tool_results.clear();
    app.hovered_chat_source = None;
    app.selected_chat_source = None;
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
    // desktop_notifications + last_message_action are toggled/updated in place.
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

/// Save the global config to disk using the app's resolved path.
pub(crate) fn save_global_config_for_app(app: &TuiApp) -> anyhow::Result<()> {
    let Some(global_path) = app.loaded_config.global_config_path.clone() else {
        anyhow::bail!("global config path not resolved");
    };
    save_global_config(&global_path, &app.loaded_config.config)?;
    Ok(())
}

pub(crate) fn load_session(app: &mut TuiApp, snapshot: &SessionSnapshot) {
    app.messages.clear();
    app.session_id = snapshot.id.clone();
    app.session_title = snapshot.title.clone();
    app.session_goal = snapshot.goal.clone();
    app.session_checkpoint_due = None;
    reset_system_context(app);
    app.events.clear();
    app.compact_state = CompactState::new(effective_context_window(&app.loaded_config.config));
    // Restore persisted session spend / token totals.
    if let Some(usage) = &snapshot.usage {
        app.usage_state.session_input_tokens = usage.input_tokens;
        app.usage_state.session_output_tokens = usage.output_tokens;
        app.usage_state.session_cost_usd = usage.cost_usd;
        app.usage_state.session_cost_known = usage.cost_known;
        app.usage_state.session_credits_spent = usage.credits_spent;
        app.usage_state.session_credit_unit = usage.credit_unit.clone();
    } else {
        app.usage_state.session_input_tokens = 0;
        app.usage_state.session_output_tokens = 0;
        app.usage_state.session_cost_usd = 0.0;
        app.usage_state.session_cost_known = false;
        app.usage_state.session_credits_spent = None;
        app.usage_state.session_credit_unit = None;
    }
    app.usage_state.last_input_tokens = None;
    app.usage_state.last_output_tokens = None;
    app.usage_state.last_turn_label = None;
    app.usage_state.reset_request_usage();
    app.usage_state.last_account_refresh_at = None;
    app.pending_approvals.clear();
    app.pending_questions.clear();
    app.running_tools.clear();
    app.subagent_activity.clear();
    app.subagent_transcripts.clear();
    app.subagent_order.clear();
    app.chat_view = crate::state::ChatView::Parent;
    app.tool_invocations.clear();
    app.pending_images.clear();
    app.background_commands.clear();

    let mut tool_invocations = std::collections::HashMap::new();

    for event in &snapshot.events {
        match event {
            AgentEvent::UserTaskSubmitted {
                text,
                content_parts,
                submitted_at,
            } => {
                let mut msg = ChatMessage::new(ChatRole::User, text.clone());
                if let Some(secs) = submitted_at {
                    msg.sent_at =
                        Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(*secs));
                }
                for part in content_parts.iter() {
                    if let navi_core::model::ContentPart::Image {
                        media_type, data, ..
                    } = part
                    {
                        let index = msg.images.len() + 1;
                        let chat_image = ChatImage {
                            index,
                            media_type: media_type.clone(),
                            width: None,
                            height: None,
                            data: data.clone(),
                            label: media_type
                                .strip_prefix("image/")
                                .unwrap_or(media_type)
                                .to_uppercase(),
                        };
                        msg.image_labels
                            .push(format!("[Image {}]", chat_image.index));
                        msg.images.push(chat_image);
                    }
                }
                app.messages.push(msg);
            }
            AgentEvent::ModelOutput { text, thinking } => {
                app.messages.push(ChatMessage {
                    thinking_content: thinking.clone().unwrap_or_default(),
                    ..ChatMessage::new(ChatRole::Assistant, text.clone())
                });
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

    // Provider-facing history: include tool turns and rehydrate view_image
    // bytes from path or durable attachment store. Keep the system prompt
    // seeded by reset_system_context as the prefix.
    let mut history = model_messages_from_agent_events(
        &snapshot.events,
        Some(app.project_dir.as_path()),
        Some(app.loaded_config.data_dir.as_path()),
    );
    if let Some(system) = app.conversation_history.first().cloned() {
        history.insert(0, system);
    }
    app.conversation_history = history;

    app.scroll_offset = 0;
    app.input.clear();
    app.input_cursor = 0;
    app.message_action_target = None;
    // Keep selected_message_action — last Message Actions choice is a preference.
    app.expanded_tool_results.clear();
    app.collapsed_tool_results.clear();
    app.hovered_chat_source = None;
    app.selected_chat_source = None;
}
