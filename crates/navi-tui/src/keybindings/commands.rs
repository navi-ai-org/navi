use crate::TuiApp;
use crate::chat::{reset_system_context, retry_last_response};
use crate::commands::{CommandAction, filtered_commands};
use crate::mouse::copy_text_to_clipboard;
use crate::notifications::show_notification;
use crate::render::command_scroll_offset;
use crate::session::session_created_at;
use crate::state::ModalKind;
use crate::ui::list::SelectListState;
use crossterm::event::KeyCode;
use navi_sdk::{AgentEvent, session_title_from_events};

pub(crate) fn handle_command_key(app: &mut TuiApp, code: KeyCode) -> bool {
    const VISIBLE_ROWS: usize = 10;
    let mut list_state = SelectListState::new(app.selected_command, app.command_scroll);
    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Char(ch) => {
            app.command_filter.push(ch);
            list_state.reset();
        }
        KeyCode::Backspace => {
            app.command_filter.pop();
            list_state.clamp(filtered_commands(app).len());
        }
        KeyCode::Down | KeyCode::Tab => {
            list_state.select_next(filtered_commands(app).len());
        }
        KeyCode::PageDown => {
            list_state.page_next(filtered_commands(app).len(), 8);
        }
        KeyCode::Up => {
            list_state.select_previous();
        }
        KeyCode::PageUp => {
            list_state.page_previous(8);
        }
        KeyCode::Enter => return run_selected_command(app),
        _ => {}
    }
    app.selected_command = list_state.selected();
    app.command_scroll = command_scroll_offset(app.selected_command, VISIBLE_ROWS);

    false
}

pub(crate) fn run_selected_command(app: &mut TuiApp) -> bool {
    let commands = filtered_commands(app);
    let Some(command) = commands.get(app.selected_command).copied() else {
        super::close_all_modals(app);
        return false;
    };

    match command.action {
        CommandAction::NewSession => {
            app.messages.clear();
            reset_system_context(app);
            app.input.clear();
            app.input_cursor = 0;
            app.scroll_offset = 0;
            super::close_all_modals(app);
        }
        CommandAction::SwitchModel => {
            super::open_model_picker(app);
        }
        CommandAction::RetryLast => {
            retry_last_response(app);
        }
        CommandAction::OpenThinking => {
            super::open_thinking_picker(app);
        }
        CommandAction::Compact => {
            if app.is_loading {
                show_notification(app, "Compact", "Cannot compact while a request is active.");
            } else {
                show_notification(
                    app,
                    "Compact",
                    "Compaction will trigger on next request if context is full.",
                );
                app.compact_state.last_input_tokens = Some(app.compact_state.context_window);
            }
            super::close_all_modals(app);
        }
        CommandAction::Sessions => {
            super::open_sessions_picker(app);
        }
        CommandAction::CopySession => {
            copy_session_transcript(app);
            super::close_all_modals(app);
        }
        CommandAction::ShareSession => {
            copy_session_json(app);
            super::close_all_modals(app);
        }
        CommandAction::SyncModels => {
            super::provider_sync::sync_models_tui(app);
            super::close_all_modals(app);
        }
        CommandAction::Providers => {
            super::open_provider_settings(app);
        }
        CommandAction::Skills => {
            super::open_skills_picker(app);
        }
        CommandAction::Plugins => {
            super::open_plugins_picker(app);
        }
        CommandAction::McpServers => {
            app.mcp_ui_state.selected_server = 0;
            app.mcp_ui_state.selected_tool = 0;
            app.mcp_ui_state.scroll = 0;
            app.mcp_ui_state.is_focused_on_tools = false;
            super::replace_modal(app, ModalKind::Mcp);
        }
        CommandAction::BackgroundCommands => {
            super::replace_modal(app, ModalKind::BackgroundCommands);
            app.bg_command_selected = 0;
            app.bg_command_scroll = 0;
            // Refresh the list when opening
            let engine = app.engine();
            let session_id = app.session_id.as_str().to_string();
            let tx = app.async_sender();
            crate::runtime::spawn_runtime_task(async move {
                if let Ok(commands) = engine.list_background_commands(&session_id).await {
                    let _ = tx.send(crate::dispatch::AsyncEvent::BackgroundCommandsUpdated(
                        commands,
                    ));
                }
            });
        }
        CommandAction::Quit => return true,
        CommandAction::Settings => {
            super::replace_modal(app, ModalKind::Settings);
            app.selected_setting = 0;
        }
        _ => super::close_all_modals(app),
    }

    false
}

fn copy_session_transcript(app: &mut TuiApp) {
    let transcript = session_transcript(app);
    if transcript.trim().is_empty() {
        show_notification(app, "Session", "Nothing to copy yet.");
        return;
    }
    copy_text_to_clipboard(app, &transcript);
    show_notification(app, "Session", "Session transcript copied.");
}

fn copy_session_json(app: &mut TuiApp) {
    let now = navi_sdk::current_unix_timestamp();
    let value = serde_json::json!({
        "version": 1,
        "id": app.session_id.as_str(),
        "title": session_title_from_events(&app.events),
        "project": app.project_dir,
        "created_at": session_created_at(&app.session_id).unwrap_or(now),
        "updated_at": now,
        "events": app.events,
    });
    let Ok(json) = serde_json::to_string_pretty(&value) else {
        show_notification(app, "Session", "Failed to serialize session.");
        return;
    };
    copy_text_to_clipboard(app, &json);
    show_notification(app, "Session", "Shareable session JSON copied.");
}

fn session_transcript(app: &TuiApp) -> String {
    let mut lines = Vec::new();
    for event in &app.events {
        match event {
            AgentEvent::UserTaskSubmitted { text, content_parts: _ } => {
                lines.push(format!("User:\n{}", text.trim_end()));
            }
            AgentEvent::ModelOutput { text, thinking: _ } => {
                lines.push(format!("Assistant:\n{}", text.trim_end()));
            }
            AgentEvent::ToolRequested(invocation) => {
                lines.push(format!("Tool requested: {}", invocation.tool_name));
            }
            AgentEvent::ToolCompleted(result) => {
                let status = if result.ok { "ok" } else { "error" };
                lines.push(format!(
                    "Tool completed: {} ({status})",
                    result.invocation_id
                ));
            }
            _ => {}
        }
    }
    lines.join("\n\n")
}
