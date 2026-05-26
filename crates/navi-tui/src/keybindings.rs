use crate::TuiApp;
use crate::chat::{reset_system_context, retry_last_response, submit_message};
use crate::commands::{CommandAction, filtered_commands};
use crate::dispatch::AsyncEvent;
use crate::input::{
    api_key_input_ref, chat_input_ref, delete_input_next_char, delete_input_next_hump,
    delete_input_previous_char, delete_input_previous_hump, delete_input_previous_space_word,
    insert_input_char, move_input_next_char, move_input_next_control_stop, move_input_next_hump,
    move_input_previous_char, move_input_previous_control_stop, move_input_previous_hump,
};
use crate::notifications::show_notification;
use crate::persistence::{load_session, save_current_session};
use crate::providers::{
    apply_model_selection, build_model_rows, first_model_index, model_is_available_for_selection,
    next_model_index, previous_model_index, save_api_key_and_rebuild, selected_model_in_rows,
    start_provider_oauth, sync_scroll_to_selection,
};
use crate::session::load_saved_sessions;
use crate::state::{ChatMessage, ChatRole, ModalKind, Mode, ThinkingLevel};
use crate::tools::{approve_pending_tool, cancel_stream, deny_pending_tool};
use crate::ui::effect::UiEffect;
use crate::ui::keymap::KeyOutcome;
use crate::ui::list::SelectListState;
use crossterm::event::{KeyCode, KeyModifiers};
use navi_core::{
    AgentMode, CredentialStore, ModelProvider, canonical_provider_id, provider_catalog,
    resolve_provider_api_key,
};
use navi_openai::OpenAiProvider;
use std::time::Instant;

// ─── key handling ──────────────────────────────────────────────────────────────
pub(crate) fn handle_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    route_key(app, code, modifiers).should_quit()
}

pub(crate) fn route_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> KeyOutcome {
    let approval = route_approval_key(app, code);
    if approval.is_handled() {
        return approval;
    }

    let normal_cancel = route_normal_cancel_key(app, code);
    if normal_cancel.is_handled() {
        return normal_cancel;
    }

    let global = route_global_key(app, code, modifiers);
    if global.is_handled() {
        return global;
    }

    route_mode_key(app, code, modifiers)
}

fn route_approval_key(app: &mut TuiApp, code: KeyCode) -> KeyOutcome {
    if !app.pending_approvals.is_empty() {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                approve_pending_tool(app);
                return KeyOutcome::Handled;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                deny_pending_tool(app);
                return KeyOutcome::Handled;
            }
            _ => {}
        }
    }
    KeyOutcome::Ignored
}

fn route_normal_cancel_key(app: &mut TuiApp, code: KeyCode) -> KeyOutcome {
    if app.mode == Mode::Normal && code == KeyCode::Esc && (app.is_loading || app.has_async_task())
    {
        cancel_stream(app);
        return KeyOutcome::Handled;
    }
    KeyOutcome::Ignored
}

fn route_global_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> KeyOutcome {
    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('c') => return apply_ui_effect(app, UiEffect::Quit),
            KeyCode::Char('d') => {
                let outcome = apply_ui_effect(app, UiEffect::ReplaceModal(ModalKind::Debug));
                tracing::info!("debug modal opened");
                return outcome;
            }
            KeyCode::Char('g') => {
                app.yolo_mode = !app.yolo_mode;
                tracing::info!(enabled = app.yolo_mode, "yolo mode toggled");
                show_notification(
                    app,
                    "Tools",
                    format!(
                        "YOLO mode {}.",
                        if app.yolo_mode { "enabled" } else { "disabled" }
                    ),
                );
                return KeyOutcome::Handled;
            }
            KeyCode::Char('p') => {
                let outcome = apply_ui_effect(app, UiEffect::ReplaceModal(ModalKind::Commands));
                app.command_filter.clear();
                app.selected_command = 0;
                return outcome;
            }
            KeyCode::Char('m') => {
                open_model_picker(app);
                return KeyOutcome::Handled;
            }
            KeyCode::Char('o') | KeyCode::Char('O') => {
                app.full_tool_view = !app.full_tool_view;
                show_notification(
                    app,
                    "Tools",
                    if app.full_tool_view {
                        "Full tool view enabled."
                    } else {
                        "Compact tool view enabled."
                    },
                );
                return KeyOutcome::Handled;
            }
            KeyCode::Char('j') | KeyCode::Char('\n') | KeyCode::Char('\r') | KeyCode::Enter => {
                if !app.input.trim().is_empty() && !app.is_loading {
                    submit_message(app);
                }
                return KeyOutcome::Handled;
            }
            KeyCode::Char('n') => {
                app.messages.clear();
                reset_system_context(app);
                app.input.clear();
                app.input_cursor = 0;
                app.scroll_offset = 0;
                let outcome = apply_ui_effect(app, UiEffect::CloseAllModals);
                show_notification(app, "Layer", "New layer started.");
                return outcome;
            }
            _ => {}
        }
    }
    KeyOutcome::Ignored
}

fn route_mode_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> KeyOutcome {
    let should_quit = match app.mode {
        Mode::Normal => handle_normal_key(app, code, modifiers),
        Mode::Commands => handle_command_key(app, code),
        Mode::Models => handle_model_key(app, code, modifiers),
        Mode::ApiKeyEntry => handle_api_key_key(app, code, modifiers),
        Mode::Thinking => handle_thinking_key(app, code),
        Mode::Sessions => handle_sessions_key(app, code),
        Mode::Settings => handle_settings_key(app, code),
        Mode::Providers => handle_providers_key(app, code),
        Mode::Debug => handle_debug_key(app, code),
        Mode::Help => handle_help_key(app, code),
    };
    if should_quit {
        KeyOutcome::Quit
    } else {
        KeyOutcome::Handled
    }
}

fn apply_ui_effect(app: &mut TuiApp, effect: UiEffect<ModalKind>) -> KeyOutcome {
    match effect {
        UiEffect::Quit => KeyOutcome::Quit,
        UiEffect::OpenModal(modal) => {
            open_modal(app, modal);
            KeyOutcome::Handled
        }
        UiEffect::ReplaceModal(modal) => {
            replace_modal(app, modal);
            KeyOutcome::Handled
        }
        UiEffect::CloseModal => {
            close_active_modal(app);
            KeyOutcome::Handled
        }
        UiEffect::CloseAllModals => {
            close_all_modals(app);
            KeyOutcome::Handled
        }
    }
}

fn open_modal(app: &mut TuiApp, modal: ModalKind) {
    app.modal_stack.open(modal);
    app.mode = modal.mode();
}

fn replace_modal(app: &mut TuiApp, modal: ModalKind) {
    app.modal_stack.replace(Some(modal));
    app.mode = modal.mode();
}

pub(crate) fn close_active_modal(app: &mut TuiApp) {
    app.modal_stack.close();
    app.mode = app
        .modal_stack
        .top()
        .map(ModalKind::mode)
        .unwrap_or(Mode::Normal);
}

pub(crate) fn close_all_modals(app: &mut TuiApp) {
    app.modal_stack.clear();
    app.mode = Mode::Normal;
}

fn handle_debug_key(app: &mut TuiApp, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc | KeyCode::Enter => {
            apply_ui_effect(app, UiEffect::CloseModal);
            tracing::info!("debug modal closed");
        }
        _ => {}
    }
    false
}

pub(crate) fn handle_help_key(app: &mut TuiApp, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') => {
            apply_ui_effect(app, UiEffect::CloseModal);
        }
        _ => {}
    }
    false
}

pub(crate) fn open_model_picker(app: &mut TuiApp) {
    replace_modal(app, ModalKind::Models);
    app.pending_model_selection = None;
    app.model_filter.clear();
    app.model_scroll = 0;

    let rows = build_model_rows(app);
    app.selected_model = first_model_index(&rows).unwrap_or(app.selected_model);
}

fn cycle_agent(app: &mut TuiApp) {
    app.selected_agent = Some(match app.selected_agent {
        Some(agent) => agent.next_code_mode(),
        None => AgentMode::Plan,
    });
    show_notification(
        app,
        "Agent",
        format!(
            "{} agent selected.",
            app.selected_agent.expect("agent selected").label()
        ),
    );
}

fn open_provider_settings(app: &mut TuiApp) {
    replace_modal(app, ModalKind::Providers);
    app.selected_provider_setting = 0;
    app.provider_settings_scroll = 0;
}

fn open_thinking_picker(app: &mut TuiApp) {
    replace_modal(app, ModalKind::Thinking);
    app.selected_thinking = app.thinking_level as usize;
}

pub(crate) const THINKING_OPTIONS: &[ThinkingLevel] = &[
    ThinkingLevel::Max,
    ThinkingLevel::High,
    ThinkingLevel::Medium,
    ThinkingLevel::Low,
    ThinkingLevel::Off,
];

fn handle_thinking_key(app: &mut TuiApp, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc => close_active_modal(app),
        KeyCode::Down => {
            app.selected_thinking = (app.selected_thinking + 1).min(THINKING_OPTIONS.len() - 1);
        }
        KeyCode::Up => {
            app.selected_thinking = app.selected_thinking.saturating_sub(1);
        }
        KeyCode::Enter => {
            let level = THINKING_OPTIONS[app.selected_thinking];
            app.thinking_level = level;
            close_all_modals(app);
            show_notification(
                app,
                "Thinking",
                format!("Thinking set to {}.", level.label()),
            );
        }
        _ => {}
    }

    false
}

pub(crate) fn handle_settings_key(app: &mut TuiApp, code: KeyCode) -> bool {
    const SETTINGS_COUNT: usize = 2;
    let mut list_state = SelectListState::new(app.selected_setting, 0);
    match code {
        KeyCode::Esc => close_active_modal(app),
        KeyCode::Down => {
            list_state.select_next(SETTINGS_COUNT);
        }
        KeyCode::Up => {
            list_state.select_previous();
        }
        KeyCode::Char(' ') | KeyCode::Enter => match app.selected_setting {
            0 => {
                app.show_thinking = !app.show_thinking;
                show_notification(
                    app,
                    "Settings",
                    if app.show_thinking {
                        "Thinking text visible."
                    } else {
                        "Thinking text hidden."
                    },
                );
            }
            1 => {
                app.full_tool_view = !app.full_tool_view;
                show_notification(
                    app,
                    "Settings",
                    if app.full_tool_view {
                        "Full tool output visible."
                    } else {
                        "Tool output compacted."
                    },
                );
            }
            _ => {}
        },
        _ => {}
    }
    app.selected_setting = list_state.selected();
    false
}

fn handle_providers_key(app: &mut TuiApp, code: KeyCode) -> bool {
    let providers = provider_catalog(&app.loaded_config.config);
    let mut list_state =
        SelectListState::new(app.selected_provider_setting, app.provider_settings_scroll);
    match code {
        KeyCode::Esc => close_active_modal(app),
        KeyCode::Down => {
            list_state.select_next(providers.len());
            list_state.sync_scroll(12);
        }
        KeyCode::Up => {
            list_state.select_previous();
            list_state.sync_scroll(12);
        }
        KeyCode::Enter | KeyCode::Char('k') => {
            if let Some(provider) = providers.get(app.selected_provider_setting) {
                app.pending_provider_setup = Some(provider.id.clone());
                app.pending_model_selection = None;
                app.api_key_input.clear();
                app.api_key_cursor = 0;
                apply_ui_effect(app, UiEffect::OpenModal(ModalKind::ApiKeyEntry));
            }
        }
        KeyCode::Char('o') | KeyCode::Char('O') => {
            if let Some(provider) = providers.get(app.selected_provider_setting) {
                start_provider_oauth(app, provider);
            }
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            if let Some(provider) = providers.get(app.selected_provider_setting) {
                let provider_id = provider.id.clone();
                sync_provider_tui(app, &provider_id);
            }
        }
        _ => {}
    }
    app.selected_provider_setting = list_state.selected();
    app.provider_settings_scroll = list_state.scroll();
    false
}

fn open_sessions_picker(app: &mut TuiApp) {
    app.saved_sessions = load_saved_sessions(&app.session_store);
    replace_modal(app, ModalKind::Sessions);
    app.selected_session = 0;
    app.session_scroll = 0;
}

fn handle_sessions_key(app: &mut TuiApp, code: KeyCode) -> bool {
    let mut list_state = SelectListState::new(app.selected_session, app.session_scroll);
    match code {
        KeyCode::Esc => close_active_modal(app),
        KeyCode::Down => {
            list_state.select_next(app.saved_sessions.len());
            list_state.sync_scroll(10);
        }
        KeyCode::Up => {
            list_state.select_previous();
            list_state.sync_scroll(10);
        }
        KeyCode::Enter => {
            if let Some(snapshot) = app.saved_sessions.get(app.selected_session).cloned() {
                save_current_session(app);
                load_session(app, &snapshot);
            }
            close_all_modals(app);
        }
        KeyCode::Delete => {
            if let Some(snapshot) = app.saved_sessions.get(app.selected_session) {
                let path = app
                    .session_store
                    .root()
                    .join(format!("{}.json", snapshot.id.0));
                let _ = std::fs::remove_file(&path);
            }
            app.saved_sessions = load_saved_sessions(&app.session_store);
            list_state.clamp(app.saved_sessions.len());
            list_state.sync_scroll(10);
        }
        _ => {}
    }
    app.selected_session = list_state.selected();
    app.session_scroll = list_state.scroll();

    false
}

pub(crate) fn handle_normal_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Left | KeyCode::Char('b') => move_input_previous_control_stop(app),
            KeyCode::Right | KeyCode::Char('f') => move_input_next_control_stop(app),
            KeyCode::Backspace
            | KeyCode::Char('h')
            | KeyCode::Char('w')
            | KeyCode::Char('\u{7f}') => delete_input_previous_hump(app),
            KeyCode::Delete => delete_input_next_hump(app),
            KeyCode::Char('a') => app.input_cursor = 0,
            KeyCode::Char('e') => app.input_cursor = app.input.len(),
            KeyCode::Char('u') => {
                app.input.drain(..app.input_cursor);
                app.input_cursor = 0;
            }
            KeyCode::Char('k') => {
                chat_input_ref(app).delete_to_end();
            }
            _ => return false,
        }
        return false;
    }

    if modifiers.contains(KeyModifiers::ALT) {
        match code {
            KeyCode::Left | KeyCode::Char('b') | KeyCode::Char(',') => {
                move_input_previous_hump(app)
            }
            KeyCode::Right | KeyCode::Char('f') | KeyCode::Char('.') => move_input_next_hump(app),
            KeyCode::Backspace | KeyCode::Char('h') | KeyCode::Char('\u{7f}') => {
                delete_input_previous_space_word(app)
            }
            KeyCode::Delete | KeyCode::Char('d') => delete_input_next_hump(app),
            _ => return false,
        }
        return false;
    }

    match code {
        KeyCode::Tab => cycle_agent(app),
        KeyCode::Char('/') if app.input.is_empty() => {
            replace_modal(app, ModalKind::Commands);
            app.command_filter.clear();
            app.selected_command = 0;
        }
        KeyCode::Char('?') if app.input.is_empty() => {
            replace_modal(app, ModalKind::Help);
        }
        KeyCode::Char('q') if app.input.is_empty() && app.messages.is_empty() => return true,
        KeyCode::Char(ch) => insert_input_char(app, ch),
        KeyCode::Backspace => {
            delete_input_previous_char(app);
        }
        KeyCode::Delete => {
            delete_input_next_char(app);
        }
        KeyCode::Left => {
            move_input_previous_char(app);
        }
        KeyCode::Right => {
            move_input_next_char(app);
        }
        KeyCode::Home => {
            app.input_cursor = 0;
        }
        KeyCode::End => {
            app.input_cursor = app.input.len();
        }
        KeyCode::Up => {
            app.scroll_offset = app.scroll_offset.saturating_add(3);
        }
        KeyCode::Down => {
            app.scroll_offset = app.scroll_offset.saturating_sub(3);
        }
        KeyCode::PageUp => {
            app.scroll_offset = app.scroll_offset.saturating_add(15);
        }
        KeyCode::PageDown => {
            app.scroll_offset = app.scroll_offset.saturating_sub(15);
        }
        KeyCode::Enter => {
            insert_input_char(app, '\n');
        }
        KeyCode::Esc => {
            if app.is_loading {
                cancel_stream(app);
            } else {
                app.scroll_offset = 0;
            }
        }
        _ => {}
    }

    false
}

pub(crate) fn handle_command_key(app: &mut TuiApp, code: KeyCode) -> bool {
    let mut list_state = SelectListState::new(app.selected_command, 0);
    match code {
        KeyCode::Esc => close_active_modal(app),
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

    false
}

pub(crate) fn handle_model_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    let rows = build_model_rows(app);
    // List visible height is approximately modal height (22) minus decoration (~7 rows)
    let visible_rows = 14u16;
    match code {
        KeyCode::Esc => close_active_modal(app),
        KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
            sync_models_tui(app);
            close_all_modals(app);
        }
        KeyCode::Char('e') if modifiers.contains(KeyModifiers::CONTROL) => {
            if selected_model_in_rows(&rows, app.selected_model).is_some() {
                app.pending_model_selection = Some(app.selected_model);
                replace_modal(app, ModalKind::ApiKeyEntry);
                app.api_key_input.clear();
                app.api_key_cursor = 0;
            }
        }
        KeyCode::Tab => {
            // Sync just the provider that owns the currently selected model
            let provider_id = app
                .models
                .get(app.selected_model)
                .map(|m| m.provider_id.clone());
            if let Some(pid) = provider_id {
                sync_provider_tui(app, &pid);
            }
            close_all_modals(app);
        }
        KeyCode::Char(ch) => {
            app.model_filter.push(ch);
            app.model_scroll = 0;
            app.selected_model =
                first_model_index(&build_model_rows(app)).unwrap_or(app.selected_model);
        }
        KeyCode::Backspace => {
            app.model_filter.pop();
            app.model_scroll = 0;
            app.selected_model =
                first_model_index(&build_model_rows(app)).unwrap_or(app.selected_model);
        }
        KeyCode::Down => {
            app.selected_model = next_model_index(app, &rows);
            sync_scroll_to_selection(app, &rows, visible_rows);
        }
        KeyCode::Up => {
            app.selected_model = previous_model_index(app, &rows);
            sync_scroll_to_selection(app, &rows, visible_rows);
        }
        KeyCode::Enter => {
            if selected_model_in_rows(&rows, app.selected_model).is_none() {
                return false;
            }
            let model = &app.models[app.selected_model];
            if model_is_available_for_selection(app, model) {
                apply_model_selection(app, app.selected_model);
                app.pending_model_selection = None;
                close_all_modals(app);
            } else {
                app.pending_model_selection = Some(app.selected_model);
                replace_modal(app, ModalKind::ApiKeyEntry);
                app.api_key_input.clear();
                app.api_key_cursor = 0;
            }
        }
        _ => {}
    }

    false
}

fn handle_api_key_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('a') => {
                api_key_input_ref(app).move_to_start();
                return false;
            }
            KeyCode::Char('e') => {
                api_key_input_ref(app).move_to_end();
                return false;
            }
            KeyCode::Char('u') => {
                api_key_input_ref(app).delete_to_start();
                return false;
            }
            // ctrl+v is handled as a paste by the terminal — characters arrive as Char events
            _ => return false,
        }
    }

    match code {
        KeyCode::Esc => {
            api_key_input_ref(app).clear();
            app.pending_model_selection = None;
            let had_provider_parent = app.pending_provider_setup.take().is_some();
            if had_provider_parent {
                close_active_modal(app);
            } else {
                close_all_modals(app);
            }
        }
        KeyCode::Enter => {
            save_api_key_and_rebuild(app);
        }
        KeyCode::Char(ch) => {
            api_key_input_ref(app).insert_char(ch);
        }
        KeyCode::Backspace => {
            api_key_input_ref(app).delete_previous_char();
        }
        KeyCode::Left => {
            api_key_input_ref(app).move_previous_char();
        }
        KeyCode::Right => {
            api_key_input_ref(app).move_next_char();
        }
        KeyCode::Home => {
            api_key_input_ref(app).move_to_start();
        }
        KeyCode::End => {
            api_key_input_ref(app).move_to_end();
        }
        _ => {}
    }

    false
}

pub(crate) fn run_selected_command(app: &mut TuiApp) -> bool {
    let commands = filtered_commands(app);
    let Some(command) = commands.get(app.selected_command).copied() else {
        close_all_modals(app);
        return false;
    };

    match command.action {
        CommandAction::NewSession => {
            app.messages.clear();
            reset_system_context(app);
            app.input.clear();
            app.input_cursor = 0;
            app.scroll_offset = 0;
            close_all_modals(app);
        }
        CommandAction::Agent => {
            cycle_agent(app);
            close_all_modals(app);
        }
        CommandAction::SwitchModel => {
            open_model_picker(app);
        }
        CommandAction::RetryLast => {
            retry_last_response(app);
        }
        CommandAction::OpenThinking => {
            open_thinking_picker(app);
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
            close_all_modals(app);
        }
        CommandAction::Sessions => {
            open_sessions_picker(app);
        }
        CommandAction::SyncModels => {
            sync_models_tui(app);
            close_all_modals(app);
        }
        CommandAction::Providers => {
            open_provider_settings(app);
        }
        CommandAction::Quit => return true,
        CommandAction::Settings => {
            replace_modal(app, ModalKind::Settings);
            app.selected_setting = 0;
        }
        _ => close_all_modals(app),
    }

    false
}

pub(crate) fn sync_models_tui(app: &mut TuiApp) {
    if app.is_loading {
        return;
    }
    app.is_loading = true;
    app.loading_start = Some(Instant::now());

    app.messages.push(ChatMessage {
        status: Some("syncing".to_string()),
        ..ChatMessage::new(
            ChatRole::Assistant,
            "Syncing models from providers...".to_string(),
        )
    });

    let tx = app.async_sender();
    let mut loaded_config = app.loaded_config.clone();
    let cwd = app.project_dir.clone();

    app.set_stream_task(tokio::spawn(async move {
        let credential_store = CredentialStore::new(loaded_config.data_dir.clone());
        let catalog = provider_catalog(&loaded_config.config);
        let mut updated_any = false;
        let mut synced_providers = Vec::new();
        let mut failed_providers = Vec::new();

        for provider_config in catalog {
            if let Some(api_key) =
                resolve_provider_api_key(&credential_store, &provider_config, &provider_config.id)
            {
                match OpenAiProvider::from_provider_config_with_key(&provider_config, api_key) {
                    Ok(provider) => match provider.list_models().await {
                        Ok(models) => {
                            if !models.is_empty() {
                                loaded_config
                                    .config
                                    .update_provider_models(&provider_config.id, &models);
                                updated_any = true;
                                synced_providers.push(provider_config.id.clone());
                            }
                        }
                        Err(e) => {
                            failed_providers.push(format!("{}: {}", provider_config.id, e));
                        }
                    },
                    Err(e) => {
                        failed_providers.push(format!("{}: {}", provider_config.id, e));
                    }
                }
            }
        }

        let message = if updated_any {
            let save_result = if let Some(_) = &loaded_config.project_config_path {
                navi_core::save_project_config(&cwd, &loaded_config.config)
            } else if let Some(global_path) = &loaded_config.global_config_path {
                navi_core::save_global_config(global_path, &loaded_config.config)
            } else {
                Err(anyhow::anyhow!("no config file path found to save"))
            };

            match save_result {
                Ok(path) => {
                    let synced_str = synced_providers.join(", ");
                    let mut msg = format!(
                        "Successfully synced models for: {synced_str}.\nSaved configuration to {}",
                        path.display()
                    );
                    if !failed_providers.is_empty() {
                        msg.push_str(&format!(
                            "\nFailed to sync some providers:\n- {}",
                            failed_providers.join("\n- ")
                        ));
                    }
                    msg
                }
                Err(e) => {
                    format!("Synced models, but failed to save configuration: {}", e)
                }
            }
        } else {
            if failed_providers.is_empty() {
                "No providers had credentials configured for model synchronization.".to_string()
            } else {
                format!(
                    "Failed to sync models:\n- {}",
                    failed_providers.join("\n- ")
                )
            }
        };

        let _ = tx.send(AsyncEvent::SyncCompleted {
            loaded_config,
            message,
        });
    }));
}

fn sync_provider_tui(app: &mut TuiApp, provider_id: &str) {
    if app.is_loading {
        return;
    }
    app.is_loading = true;
    app.loading_start = Some(Instant::now());

    app.messages.push(ChatMessage {
        status: Some("syncing".to_string()),
        ..ChatMessage::new(
            ChatRole::Assistant,
            format!("Syncing models for provider '{provider_id}'..."),
        )
    });

    let tx = app.async_sender();
    let mut loaded_config = app.loaded_config.clone();
    let cwd = app.project_dir.clone();
    let target_provider = provider_id.to_string();

    app.set_stream_task(tokio::spawn(async move {
        let credential_store = CredentialStore::new(loaded_config.data_dir.clone());
        let catalog = provider_catalog(&loaded_config.config);

        let message = if let Some(provider_config) = catalog
            .iter()
            .find(|pc| canonical_provider_id(&pc.id) == canonical_provider_id(&target_provider))
        {
            if let Some(api_key) =
                resolve_provider_api_key(&credential_store, provider_config, &target_provider)
            {
                match OpenAiProvider::from_provider_config_with_key(provider_config, api_key) {
                    Ok(provider) => match provider.list_models().await {
                        Ok(models) if !models.is_empty() => {
                            loaded_config
                                .config
                                .update_provider_models(&target_provider, &models);

                            let save_result = if loaded_config.project_config_path.is_some() {
                                navi_core::save_project_config(&cwd, &loaded_config.config)
                            } else if let Some(global_path) = &loaded_config.global_config_path {
                                navi_core::save_global_config(global_path, &loaded_config.config)
                            } else {
                                Err(anyhow::anyhow!("no config file path found to save"))
                            };

                            match save_result {
                                Ok(path) => format!(
                                    "Synced {} models for '{target_provider}'.\nSaved to {}",
                                    models.len(),
                                    path.display()
                                ),
                                Err(e) => format!(
                                    "Synced models for '{target_provider}', but failed to save: {e}"
                                ),
                            }
                        }
                        Ok(_) => {
                            format!("No models returned by provider '{target_provider}'.")
                        }
                        Err(e) => format!("Failed to sync '{target_provider}': {e}"),
                    },
                    Err(e) => format!("Failed to initialize provider '{target_provider}': {e}"),
                }
            } else {
                format!(
                    "No API key configured for provider '{target_provider}'. Set it via ctrl+m."
                )
            }
        } else {
            format!("Provider '{target_provider}' not found in the catalog.")
        };

        let _ = tx.send(AsyncEvent::SyncCompleted {
            loaded_config,
            message,
        });
    }));
}
