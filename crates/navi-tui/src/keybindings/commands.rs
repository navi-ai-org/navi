use crate::TuiApp;
use crate::chat::{retry_last_response, start_new_session, submit_message};
use crate::commands::{
    CommandAction, CommandRow, clamp_command_selection, command_rows, first_selectable_command_row,
    next_selectable_command_row, page_next_command_row, page_previous_command_row,
    previous_selectable_command_row,
};
use crate::input::{command_filter_ref, handle_text_input_key};
use crate::mouse::copy_text_to_clipboard;
use crate::notifications::show_notification;
use crate::render::command_scroll_offset;
use crate::session::session_created_at;
use crate::state::Mode;
use crate::state::{ChatMessage, ChatRole, ModalKind, SetupPhase};
use anyhow::Context;
use crossterm::event::{KeyCode, KeyModifiers};
use navi_core::PermissionMode;
use navi_sdk::{AgentEvent, session_title_from_events};

pub(crate) fn open_command_palette(app: &mut TuiApp) {
    super::replace_modal(app, ModalKind::Commands);
    app.command_filter.clear();
    app.command_filter_cursor = 0;
    app.command_hub = None;
    app.selected_command = 0;
    app.command_scroll = 0;
    refresh_extension_palette(app);
}

/// Reload palette entries from installed package `tui.json` files.
pub(crate) fn refresh_extension_palette(app: &mut TuiApp) {
    app.extension_palette = navi_sdk::list_installed_tui_extensions(&app.loaded_config.data_dir)
        .unwrap_or_default()
        .into_iter()
        .flat_map(|ext| ext.spec.commands)
        .collect();
}

pub(crate) fn handle_command_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    const VISIBLE_ROWS: usize = 10;
    let rows = command_rows(app);
    let mut selected = clamp_command_selection(&rows, app.selected_command);

    match code {
        KeyCode::Esc => {
            // Search → clear filter; hub → root; root → close.
            if !app.command_filter.is_empty() {
                app.command_filter.clear();
                app.command_filter_cursor = 0;
                selected = first_selectable_command_row(&command_rows(app));
            } else if app.command_hub.is_some() {
                app.command_hub = None;
                selected = 0;
            } else {
                super::close_active_modal(app);
                return false;
            }
        }
        KeyCode::Down | KeyCode::Tab => {
            selected = next_selectable_command_row(&rows, selected);
        }
        KeyCode::PageDown => {
            selected = page_next_command_row(&rows, selected, 8);
        }
        KeyCode::Up => {
            selected = previous_selectable_command_row(&rows, selected);
        }
        KeyCode::PageUp => {
            selected = page_previous_command_row(&rows, selected, 8);
        }
        KeyCode::Enter => {
            app.selected_command = selected;
            return run_selected_command(app);
        }
        _ => {
            let before = app.command_filter.clone();
            if handle_text_input_key(command_filter_ref(app), code, modifiers, false)
                && app.command_filter != before
            {
                // Typing always searches the full catalog (including hub actions).
                selected = first_selectable_command_row(&command_rows(app));
                app.selected_command = selected;
                app.command_scroll = command_scroll_offset(app.selected_command, VISIBLE_ROWS);
                return false;
            }
        }
    }
    app.selected_command = selected;
    app.command_scroll = command_scroll_offset(app.selected_command, VISIBLE_ROWS);

    false
}

pub(crate) fn run_selected_command(app: &mut TuiApp) -> bool {
    let rows = command_rows(app);
    let selected = clamp_command_selection(&rows, app.selected_command);
    app.selected_command = selected;
    let Some(row) = rows.get(selected).cloned() else {
        super::close_all_modals(app);
        return false;
    };

    if let CommandRow::Extension { index } = row {
        super::close_all_modals(app);
        if let Some(cmd) = app.extension_palette.get(index).cloned() {
            // Resolve optional panel body from full tui.json specs.
            let body = navi_sdk::list_installed_tui_extensions(&app.loaded_config.data_dir)
                .ok()
                .and_then(|exts| {
                    for ext in exts {
                        if let Some(panel) = ext.spec.panels.first() {
                            if ext.spec.commands.iter().any(|c| c.id == cmd.id) {
                                return Some(panel.body.clone());
                            }
                        }
                    }
                    None
                })
                .filter(|s| !s.is_empty());
            let msg = match body {
                Some(b) => format!("**{}**\n\n{b}", cmd.title),
                None if !cmd.description.is_empty() => {
                    format!("**{}**\n\n{}", cmd.title, cmd.description)
                }
                None => format!("Extension command `{}`", cmd.id),
            };
            app.messages.push(ChatMessage::new(ChatRole::Assistant, msg));
            show_notification(app, "Extension", format!("Ran {}", cmd.title));
        }
        return false;
    }

    let CommandRow::Item(command) = row else {
        super::close_all_modals(app);
        return false;
    };

    match command.action {
        CommandAction::OpenHub(hub) => {
            app.command_hub = Some(hub);
            app.command_filter.clear();
            app.command_filter_cursor = 0;
            app.selected_command = 0;
            app.command_scroll = 0;
            return false;
        }
        CommandAction::Help => {
            crate::view::help::open_help(app);
        }
        CommandAction::NewSession => {
            start_new_session(app);
            super::close_all_modals(app);
        }
        CommandAction::SwitchModel => {
            super::open_model_picker(app);
        }
        CommandAction::RetryLast => {
            retry_last_response(app);
            super::close_all_modals(app);
        }
        CommandAction::OpenThinking => {
            super::open_thinking_picker(app);
        }
        CommandAction::Compact => {
            if app.is_loading {
                show_notification(app, "Compact", "Cannot compact while a request is active.");
            } else if app.conversation_history.len() < 3 {
                show_notification(
                    app,
                    "Compact",
                    "Not enough conversation yet to compact. Continue working first.",
                );
            } else {
                app.input = "Compact this conversation now: write a concise multi-section summary of goals, key decisions, files changed, errors fixed, and next steps, then call the new_context_window tool with that summary. Do not wait for the context window to fill.".to_string();
                app.input_cursor = app.input.len();
                show_notification(app, "Compact", "Requesting conversation compaction…");
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
            crate::mcp_status::open_mcp_modal(app);
        }
        CommandAction::BackgroundCommands => {
            super::replace_modal(app, ModalKind::BackgroundCommands);
            app.bg_command_selected = 0;
            app.bg_command_scroll = 0;
            crate::background::refresh_background_commands(app);
        }
        CommandAction::BackgroundModels => {
            super::open_model_routing(app, crate::state::ModelRoutingTab::Agents);
            app.bg_models_selected = 0;
            app.bg_models_scroll = 0;
        }
        CommandAction::ModelRouting => {
            super::open_model_routing(app, crate::state::ModelRoutingTab::Agents);
        }
        CommandAction::MessageQueue => {
            super::replace_modal(app, ModalKind::MessageQueue);
        }
        CommandAction::ToggleYolo => {
            let mode = if app.yolo_mode {
                PermissionMode::Restricted
            } else {
                PermissionMode::Yolo
            };
            super::global::set_permission_mode_for_command(app, mode);
            super::close_all_modals(app);
        }
        CommandAction::Quit => return true,
        CommandAction::Settings => {
            super::open_settings(app);
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
            super::open_model_routing(app, crate::state::ModelRoutingTab::Attachments);
            app.selected_attachment_model = 0;
        }
        CommandAction::TogglePlanMode => {
            let session_id = app.session_id.as_str().to_string();
            let engine = app.engine();
            let currently_plan = app.agent_mode.restricts_tools();
            tokio::spawn(async move {
                if currently_plan {
                    let _ = engine.exit_plan_mode(&session_id).await;
                } else {
                    let _ = engine.enter_plan_mode(&session_id).await;
                }
            });
            super::close_all_modals(app);
        }
        CommandAction::About => {
            crate::view::about::open_about(app);
        }
        CommandAction::CheckForUpdates => {
            app.update_check_user_initiated = true;
            crate::update_check::spawn_update_check(app);
            show_notification(app, "Updates", "Checking for a newer NAVI release…");
            super::close_all_modals(app);
        }
        CommandAction::InstallUpdate => {
            if app.available_update.is_some() {
                super::replace_modal(app, ModalKind::UpdateAvailable);
            } else {
                app.update_check_user_initiated = true;
                crate::update_check::spawn_update_check(app);
                show_notification(app, "Updates", "Checking for updates before install…");
                super::close_all_modals(app);
            }
        }
        CommandAction::InitializeProject => match initialize_project_config(app) {
            Ok(path) => {
                show_notification(
                    app,
                    "Initialize Project",
                    format!("Wrote {}", path.display()),
                );
                super::close_all_modals(app);
            }
            Err(err) => {
                show_notification(app, "Initialize Project", format!("{err:#}"));
                super::close_all_modals(app);
            }
        },
        CommandAction::Theme => {
            app.theme_filter.clear();
            app.theme_filter_cursor = 0;
            super::replace_modal(app, ModalKind::ThemePicker);
        }
        CommandAction::Debug => {
            super::replace_modal(app, ModalKind::Debug);
        }
        CommandAction::ToggleShowReasoning => {
            app.show_thinking = !app.show_thinking;
            show_notification(
                app,
                "Reasoning",
                if app.show_thinking {
                    "Thinking text visible."
                } else {
                    "Thinking text hidden."
                },
            );
            crate::persistence::save_preferences(app);
            super::close_all_modals(app);
        }
        CommandAction::CyclePermissionMode => {
            super::global::cycle_permission_mode_for_command(app);
            super::close_all_modals(app);
        },
    }

    false
}

/// Create user-authored project config at `.navi/config.toml` if missing.
fn initialize_project_config(app: &TuiApp) -> anyhow::Result<std::path::PathBuf> {
    let project = &app.project_dir;
    let navi_dir = project.join(".navi");
    let config_path = navi_dir.join("config.toml");
    if config_path.exists() {
        anyhow::bail!(
            "{} already exists — edit it manually or remove it first",
            config_path.display()
        );
    }
    std::fs::create_dir_all(&navi_dir)
        .with_context(|| format!("create {}", navi_dir.display()))?;
    let model = &app.loaded_config.config.model;
    let content = format!(
        r#"# Project-local NAVI config (user-authored).
# Global defaults live in ~/.config/navi/config.toml

[model]
provider = "{provider}"
name = "{name}"
"#,
        provider = model.provider,
        name = model.name,
    );
    std::fs::write(&config_path, content)
        .with_context(|| format!("write {}", config_path.display()))?;
    Ok(config_path)
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
                submitted_at: _,
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
