use crate::TuiApp;
use crate::chat::{retry_last_response, start_new_session, submit_message};
use crate::commands::{CommandAction, filtered_commands};
use crate::input::{command_filter_ref, handle_text_input_key};
use crate::mouse::copy_text_to_clipboard;
use crate::notifications::show_notification;
use crate::render::command_scroll_offset;
use crate::session::session_created_at;
use crate::state::Mode;
use crate::state::{ChatMessage, ChatRole};
use crate::state::{ModalKind, SetupPhase};
use crate::ui::SelectListState;
use crossterm::event::{KeyCode, KeyModifiers};
use navi_sdk::{AgentEvent, session_title_from_events};

pub(crate) fn handle_command_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    const VISIBLE_ROWS: usize = 10;
    let mut list_state = SelectListState::new(app.selected_command, app.command_scroll);
    match code {
        KeyCode::Esc => super::close_active_modal(app),
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
        _ => {
            let before = app.command_filter.clone();
            if handle_text_input_key(command_filter_ref(app), code, modifiers, false) {
                if app.command_filter != before {
                    list_state.reset();
                    list_state.clamp(filtered_commands(app).len());
                }
            }
        }
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
            start_new_session(app);
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
                app.input = "Summarize the current session state now and call the new_context_window tool with that summary. Do not wait for the context window to fill.".to_string();
                app.input_cursor = app.input.len();
                submit_message(app);
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
        CommandAction::Usage => {
            crate::usage::open_usage_modal(app);
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
            crate::background::refresh_background_commands(app);
        }
        CommandAction::BackgroundModels => {
            super::replace_modal(app, ModalKind::BackgroundModels);
            app.bg_models_selected = 0;
            app.bg_models_scroll = 0;
            app.bg_model_picker_active = false;
            app.bg_model_picker_task = None;
        }
        CommandAction::Quit => return true,
        CommandAction::Settings => {
            super::replace_modal(app, ModalKind::Settings);
            app.selected_setting = 0;
        }
        CommandAction::ReSetup => {
            app.setup_phase = Some(SetupPhase::ProviderLogin);
            app.mode = Mode::Setup;
            super::close_all_modals(app);
            app.modal_stack.open(ModalKind::Models);
            app.model_filter.clear();
            app.model_filter_cursor = 0;
            app.model_scroll = 0;
            app.refresh_authenticated_providers();
            app.messages.push(ChatMessage::new(
                ChatRole::Assistant,
                "Setting up again. Choose your provider.".to_string(),
            ));
            // onboarding_completed field removed
        }
        CommandAction::ClearGoal => {
            app.goal_state = None;
            let session_id = app.session_id.as_str().to_string();
            let engine = app.engine();
            tokio::spawn(async move {
                let _ = engine.clear_goal(&session_id).await;
            });
            super::close_all_modals(app);
            show_notification(app, "Goal", "Goal cleared.");
        }
        CommandAction::AttachmentModels => {
            super::replace_modal(app, ModalKind::AttachmentModels);
            app.selected_attachment_model = 0;
        }
        CommandAction::Memory => {
            show_notification(
                app,
                "Memory",
                "Use the `memory` tool to search and manage memories. CLI: `navi memory list` or `navi memory search <query>`.",
            );
        }
        CommandAction::Dream => {
            if app.dreaming {
                show_notification(app, "Dream", "Dream is already running in the background.");
            } else {
                show_notification(
                    app,
                    "Dream",
                    "Auto-dream runs every 24h. Manual: `navi memory dream --apply` in a terminal.",
                );
            }
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
            AgentEvent::UserTaskSubmitted {
                text,
                content_parts: _,
            } => {
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
