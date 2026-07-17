use crate::TuiApp;
use crate::chat::{retry_last_response, start_new_session};
use crate::commands::{
    CommandAction, CommandRow, clamp_command_selection, command_rows, first_selectable_command_row,
    next_selectable_command_row, page_next_command_row, page_previous_command_row,
    previous_selectable_command_row,
};
use crate::dispatch::AsyncEvent;
use crate::input::{command_filter_ref, handle_text_input_key};
use crate::mouse::copy_text_to_clipboard;
use crate::notifications::show_notification;
use crate::render::command_scroll_offset;
use crate::runtime::forward_runtime_event_to_tui_for_session;
use crate::session::session_created_at;
use crate::state::Mode;
use crate::state::{ChatMessage, ChatRole, ModalKind, SetupPhase};
use anyhow::Context;
use crossterm::event::{KeyCode, KeyModifiers};
use navi_core::PermissionMode;
use navi_sdk::{AgentEvent, NaviSessionRequest, session_title_from_events};
use std::time::Instant;

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
            app.messages
                .push(ChatMessage::new(ChatRole::Assistant, msg));
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
                // Compact with the session model directly — not via a tool prompt
                // or subagent. The engine summarizes and replaces live history;
                // AutoCompactCompleted clears the TUI transcript.
                start_session_compact(app);
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
        CommandAction::CopyLastResponse => {
            copy_last_response(app);
            super::close_all_modals(app);
        }
        CommandAction::ShareSession => {
            copy_session_json(app);
            super::close_all_modals(app);
        }
        CommandAction::Rewind => {
            let checkpoints = crate::chat::rewind_checkpoints(app);
            if checkpoints.is_empty() {
                show_notification(
                    app,
                    "Rewind",
                    "No user messages yet — send a prompt first.",
                );
                super::close_all_modals(app);
            } else {
                app.selected_rewind = checkpoints.len().saturating_sub(1);
                app.rewind_scroll = 0;
                super::replace_modal(app, ModalKind::Rewind);
            }
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
        CommandAction::ToggleDesktopNotifications => {
            let enabled = !app.loaded_config.config.tui.desktop_notifications;
            app.loaded_config.config.tui.desktop_notifications = enabled;
            show_notification(
                app,
                "Desktop notifications",
                if enabled {
                    "Notify when a job finishes while unfocused."
                } else {
                    "Desktop notifications disabled."
                },
            );
            crate::persistence::save_preferences(app);
            super::close_all_modals(app);
        }
        CommandAction::CyclePermissionMode => {
            super::global::cycle_permission_mode_for_command(app);
            super::close_all_modals(app);
        }
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
    std::fs::create_dir_all(&navi_dir).with_context(|| format!("create {}", navi_dir.display()))?;
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

pub(crate) fn copy_session_transcript(app: &mut TuiApp) {
    let transcript = session_transcript(app);
    if transcript.trim().is_empty() {
        show_notification(app, "Session", "Nothing to copy yet.");
        return;
    }
    copy_text_to_clipboard(app, &transcript);
    show_notification(app, "Session", "Session transcript copied.");
}

pub(crate) fn copy_last_response(app: &mut TuiApp) {
    let Some(user_index) = last_user_message_index(app) else {
        show_notification(app, "Session", "No user message to copy from.");
        return;
    };
    let text = output_since_user_message(app, user_index);
    if text.trim().is_empty() {
        show_notification(
            app,
            "Session",
            "No assistant output after the last user message.",
        );
        return;
    }
    copy_text_to_clipboard(app, &text);
    show_notification(app, "Session", "Last response copied.");
}

/// Copy assistant/tool output after a specific user message (until the next user turn).
pub(crate) fn copy_response_since_user_message(app: &mut TuiApp, user_message_index: usize) {
    let text = output_since_user_message(app, user_message_index);
    if text.trim().is_empty() {
        show_notification(app, "Message", "No assistant output after this message.");
        return;
    }
    copy_text_to_clipboard(app, &text);
    show_notification(app, "Message", "Response copied.");
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

pub(crate) fn session_transcript(app: &TuiApp) -> String {
    // Prefer display messages (includes tool bodies) when available; fall back
    // to session events for sessions that only have event history.
    let from_messages = session_transcript_from_messages(app);
    if !from_messages.trim().is_empty() {
        return from_messages;
    }
    session_transcript_from_events(app)
}

fn session_transcript_from_events(app: &TuiApp) -> String {
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

fn session_transcript_from_messages(app: &TuiApp) -> String {
    let mut lines = Vec::new();
    for msg in &app.messages {
        if let Some(block) = format_message_transcript_block(app, msg) {
            lines.push(block);
        }
    }
    lines.join("\n\n")
}

fn last_user_message_index(app: &TuiApp) -> Option<usize> {
    app.messages
        .iter()
        .rposition(|message| message.role == ChatRole::User)
}

/// Assistant/tool output after `user_message_index`, stopping at the next user message.
pub(crate) fn output_since_user_message(app: &TuiApp, user_message_index: usize) -> String {
    let mut parts = Vec::new();
    for (index, msg) in app.messages.iter().enumerate() {
        if index <= user_message_index {
            continue;
        }
        if msg.role == ChatRole::User {
            break;
        }
        if let Some(block) = format_assistant_output_block(app, msg) {
            parts.push(block);
        }
    }
    parts.join("\n\n")
}

fn format_message_transcript_block(app: &TuiApp, msg: &ChatMessage) -> Option<String> {
    match msg.role {
        ChatRole::User => {
            let text = msg.content.trim();
            if text.is_empty() && msg.images.is_empty() {
                return None;
            }
            let mut body = text.to_string();
            if !msg.images.is_empty() {
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(&format!("[{} image(s)]", msg.images.len()));
            }
            Some(format!("User:\n{}", body.trim_end()))
        }
        ChatRole::Assistant => format_assistant_output_block(app, msg)
            .map(|body| format!("Assistant:\n{}", body.trim_end())),
    }
}

fn format_assistant_output_block(app: &TuiApp, msg: &ChatMessage) -> Option<String> {
    use crate::render::tool::{tool_compact_text, tool_full_content};

    let mut parts = Vec::new();
    if !msg.thinking_content.trim().is_empty() && app.show_thinking {
        parts.push(msg.thinking_content.trim().to_string());
    }
    if !msg.content.trim().is_empty() {
        parts.push(msg.content.trim().to_string());
    }
    if let (Some(inv), Some(result)) = (&msg.tool_invocation, &msg.tool_result) {
        if app.full_tool_view {
            parts.push(tool_full_content(inv, result));
        } else {
            parts.push(tool_compact_text(inv, result));
        }
    } else if let Some(inv) = &msg.tool_invocation {
        parts.push(format!("Tool: {}", inv.tool_name));
    }
    let text = parts.join("\n\n");
    (!text.trim().is_empty()).then_some(text)
}

/// Run forced context compaction with the active session model.
///
/// Ensures the engine session exists (seeded from current history), then calls
/// `compact_session`. Compact events are forwarded so the TUI clears history.
fn start_session_compact(app: &mut TuiApp) {
    if !app.provider_configured {
        show_notification(
            app,
            "Compact",
            "No API key configured for the selected provider.",
        );
        return;
    }

    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        show_notification(
            app,
            "Compact",
            "Cannot compact without a running async runtime.",
        );
        return;
    };

    app.is_loading = true;
    app.loading_start = Some(Instant::now());
    show_notification(app, "Compact", "Compacting conversation with session model…");

    let engine = app.engine();
    let session_id = app.session_id.as_str().to_string();
    let project_dir = app.project_dir.clone();
    let initial_messages = app.conversation_history.clone();
    let active_skills = app.active_skills.clone();
    let tx = app.async_sender();

    app.set_stream_task(runtime.spawn(async move {
        let result = async {
            engine
                .start_session(NaviSessionRequest {
                    project_dir: Some(project_dir),
                    session_id: Some(session_id.clone()),
                    context_packets: Vec::new(),
                    active_skills,
                    initial_messages,
                    ..NaviSessionRequest::default()
                })
                .await
                .map_err(|err| format!("{err:#}"))?;

            let mut events = engine
                .subscribe_events(&session_id)
                .map_err(|err| format!("{err:#}"))?;

            let compact = engine.compact_session(&session_id);
            tokio::pin!(compact);

            let outcome = loop {
                tokio::select! {
                    response = &mut compact => {
                        break response.map_err(|err| format!("{err:#}"))?;
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

            // Guarantee the TUI applies cleanup even if a subscriber missed the
            // broadcast (e.g. race on session start). apply_compacted_* is
            // idempotent when the summary is unchanged.
            let _ = tx.send(AsyncEvent::AgentForSession {
                session_id: session_id.clone(),
                event: AgentEvent::AutoCompactCompleted {
                    tokens_saved: outcome.tokens_saved,
                    summary: outcome.summary,
                    kept_recent_messages: outcome.kept_recent_messages,
                },
            });

            let _ = engine.snapshot_session(&session_id).await;
            Ok::<(), String>(())
        }
        .await;

        if let Err(err) = result {
            let _ = tx.send(AsyncEvent::AgentForSession {
                session_id: session_id.clone(),
                event: AgentEvent::AutoCompactFailed { reason: err.clone() },
            });
            let _ = tx.send(AsyncEvent::TurnCompletedForSession {
                session_id,
                result: Err(format!("Compact failed: {err}")),
            });
        } else {
            let _ = tx.send(AsyncEvent::TurnCompletedForSession {
                session_id,
                result: Ok(String::new()),
            });
        }
    }));
}
