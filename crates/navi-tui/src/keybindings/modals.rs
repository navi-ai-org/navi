use crate::TuiApp;
use crate::input::{
    api_key_input_ref, handle_text_input_key, model_filter_ref, provider_filter_ref,
    queued_edit_input_ref, session_filter_ref, skill_filter_ref, theme_filter_ref,
};
use crate::mouse::copy_text_to_clipboard;
use crate::notifications::show_notification;
use crate::persistence::{load_session, save_current_session, save_preferences};
use crate::providers::{
    apply_model_selection, build_model_rows, first_model_index, model_is_available_for_selection,
    next_model_index, next_model_index_from, previous_model_index, previous_model_index_from,
    rebuild_provider, save_api_key_and_rebuild, selected_model_in_rows, start_provider_oauth,
    sync_scroll_to_model_index, sync_scroll_to_selection,
};
use crate::session::{load_saved_sessions, load_session_snapshot};
use crate::state::{MessageAction, ModalKind, ThinkingLevel};
use crate::theme::filtered_theme_options;
use crate::ui::SelectListState;
use crate::ui::UiEffect;
use crossterm::event::{KeyCode, KeyModifiers};
use navi_sdk::{NaviConfigSaveTarget, QuestionResponse};

use crate::runtime::spawn_runtime_task;

/// Fallback binary effort options (thinking on / thinking off) when no model
/// is selected or the model has no registry effort levels.
pub(crate) const BINARY_EFFORT_OPTIONS: &[ThinkingLevel] =
    &[ThinkingLevel::Max, ThinkingLevel::Off];

/// Registry-aware effort options for the selected model.
///
/// Each model only shows its own configured effort levels. Models without
/// `reasoning_levels` get the binary off/on pair.
pub(crate) fn thinking_options_for_app(app: &TuiApp) -> Vec<ThinkingLevel> {
    let model = app.models.get(app.selected_model);
    let options = ThinkingLevel::options_for_model(model);
    if options.is_empty() {
        BINARY_EFFORT_OPTIONS.to_vec()
    } else {
        options
    }
}

/// Whether the effort picker for the selected model is binary off/on.
pub(crate) fn effort_is_binary_for_app(app: &TuiApp) -> bool {
    let model = app.models.get(app.selected_model);
    ThinkingLevel::is_binary_for_model(model)
}

/// Clamp the app's thinking level to what the selected model supports.
pub(crate) fn clamp_thinking_to_selected_model(app: &mut TuiApp) {
    let model = app.models.get(app.selected_model);
    let resolved = app.thinking_level.resolve_for_model(model);
    if resolved != app.thinking_level {
        app.thinking_level = resolved;
        app.selected_thinking = resolved.index();
    }
}

pub(crate) fn handle_debug_key(app: &mut TuiApp, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc | KeyCode::Enter => {
            super::apply_ui_effect(app, UiEffect::CloseModal);
            tracing::info!("debug modal closed");
        }
        _ => {}
    }
    false
}

pub(crate) fn handle_help_key(app: &mut TuiApp, code: KeyCode) -> bool {
    use crate::view::help::{
        ensure_help_visible, help_entry_count, move_help_selection, set_help_visible_rows,
    };

    // Keep a sane default until the first frame measures the list body.
    if app.help_visible_rows.get() < 3 {
        set_help_visible_rows(app, 8);
    }

    match code {
        KeyCode::Esc | KeyCode::Char('?') => {
            super::apply_ui_effect(app, UiEffect::CloseModal);
        }
        KeyCode::Enter => {
            // Enter closes (same as keyboard cheatsheet dismiss).
            super::apply_ui_effect(app, UiEffect::CloseModal);
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
            move_help_selection(app, 1);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            move_help_selection(app, -1);
        }
        KeyCode::PageDown => {
            let step = app.help_visible_rows.get().max(3) as isize;
            move_help_selection(app, step);
        }
        KeyCode::PageUp => {
            let step = app.help_visible_rows.get().max(3) as isize;
            move_help_selection(app, -step);
        }
        KeyCode::Home => {
            app.selected_help = 0;
            app.help_scroll = 0;
            crate::view::help::clamp_help_selection(app);
            ensure_help_visible(app);
        }
        KeyCode::End => {
            app.selected_help = help_entry_count().saturating_sub(1);
            ensure_help_visible(app);
        }
        _ => {}
    }
    false
}

pub(crate) fn handle_usage_key(app: &mut TuiApp, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc | KeyCode::Enter => {
            super::apply_ui_effect(app, UiEffect::CloseModal);
        }
        KeyCode::Char('r') => crate::usage::refresh_usage(app),
        _ => {}
    }
    false
}

pub(crate) fn handle_message_actions_key(app: &mut TuiApp, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Down | KeyCode::Tab => {
            app.selected_message_action =
                (app.selected_message_action + 1).min(MessageAction::ALL.len().saturating_sub(1));
        }
        KeyCode::Up => {
            app.selected_message_action = app.selected_message_action.saturating_sub(1);
        }
        KeyCode::Enter => crate::mouse::run_message_action(app, app.selected_message_action),
        _ => {}
    }
    false
}

pub(crate) fn handle_rewind_key(app: &mut TuiApp, code: KeyCode) -> bool {
    let checkpoints = crate::chat::rewind_checkpoints(app);
    let len = checkpoints.len();
    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Down | KeyCode::Tab => {
            if len > 0 {
                app.selected_rewind = (app.selected_rewind + 1).min(len.saturating_sub(1));
                ensure_rewind_visible(app, len);
            }
        }
        KeyCode::Up => {
            app.selected_rewind = app.selected_rewind.saturating_sub(1);
            ensure_rewind_visible(app, len);
        }
        KeyCode::Enter => {
            if let Some((message_index, _)) = checkpoints.get(app.selected_rewind) {
                crate::mouse::run_rewind_checkpoint(app, *message_index);
            }
        }
        _ => {}
    }
    false
}

fn ensure_rewind_visible(app: &mut TuiApp, total: usize) {
    const VISIBLE: usize = 10;
    if total == 0 {
        app.rewind_scroll = 0;
        return;
    }
    let selected = app.selected_rewind.min(total.saturating_sub(1));
    if selected < app.rewind_scroll {
        app.rewind_scroll = selected;
    } else if selected >= app.rewind_scroll + VISIBLE {
        app.rewind_scroll = selected + 1 - VISIBLE;
    }
}

pub(crate) fn handle_question_key(
    app: &mut TuiApp,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    const QUESTION_VISIBLE_OPTIONS: usize = 8;
    if app.pending_questions.is_empty() {
        super::close_active_modal(app);
        return false;
    }

    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('a') => {
                if let Some(question) = app.pending_questions.first_mut() {
                    question.move_custom_home();
                }
                return false;
            }
            KeyCode::Char('e') => {
                if let Some(question) = app.pending_questions.first_mut() {
                    question.move_custom_end();
                }
                return false;
            }
            KeyCode::Char('u') => {
                if let Some(question) = app.pending_questions.first_mut() {
                    question.clear_custom();
                }
                return false;
            }
            _ => {}
        }
    }

    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Down | KeyCode::Tab => {
            if let Some(question) = app.pending_questions.first_mut() {
                let row_count = question.row_count();
                if row_count > 0 {
                    question.selected_row = (question.selected_row + 1).min(row_count - 1);
                }
            }
        }
        KeyCode::Up => {
            if let Some(question) = app.pending_questions.first_mut() {
                question.selected_row = question.selected_row.saturating_sub(1);
            }
        }
        KeyCode::Left => {
            if let Some(question) = app.pending_questions.first_mut()
                && question.selected_is_custom()
            {
                question.move_custom_left();
            }
        }
        KeyCode::Right => {
            if let Some(question) = app.pending_questions.first_mut()
                && question.selected_is_custom()
            {
                question.move_custom_right();
            }
        }
        KeyCode::Home => {
            if let Some(question) = app.pending_questions.first_mut()
                && question.selected_is_custom()
            {
                question.move_custom_home();
            }
        }
        KeyCode::End => {
            if let Some(question) = app.pending_questions.first_mut()
                && question.selected_is_custom()
            {
                question.move_custom_end();
            }
        }
        KeyCode::Delete => {
            if let Some(question) = app.pending_questions.first_mut()
                && question.selected_is_custom()
            {
                question.delete_custom_next_char();
            }
        }
        KeyCode::Char(ch) if ('1'..='9').contains(&ch) => {
            let index = ch as usize - '1' as usize;
            if let Some(question) = app.pending_questions.first_mut()
                && index < question.request.options.len()
            {
                question.selected_row = index;
                if question.request.multiple
                    && let Some(selected) = question.selected_options.get_mut(index)
                {
                    *selected = !*selected;
                }
            }
        }
        KeyCode::Char(' ')
            if app
                .pending_questions
                .first()
                .is_some_and(|question| question.selected_is_custom()) =>
        {
            if let Some(question) = app.pending_questions.first_mut() {
                question.insert_custom_char(' ');
            }
        }
        KeyCode::Char(' ') => {
            if let Some(question) = app.pending_questions.first_mut()
                && question.request.multiple
                && let Some(selected) = question.selected_options.get_mut(question.selected_row)
            {
                *selected = !*selected;
            }
        }
        KeyCode::Backspace
            if app
                .pending_questions
                .first()
                .is_some_and(|question| question.selected_is_custom()) =>
        {
            if let Some(question) = app.pending_questions.first_mut() {
                question.delete_custom_previous_char();
            }
        }
        KeyCode::Char(ch)
            if app
                .pending_questions
                .first()
                .is_some_and(|question| question.selected_is_custom()) =>
        {
            if let Some(question) = app.pending_questions.first_mut() {
                question.insert_custom_char(ch);
            }
        }
        KeyCode::Enter => submit_active_question(app),
        KeyCode::Char('n') | KeyCode::Char('N') => deny_active_question(app),
        KeyCode::Char(ch) => {
            if let Some(question) = app.pending_questions.first_mut() {
                question.insert_custom_char(ch);
            }
        }
        _ => {}
    }

    if let Some(question) = app.pending_questions.first_mut() {
        let option_count = question.request.options.len();
        if question.selected_row < option_count {
            let mut list_state =
                SelectListState::new(question.selected_row, question.option_scroll);
            list_state.sync_scroll(QUESTION_VISIBLE_OPTIONS);
            list_state.clamp_scroll(option_count, QUESTION_VISIBLE_OPTIONS);
            question.option_scroll = list_state.scroll();
        }
    }

    false
}

fn submit_active_question(app: &mut TuiApp) {
    let Some(question) = app.pending_questions.first() else {
        super::close_active_modal(app);
        return;
    };
    if question.selected_is_deny() {
        deny_active_question(app);
        return;
    }
    let answers = question.selected_answers();
    if answers.is_empty() {
        show_notification(app, "Question", "Choose an option or enter an answer.");
        return;
    }
    let id = question.request.id.clone();
    resolve_active_question(app, QuestionResponse::Answered { id, answers });
}

fn deny_active_question(app: &mut TuiApp) {
    let Some(question) = app.pending_questions.first() else {
        super::close_active_modal(app);
        return;
    };
    resolve_active_question(
        app,
        QuestionResponse::Dismissed {
            id: question.request.id.clone(),
        },
    );
}

fn resolve_active_question(app: &mut TuiApp, response: QuestionResponse) {
    let id = response.id().to_string();
    app.pending_questions
        .retain(|question| question.request.id != id);
    let engine = app.engine();
    let session_id = app.session_id.as_str().to_string();
    spawn_runtime_task(async move {
        let _ = engine.resolve_question(&session_id, response).await;
    });
    if app.pending_questions.is_empty() {
        super::close_active_modal(app);
    }
}

pub(crate) fn handle_thinking_key(app: &mut TuiApp, code: KeyCode) -> bool {
    let options = thinking_options_for_app(app);
    if options.is_empty() {
        if matches!(code, KeyCode::Esc) {
            super::close_active_modal(app);
        }
        return false;
    }
    let max_idx = options.len().saturating_sub(1);
    // Cursor is `selected_thinking` (global level index), NOT an index into
    // `options`. Fall back to the active thinking_level when the cursor is
    // not among the model's offered levels.
    let mut local_idx = options
        .iter()
        .position(|l| l.index() == app.selected_thinking)
        .or_else(|| options.iter().position(|l| *l == app.thinking_level))
        .unwrap_or(0)
        .min(max_idx);

    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
            local_idx = (local_idx + 1).min(max_idx);
            app.selected_thinking = options[local_idx].index();
        }
        KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
            local_idx = local_idx.saturating_sub(1);
            app.selected_thinking = options[local_idx].index();
        }
        KeyCode::Enter => {
            let level = options
                .iter()
                .find(|l| l.index() == app.selected_thinking)
                .copied()
                .unwrap_or(options[local_idx]);
            app.thinking_level = level;
            app.selected_thinking = level.index();
            super::close_all_modals(app);
            let binary = ThinkingLevel::is_binary_for_model(app.models.get(app.selected_model));
            show_notification(
                app,
                "Effort",
                format!("Effort set to {}.", level.display_label(binary)),
            );
            save_preferences(app);
        }
        _ => {}
    }

    false
}

pub(crate) fn handle_settings_key(app: &mut TuiApp, code: KeyCode) -> bool {
    use crate::settings::{
        SETTINGS_ROWS, SettingAction, SettingRow, clamp_setting_selection, next_selectable_setting,
        previous_selectable_setting,
    };
    const COMPACT_TOOL_LIMITS: &[usize] = &[3, 5, 8, 12, 20];

    let mut selected = clamp_setting_selection(app.selected_setting);
    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Down | KeyCode::Char('j') => {
            selected = next_selectable_setting(selected);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            selected = previous_selectable_setting(selected);
        }
        KeyCode::Char(' ') | KeyCode::Enter => {
            let Some(SettingRow::Action(action)) = SETTINGS_ROWS.get(selected).copied() else {
                app.selected_setting = selected;
                return false;
            };
            match action {
                SettingAction::ShowReasoning => {
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
                    save_preferences(app);
                }
                SettingAction::DesktopNotifications => {
                    let enabled = !app.loaded_config.config.tui.desktop_notifications;
                    app.loaded_config.config.tui.desktop_notifications = enabled;
                    show_notification(
                        app,
                        "Settings",
                        if enabled {
                            "Desktop notifications when unfocused."
                        } else {
                            "Desktop notifications disabled."
                        },
                    );
                    save_preferences(app);
                }
                SettingAction::CompactToolView => {
                    let pin = app
                        .selected_chat_source
                        .as_ref()
                        .and_then(crate::render::tool_policy::selected_tool_id)
                        .map(str::to_string);
                    crate::render::tool_policy::toggle_expand_all_mode(
                        &mut app.full_tool_view,
                        &mut app.expanded_tool_results,
                        &mut app.collapsed_tool_results,
                        pin.as_deref(),
                    );
                    app.chat_render_cache.borrow_mut().signature_hash = 0;
                    show_notification(
                        app,
                        "Settings",
                        if !app.full_tool_view {
                            "Tool output compacted."
                        } else {
                            "Full tool output visible."
                        },
                    );
                    save_preferences(app);
                }
                SettingAction::CompactToolRows => {
                    let current = COMPACT_TOOL_LIMITS
                        .iter()
                        .position(|limit| *limit >= app.compact_tool_visible_limit)
                        .unwrap_or(0);
                    app.compact_tool_visible_limit =
                        COMPACT_TOOL_LIMITS[(current + 1) % COMPACT_TOOL_LIMITS.len()];
                    show_notification(
                        app,
                        "Settings",
                        format!(
                            "Compact tool rows set to {}.",
                            app.compact_tool_visible_limit
                        ),
                    );
                    save_preferences(app);
                }
                SettingAction::Theme => {
                    app.theme_filter.clear();
                    app.theme_filter_cursor = 0;
                    super::replace_modal(app, ModalKind::ThemePicker);
                }
                SettingAction::ChatModel => {
                    super::open_model_routing(app, crate::state::ModelRoutingTab::Chat);
                }
                SettingAction::Effort => {
                    super::open_thinking_picker(app);
                }
                SettingAction::AgentRoutes => {
                    super::open_model_routing(app, crate::state::ModelRoutingTab::Agents);
                    app.bg_models_selected = 0;
                    app.bg_models_scroll = 0;
                }
                SettingAction::AttachmentFallbacks => {
                    super::open_model_routing(app, crate::state::ModelRoutingTab::Attachments);
                    app.selected_attachment_model = 0;
                }
                SettingAction::Providers => {
                    super::open_provider_settings(app);
                }
                SettingAction::PermissionMode => {
                    super::global::cycle_permission_mode_for_command(app);
                }
                SettingAction::AutoUpdate => {
                    let enabled = !app.loaded_config.config.updates.auto_update;
                    app.loaded_config.config.updates.auto_update = enabled;
                    show_notification(
                        app,
                        "Settings",
                        if enabled {
                            "Auto-update enabled — newer releases install automatically."
                        } else {
                            "Auto-update disabled — you'll be notified when updates are available."
                        },
                    );
                    save_preferences(app);
                }
                SettingAction::CheckUpdates => {
                    app.update_check_user_initiated = true;
                    crate::update_check::spawn_update_check(app);
                    show_notification(app, "Updates", "Checking for a newer NAVI release…");
                    super::close_active_modal(app);
                }
                SettingAction::Debug => {
                    super::replace_modal(app, ModalKind::Debug);
                }
                SettingAction::SetupWizard => {
                    app.setup_phase = Some(crate::state::SetupPhase::ProviderLogin);
                    app.mode = crate::state::Mode::Setup;
                    super::close_all_modals(app);
                    app.modal_stack.open(ModalKind::Models);
                    app.model_filter.clear();
                    app.model_filter_cursor = 0;
                    app.model_scroll = 0;
                    app.refresh_authenticated_providers();
                    app.messages.push(crate::state::ChatMessage::new(
                        crate::state::ChatRole::Assistant,
                        "Setting up again. Choose your provider.".to_string(),
                    ));
                }
                SettingAction::MemoryHint => {
                    let detail = app
                        .engine()
                        .memory_quick_status()
                        .unwrap_or_else(|err| format!("status unavailable ({err:#})"));
                    show_notification(
                        app,
                        "Memory",
                        format!(
                            "{detail}\nTool: memory in chat · CLI: navi memory list · navi memory dream --apply"
                        ),
                    );
                }
            }
        }
        _ => {}
    }
    app.selected_setting = selected;
    false
}

pub(crate) fn handle_about_key(app: &mut TuiApp, code: KeyCode) -> bool {
    use crate::view::about::AboutLink;
    let count = AboutLink::all().len();
    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') => {
            app.selected_about_link = (app.selected_about_link + 1) % count.max(1);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.selected_about_link = app
                .selected_about_link
                .checked_sub(1)
                .unwrap_or(count.saturating_sub(1));
        }
        KeyCode::Enter => {
            crate::view::about::open_selected_link(app);
        }
        _ => {}
    }
    false
}

pub(crate) fn handle_update_available_key(app: &mut TuiApp, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Enter => {
            crate::update_check::spawn_apply_update(app, None);
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            let enabled = !app.loaded_config.config.updates.auto_update;
            app.loaded_config.config.updates.auto_update = enabled;
            save_preferences(app);
            show_notification(
                app,
                "Auto-update",
                if enabled {
                    "Auto-update on — future releases install automatically."
                } else {
                    "Auto-update off."
                },
            );
        }
        KeyCode::Char('o') | KeyCode::Char('O') => {
            if let Some(info) = &app.available_update {
                let url = info.release_url.clone();
                match navi_core::open_url(&url) {
                    Ok(()) => show_notification(app, "Release notes", "Opening in browser…"),
                    Err(err) => show_notification(app, "Open failed", err.to_string()),
                }
            }
        }
        _ => {}
    }
    false
}

pub(crate) fn handle_providers_key(
    app: &mut TuiApp,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    use crate::providers::ProviderListRow;
    use navi_sdk::provider_catalog;

    let list_rows = app.filtered_providers();
    let catalog = provider_catalog(&app.loaded_config.config);
    let count = list_rows.len();

    // Current row position (clamped to valid range).
    let current_row_pos = app.selected_provider_setting.min(count.saturating_sub(1));

    // Returns the row index of the next/previous selectable (non-header) row.
    let first_selectable = |start: usize| -> Option<usize> {
        list_rows
            .iter()
            .skip(start)
            .position(|row| !matches!(row, ProviderListRow::Header { .. }))
            .map(|offset| start + offset)
    };
    let next_selectable_after = |start: usize| -> Option<usize> {
        list_rows
            .iter()
            .enumerate()
            .skip(start.saturating_add(1))
            .find_map(|(index, row)| {
                (!matches!(row, ProviderListRow::Header { .. })).then_some(index)
            })
    };
    let previous_selectable_before = |start: usize| -> Option<usize> {
        list_rows
            .iter()
            .enumerate()
            .take(start)
            .rev()
            .find_map(|(index, row)| {
                (!matches!(row, ProviderListRow::Header { .. })).then_some(index)
            })
    };

    // Helper that returns the provider config at a list row position.
    fn provider_at_row<'a>(
        list_rows: &[ProviderListRow],
        catalog: &'a [navi_sdk::ProviderConfig],
        pos: usize,
    ) -> Option<&'a navi_sdk::ProviderConfig> {
        match list_rows.get(pos)? {
            ProviderListRow::Provider { index } => catalog.get(*index),
            ProviderListRow::Account { provider_index, .. } => catalog.get(*provider_index),
            ProviderListRow::Header { .. } => None,
        }
    }

    // Escape-hatch: the current row is an Account row being acted on.
    let selected_account = |pos: usize| -> Option<String> {
        match list_rows.get(pos)? {
            ProviderListRow::Account { account_id, .. } => Some(account_id.clone()),
            _ => None,
        }
    };

    let mut new_row_pos = current_row_pos;
    let mut reset_to_first = false;

    match code {
        KeyCode::Esc => {
            app.provider_filter.clear();
            app.provider_filter_cursor = 0;
            super::close_active_modal(app);
        }
        KeyCode::Down => {
            if let Some(next) = next_selectable_after(current_row_pos) {
                new_row_pos = next;
            }
        }
        KeyCode::Up => {
            if let Some(prev) = previous_selectable_before(current_row_pos) {
                new_row_pos = prev;
            }
        }
        KeyCode::Enter => {
            if let Some(account_id) = selected_account(current_row_pos) {
                let provider = provider_at_row(&list_rows, &catalog, current_row_pos);
                if let Some(provider) = provider {
                    let result = app.credential_store().set_project_account(
                        &app.project_dir,
                        &provider.id,
                        &account_id,
                    );
                    match result {
                        Ok(()) => {
                            rebuild_provider(app);
                            super::close_active_modal(app);
                            show_notification(app, "Account", format!("Using {}.", provider.label));
                        }
                        Err(err) => {
                            show_notification(
                                app,
                                "Account",
                                format!("Failed to select account: {err:#}"),
                            );
                        }
                    }
                }
            } else if let Some(provider) =
                provider_at_row(&list_rows, &catalog, current_row_pos).cloned()
            {
                app.pending_provider_setup = Some(provider.id.clone());
                app.pending_model_selection = None;
                app.api_key_input.clear();
                app.api_key_cursor = 0;
                super::apply_ui_effect(app, UiEffect::OpenModal(ModalKind::ApiKeyEntry));
            }
        }
        KeyCode::Char('k') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(provider) = provider_at_row(&list_rows, &catalog, current_row_pos).cloned()
            {
                app.pending_provider_setup = Some(provider.id.clone());
                app.pending_model_selection = None;
                app.api_key_input.clear();
                app.api_key_cursor = 0;
                super::apply_ui_effect(app, UiEffect::OpenModal(ModalKind::ApiKeyEntry));
            }
        }
        KeyCode::Char('o') | KeyCode::Char('O') if modifiers.contains(KeyModifiers::CONTROL) => {
            // Resolve a provider even when the cursor sits on a section header
            // (common after filtering): try current row → next selectable → first.
            let provider = provider_at_row(&list_rows, &catalog, current_row_pos)
                .cloned()
                .or_else(|| {
                    next_selectable_after(current_row_pos)
                        .and_then(|pos| provider_at_row(&list_rows, &catalog, pos).cloned())
                })
                .or_else(|| {
                    list_rows.iter().find_map(|row| match row {
                        ProviderListRow::Provider { index } => catalog.get(*index).cloned(),
                        ProviderListRow::Account { provider_index, .. } => {
                            catalog.get(*provider_index).cloned()
                        }
                        ProviderListRow::Header { .. } => None,
                    })
                });
            match provider {
                Some(provider) => start_provider_oauth(app, &provider),
                None => {
                    show_notification(app, "OAuth", "Select a provider that supports OAuth.");
                }
            }
        }
        KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(provider) = provider_at_row(&list_rows, &catalog, current_row_pos).cloned()
            {
                super::provider_sync::sync_provider_tui(app, &provider.id);
            }
        }
        KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(account_id) = selected_account(current_row_pos) {
                let provider = provider_at_row(&list_rows, &catalog, current_row_pos);
                if let Some(provider) = provider {
                    let _ = app
                        .credential_store()
                        .delete_credential_account(&provider.id, &account_id);
                    rebuild_provider(app);
                }
            } else if let Some(provider) =
                provider_at_row(&list_rows, &catalog, current_row_pos).cloned()
            {
                let _ = app.credential_store().delete_api_key(&provider.id);
                rebuild_provider(app);
            }
        }
        _ => {
            let before = app.provider_filter.clone();
            if handle_text_input_key(provider_filter_ref(app), code, modifiers, false)
                && app.provider_filter != before
            {
                reset_to_first = true;
            }
        }
    }

    if reset_to_first {
        new_row_pos = first_selectable(0).unwrap_or(0);
        app.provider_settings_scroll = 0;
    } else {
        let visible_rows = 12usize;
        let mut state = SelectListState::new(new_row_pos, app.provider_settings_scroll);
        state.sync_scroll(visible_rows);
        state.clamp_scroll(count, visible_rows);
        app.provider_settings_scroll = state.scroll();
    }

    app.selected_provider_setting = new_row_pos.min(count.saturating_sub(1));

    false
}

pub(crate) fn handle_oauth_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    match code {
        KeyCode::Esc => {
            app.oauth_state = None;
            super::close_active_modal(app);
        }
        KeyCode::Char('c') | KeyCode::Char('C') if modifiers.is_empty() => {
            if let Some(uri) = app
                .oauth_state
                .as_ref()
                .map(|state| state.verification_uri.clone())
            {
                copy_text_to_clipboard(app, &uri);
            }
        }
        KeyCode::Char('o') | KeyCode::Char('O') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(uri) = app
                .oauth_state
                .as_ref()
                .map(|state| state.verification_uri.clone())
            {
                crate::browser::open_url(app, uri);
            }
        }
        // Paste authorization code from the provider "copy this code" page.
        KeyCode::Char('p') | KeyCode::Char('P')
            if modifiers.is_empty() || modifiers.contains(KeyModifiers::CONTROL) =>
        {
            paste_oauth_code_from_clipboard(app);
        }
        KeyCode::Char('v') | KeyCode::Char('V') if modifiers.contains(KeyModifiers::CONTROL) => {
            paste_oauth_code_from_clipboard(app);
        }
        _ => {}
    }
    false
}

fn paste_oauth_code_from_clipboard(app: &mut TuiApp) {
    let Some(state) = app.oauth_state.as_mut() else {
        return;
    };
    let Some(slot) = state.paste_slot.clone() else {
        state.paste_status = Some("This login flow does not accept a pasted code.".into());
        show_notification(
            app,
            "OAuth",
            "This login flow does not accept a pasted code.",
        );
        return;
    };
    let Some(raw) = crate::clipboard::try_read_clipboard_text() else {
        state.paste_status =
            Some("Clipboard is empty — copy the code from the browser first.".into());
        show_notification(
            app,
            "OAuth",
            "Clipboard empty. Copy the code from the browser.",
        );
        return;
    };
    let code = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'')
        .to_string();
    if code.is_empty() || code.len() < 8 {
        state.paste_status = Some("Clipboard does not look like an auth code.".into());
        show_notification(app, "OAuth", "Clipboard does not look like an auth code.");
        return;
    }
    match slot.lock() {
        Ok(mut guard) => {
            *guard = Some(code);
            state.paste_status = Some("Code pasted — finishing login…".into());
            show_notification(app, "OAuth", "Code pasted — finishing login…");
        }
        Err(_) => {
            state.paste_status = Some("Failed to hand code to OAuth waiter.".into());
            show_notification(app, "OAuth", "Failed to hand code to OAuth waiter.");
        }
    }
}

pub(crate) fn handle_sessions_key(
    app: &mut TuiApp,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    let sessions = app.filtered_sessions();
    let mut list_state = SelectListState::new(app.selected_session, app.session_scroll);
    match code {
        KeyCode::Esc => {
            app.session_filter.clear();
            app.session_filter_cursor = 0;
            super::close_active_modal(app);
        }
        KeyCode::Down => {
            list_state.select_next(sessions.len());
            list_state.sync_scroll(10);
        }
        KeyCode::Up => {
            list_state.select_previous();
            list_state.sync_scroll(10);
        }
        KeyCode::Enter => {
            let session_id = sessions
                .get(app.selected_session)
                .map(|info| info.id.clone());
            drop(sessions);
            if let Some(session_id) = session_id {
                if let Some(snapshot) = load_session_snapshot(&app.session_store, &session_id) {
                    save_current_session(app);
                    load_session(app, &snapshot);
                }
            }
            app.session_filter.clear();
            app.session_filter_cursor = 0;
            super::close_all_modals(app);
        }
        KeyCode::Delete => {
            let session_id = sessions.get(app.selected_session).map(|s| s.id.clone());
            drop(sessions);
            if let Some(id) = session_id {
                let _ = app.engine().delete_saved_session(id.as_str());
            }
            app.saved_sessions = load_saved_sessions(&app.session_store);
            let sessions = app.filtered_sessions();
            list_state.clamp(sessions.len());
            list_state.sync_scroll(10);
        }
        _ => {
            let before = app.session_filter.clone();
            if handle_text_input_key(session_filter_ref(app), code, modifiers, false)
                && app.session_filter != before
            {
                app.session_scroll = 0;
                app.selected_session = 0;
                list_state.reset();
            }
        }
    }
    app.selected_session = list_state.selected();
    app.session_scroll = list_state.scroll();

    false
}

pub(crate) fn handle_api_key_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    match code {
        KeyCode::Esc => {
            api_key_input_ref(app).clear();
            app.pending_model_selection = None;
            let had_provider_parent = app.pending_provider_setup.take().is_some();
            if had_provider_parent {
                super::close_active_modal(app);
            } else {
                super::close_all_modals(app);
            }
        }
        KeyCode::Enter => {
            save_api_key_and_rebuild(app);
        }
        _ => {
            let _ = handle_text_input_key(api_key_input_ref(app), code, modifiers, false);
        }
    }

    false
}

pub(crate) fn handle_message_queue_key(
    app: &mut TuiApp,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    let len = app.queued_user_messages.len();
    if len == 0 {
        if matches!(code, KeyCode::Esc | KeyCode::Enter) {
            super::close_active_modal(app);
        }
        return false;
    }

    app.queued_message_selected = app.queued_message_selected.min(len.saturating_sub(1));
    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Up if modifiers.contains(KeyModifiers::CONTROL) => {
            let index = app.queued_message_selected;
            if index > 0 {
                app.queued_user_messages.swap(index, index - 1);
                app.queued_message_selected = index - 1;
            }
        }
        KeyCode::Down if modifiers.contains(KeyModifiers::CONTROL) => {
            let index = app.queued_message_selected;
            if index + 1 < len {
                app.queued_user_messages.swap(index, index + 1);
                app.queued_message_selected = index + 1;
            }
        }
        KeyCode::Down | KeyCode::Tab => {
            app.queued_message_selected = (app.queued_message_selected + 1).min(len - 1);
        }
        KeyCode::Up => {
            app.queued_message_selected = app.queued_message_selected.saturating_sub(1);
        }
        // Remove selected message. Accept several keys because terminals differ:
        // Delete, Backspace, ^?, ^H, and mnemonic `d` / `x`.
        KeyCode::Delete
        | KeyCode::Backspace
        | KeyCode::Char('\u{7f}')
        | KeyCode::Char('\u{8}')
        | KeyCode::Char('d')
        | KeyCode::Char('x')
            if !modifiers.contains(KeyModifiers::CONTROL)
                && !modifiers.contains(KeyModifiers::ALT) =>
        {
            remove_selected_queued_message(app);
        }
        // Clear entire queue.
        KeyCode::Char('D') if modifiers.contains(KeyModifiers::SHIFT) => {
            app.queued_user_messages.clear();
            app.queued_message_selected = 0;
            app.queued_message_scroll = 0;
            super::close_active_modal(app);
        }
        KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
            remove_selected_queued_message(app);
        }
        KeyCode::Enter => open_queued_message_edit(app),
        _ => {}
    }

    if app.queued_user_messages.is_empty() {
        return false;
    }

    let visible_rows = 10usize;
    let mut state = SelectListState::new(app.queued_message_selected, app.queued_message_scroll);
    state.sync_scroll(visible_rows);
    state.clamp_scroll(app.queued_user_messages.len(), visible_rows);
    app.queued_message_selected = state.selected();
    app.queued_message_scroll = state.scroll();
    false
}

/// Remove the currently selected queued message (if any) and fix selection.
pub(crate) fn remove_selected_queued_message(app: &mut TuiApp) {
    let len = app.queued_user_messages.len();
    if len == 0 {
        return;
    }
    let index = app.queued_message_selected.min(len - 1);
    app.queued_user_messages.remove(index);
    if app.queued_user_messages.is_empty() {
        app.queued_message_selected = 0;
        app.queued_message_scroll = 0;
        super::close_active_modal(app);
        return;
    }
    app.queued_message_selected = index.min(app.queued_user_messages.len() - 1);
}

/// Remove a queued message by absolute index (mouse / external callers).
pub(crate) fn remove_queued_message_at(app: &mut TuiApp, index: usize) {
    if index >= app.queued_user_messages.len() {
        return;
    }
    app.queued_message_selected = index;
    remove_selected_queued_message(app);
}

fn open_queued_message_edit(app: &mut TuiApp) {
    let index = app
        .queued_message_selected
        .min(app.queued_user_messages.len().saturating_sub(1));
    let Some(message) = app.queued_user_messages.get(index) else {
        return;
    };
    app.queued_edit_index = Some(index);
    app.queued_edit_text = message.text.clone();
    app.queued_edit_cursor = app.queued_edit_text.len();
    super::apply_ui_effect(app, UiEffect::OpenModal(ModalKind::QueuedMessageEdit));
}

pub(crate) fn handle_queued_message_edit_key(
    app: &mut TuiApp,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    match code {
        KeyCode::Esc => {
            app.queued_edit_index = None;
            app.queued_edit_text.clear();
            app.queued_edit_cursor = 0;
            super::close_active_modal(app);
        }
        // Delete this queued message from the editor.
        KeyCode::Char('d') | KeyCode::Delete if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(index) = app.queued_edit_index.take() {
                app.queued_edit_text.clear();
                app.queued_edit_cursor = 0;
                super::close_active_modal(app);
                remove_queued_message_at(app, index);
            }
        }
        code if is_queued_edit_save_key(code, modifiers) => {
            save_queued_message_edit(app);
        }
        KeyCode::Enter if modifiers.contains(KeyModifiers::SHIFT) => {
            let _ =
                handle_text_input_key(queued_edit_input_ref(app), code, KeyModifiers::NONE, true);
        }
        _ => {
            let _ = handle_text_input_key(queued_edit_input_ref(app), code, modifiers, true);
        }
    }
    false
}

fn is_queued_edit_save_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::Enter) && modifiers.contains(KeyModifiers::CONTROL)
        || matches!(code, KeyCode::Char('\n') | KeyCode::Char('\r'))
}

fn save_queued_message_edit(app: &mut TuiApp) {
    let Some(index) = app.queued_edit_index else {
        super::close_active_modal(app);
        return;
    };
    if let Some(message) = app.queued_user_messages.get_mut(index) {
        message.text = app.queued_edit_text.clone();
    }
    app.queued_edit_index = None;
    app.queued_edit_text.clear();
    app.queued_edit_cursor = 0;
    super::close_active_modal(app);
}

pub(crate) fn handle_set_goal_key(
    app: &mut TuiApp,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    match code {
        KeyCode::Esc => {
            app.goal_draft_text.clear();
            app.goal_draft_cursor = 0;
            super::close_active_modal(app);
        }
        // Enter submits the goal as a chat message + set_goal.
        KeyCode::Enter if !modifiers.contains(KeyModifiers::SHIFT) => {
            let objective = std::mem::take(&mut app.goal_draft_text);
            app.goal_draft_cursor = 0;
            // Clear composer seed so it is not double-sent.
            if !objective.trim().is_empty() && app.input.trim() == objective.trim() {
                app.input.clear();
                app.input_cursor = 0;
            }
            super::close_active_modal(app);
            crate::chat::submit_goal_objective(app, objective);
        }
        KeyCode::Enter if modifiers.contains(KeyModifiers::SHIFT) => {
            let _ = handle_text_input_key(
                crate::input::goal_draft_input_ref(app),
                code,
                KeyModifiers::NONE,
                true,
            );
        }
        _ => {
            let _ = handle_text_input_key(
                crate::input::goal_draft_input_ref(app),
                code,
                modifiers,
                true,
            );
        }
    }
    false
}

pub(crate) fn handle_sudo_password_key(
    app: &mut TuiApp,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    use navi_sdk::SudoPasswordResponse;

    match code {
        KeyCode::Esc => {
            if let Some(prompt) = app.sudo_password_prompt.take() {
                let session_id = app.session_id.as_str().to_string();
                let engine = app.engine();
                let response = SudoPasswordResponse::Cancelled {
                    id: prompt.request_id,
                };
                tokio::spawn(async move {
                    let _ = engine.resolve_sudo_password(&session_id, response).await;
                });
            }
            super::close_active_modal(app);
        }
        KeyCode::Enter => {
            if let Some(prompt) = app.sudo_password_prompt.take() {
                let session_id = app.session_id.as_str().to_string();
                let engine = app.engine();
                let response = SudoPasswordResponse::Submitted {
                    id: prompt.request_id,
                    password: prompt.password,
                };
                tokio::spawn(async move {
                    let _ = engine.resolve_sudo_password(&session_id, response).await;
                });
            }
            super::close_active_modal(app);
        }
        KeyCode::Backspace => {
            if let Some(p) = app.sudo_password_prompt.as_mut()
                && p.cursor > 0
                && !p.password.is_empty()
            {
                let mut c = p.cursor.min(p.password.len());
                while c > 0 && !p.password.is_char_boundary(c) {
                    c -= 1;
                }
                if c > 0 {
                    let prev = p.password[..c]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    p.password.replace_range(prev..c, "");
                    p.cursor = prev;
                }
            }
        }
        KeyCode::Char(ch) if !ch.is_control() && !modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(p) = app.sudo_password_prompt.as_mut() {
                let c = p.cursor.min(p.password.len());
                p.password.insert(c, ch);
                p.cursor = c + ch.len_utf8();
            }
        }
        _ => {}
    }
    false
}

pub(crate) fn handle_confirm_cancel_turn_key(app: &mut TuiApp, code: KeyCode) -> bool {
    match code {
        KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
            crate::tools::cancel_stream(app);
            super::close_active_modal(app);
        }
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
            super::close_active_modal(app);
        }
        _ => {}
    }
    false
}

pub(crate) fn handle_confirm_plan_key(app: &mut TuiApp, code: KeyCode) -> bool {
    use crate::plan_review::{PlanReviewFocus, begin_comment, commit_comment};

    // Without rich review state, fall back to simple accept/reject.
    if app.plan_review.is_none() {
        match code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                crate::plan_review::approve_plan(app);
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('q') => {
                crate::plan_review::quit_plan(app);
            }
            _ => {}
        }
        return false;
    }

    let focus = app
        .plan_review
        .as_ref()
        .map(|r| r.focus)
        .unwrap_or_default();

    match focus {
        PlanReviewFocus::Preview => match code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(r) = app.plan_review.as_mut() {
                    r.cursor_line = r.cursor_line.saturating_sub(1);
                    r.sel_anchor = None;
                    r.ensure_cursor_visible(12);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(r) = app.plan_review.as_mut() {
                    r.cursor_line = r.cursor_line.saturating_add(1);
                    r.clamp_cursor();
                    r.sel_anchor = None;
                    r.ensure_cursor_visible(12);
                }
            }
            KeyCode::Char('K') => {
                if let Some(r) = app.plan_review.as_mut() {
                    if r.sel_anchor.is_none() {
                        r.sel_anchor = Some(r.cursor_line);
                    }
                    r.cursor_line = r.cursor_line.saturating_sub(1);
                    r.ensure_cursor_visible(12);
                }
            }
            KeyCode::Char('J') => {
                if let Some(r) = app.plan_review.as_mut() {
                    if r.sel_anchor.is_none() {
                        r.sel_anchor = Some(r.cursor_line);
                    }
                    r.cursor_line = r.cursor_line.saturating_add(1);
                    r.clamp_cursor();
                    r.ensure_cursor_visible(12);
                }
            }
            KeyCode::Char('c') | KeyCode::Enter => begin_comment(app),
            KeyCode::Char('a') => crate::plan_review::approve_plan(app),
            KeyCode::Char('s') => {
                if let Some(r) = app.plan_review.as_mut() {
                    r.focus = PlanReviewFocus::Prompt;
                }
            }
            KeyCode::Char('q') | KeyCode::Esc => crate::plan_review::quit_plan(app),
            KeyCode::Tab => {
                if let Some(r) = app.plan_review.as_mut() {
                    r.focus = PlanReviewFocus::Prompt;
                }
            }
            _ => {}
        },
        PlanReviewFocus::CommentInput => match code {
            KeyCode::Esc => {
                if let Some(r) = app.plan_review.as_mut() {
                    r.comment_draft.clear();
                    r.focus = PlanReviewFocus::Preview;
                }
            }
            KeyCode::Enter => commit_comment(app),
            KeyCode::Backspace => {
                if let Some(r) = app.plan_review.as_mut()
                    && r.comment_cursor > 0
                    && !r.comment_draft.is_empty()
                {
                    let mut c = r.comment_cursor.min(r.comment_draft.len());
                    while c > 0 && !r.comment_draft.is_char_boundary(c) {
                        c -= 1;
                    }
                    if c > 0 {
                        let prev = r.comment_draft[..c]
                            .char_indices()
                            .next_back()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                        r.comment_draft.replace_range(prev..c, "");
                        r.comment_cursor = prev;
                    }
                }
            }
            KeyCode::Char(ch) if !ch.is_control() => {
                if let Some(r) = app.plan_review.as_mut() {
                    let c = r.comment_cursor.min(r.comment_draft.len());
                    r.comment_draft.insert(c, ch);
                    r.comment_cursor = c + ch.len_utf8();
                }
            }
            KeyCode::Tab => {
                if let Some(r) = app.plan_review.as_mut() {
                    r.focus = PlanReviewFocus::Preview;
                }
            }
            _ => {}
        },
        PlanReviewFocus::Prompt => match code {
            KeyCode::Esc => {
                if let Some(r) = app.plan_review.as_mut() {
                    r.focus = PlanReviewFocus::Preview;
                }
            }
            KeyCode::Enter => crate::plan_review::request_plan_changes(app),
            KeyCode::Backspace => {
                if let Some(r) = app.plan_review.as_mut()
                    && r.prompt_cursor > 0
                    && !r.prompt_draft.is_empty()
                {
                    let mut c = r.prompt_cursor.min(r.prompt_draft.len());
                    while c > 0 && !r.prompt_draft.is_char_boundary(c) {
                        c -= 1;
                    }
                    if c > 0 {
                        let prev = r.prompt_draft[..c]
                            .char_indices()
                            .next_back()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                        r.prompt_draft.replace_range(prev..c, "");
                        r.prompt_cursor = prev;
                    }
                }
            }
            KeyCode::Char('a') => crate::plan_review::approve_plan(app),
            KeyCode::Char('q') => crate::plan_review::quit_plan(app),
            KeyCode::Char(ch) if !ch.is_control() => {
                if let Some(r) = app.plan_review.as_mut() {
                    let c = r.prompt_cursor.min(r.prompt_draft.len());
                    r.prompt_draft.insert(c, ch);
                    r.prompt_cursor = c + ch.len_utf8();
                }
            }
            KeyCode::Tab => {
                if let Some(r) = app.plan_review.as_mut() {
                    r.focus = PlanReviewFocus::Preview;
                }
            }
            _ => {}
        },
    }
    false
}

pub(crate) fn handle_skills_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    let skills = app.filtered_skills();
    let mut list_state = SelectListState::new(app.selected_skill, app.skill_scroll);
    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Down | KeyCode::Tab => {
            list_state.select_next(skills.len());
            list_state.sync_scroll(14);
        }
        KeyCode::Up => {
            list_state.select_previous();
            list_state.sync_scroll(14);
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if let Some(skill) = skills.get(app.selected_skill) {
                let skill_id = skill.id.clone();
                let skill_name = skill.name.clone();
                let was_active = app.is_skill_active(&skill_id);
                app.toggle_skill(&skill_id);
                show_notification(
                    app,
                    "Skills",
                    if !was_active {
                        format!("{} activated.", skill_name)
                    } else {
                        format!("{} deactivated.", skill_name)
                    },
                );
            }
        }
        _ => {
            let before = app.skill_filter.clone();
            if handle_text_input_key(skill_filter_ref(app), code, modifiers, false)
                && app.skill_filter != before
            {
                app.skill_scroll = 0;
                app.selected_skill = 0;
                list_state.reset();
            }
        }
    }
    app.selected_skill = list_state.selected();
    app.skill_scroll = list_state.scroll();
    false
}

pub(crate) fn handle_model_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    let rows = build_model_rows(app);
    let visible_rows = 14u16;
    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
            super::provider_sync::sync_models_tui(app);
            super::close_all_modals(app);
        }
        KeyCode::Char('e')
            if modifiers.contains(KeyModifiers::CONTROL)
                && selected_model_in_rows(&rows, app.selected_model).is_some() =>
        {
            app.pending_model_selection = Some(app.selected_model);
            super::replace_modal(app, ModalKind::ApiKeyEntry);
            app.api_key_input.clear();
            app.api_key_cursor = 0;
        }
        KeyCode::Tab => {
            let provider_id = app
                .models
                .get(app.selected_model)
                .map(|m| m.provider_id.clone());
            if let Some(pid) = provider_id {
                super::provider_sync::sync_provider_tui(app, &pid);
            }
            super::close_all_modals(app);
        }
        KeyCode::Char(ch) => {
            let _ =
                handle_text_input_key(model_filter_ref(app), KeyCode::Char(ch), modifiers, false);
            app.model_scroll = 0;
            app.selected_model =
                first_model_index(&build_model_rows(app)).unwrap_or(app.selected_model);
        }
        KeyCode::Backspace => {
            let before = app.model_filter.clone();
            if handle_text_input_key(model_filter_ref(app), code, modifiers, false)
                && app.model_filter != before
            {
                app.model_scroll = 0;
                app.selected_model =
                    first_model_index(&build_model_rows(app)).unwrap_or(app.selected_model);
            }
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
                super::close_all_modals(app);
                crate::providers::maybe_start_setup_interview(app);
            } else {
                app.pending_model_selection = Some(app.selected_model);
                super::replace_modal(app, ModalKind::ApiKeyEntry);
                app.api_key_input.clear();
                app.api_key_cursor = 0;
            }
        }
        _ => {
            let _ = handle_text_input_key(model_filter_ref(app), code, modifiers, false);
        }
    }

    false
}

pub(crate) fn handle_plugins_key(app: &mut TuiApp, code: KeyCode) -> bool {
    use crate::notifications::show_notification;
    use crate::plugins::{
        PluginPickerRow, install_or_update_from_marketplace, plugin_picker_rows,
        refresh_plugin_catalog,
    };

    let rows = plugin_picker_rows(app);
    let mut list_state = SelectListState::new(app.selected_plugin_row, app.plugin_row_scroll);

    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Down | KeyCode::Tab => {
            list_state.select_next(rows.len());
            list_state.sync_scroll(14);
        }
        KeyCode::Up => {
            list_state.select_previous();
            list_state.sync_scroll(14);
        }
        KeyCode::Char('r') => refresh_plugin_catalog(app),
        KeyCode::Char('i') => {
            if let Some(PluginPickerRow::Catalog(entry)) = rows.get(app.selected_plugin_row) {
                install_or_update_from_marketplace(app, &entry.id, false);
            } else {
                show_notification(app, "Plugins", "Select a marketplace plugin to install.");
            }
        }
        KeyCode::Char('u') => match rows.get(app.selected_plugin_row) {
            Some(PluginPickerRow::Catalog(entry)) => {
                install_or_update_from_marketplace(app, &entry.id, true);
            }
            Some(PluginPickerRow::Installed { id, .. }) => {
                install_or_update_from_marketplace(app, id, true);
            }
            _ => show_notification(app, "Plugins", "Select a plugin to update."),
        },
        KeyCode::Enter => match rows.get(app.selected_plugin_row) {
            Some(PluginPickerRow::Catalog(entry)) => {
                let installed = crate::plugins::list_installed_plugin_ids(app);
                let update = installed.iter().any(|id| id == &entry.id);
                install_or_update_from_marketplace(app, &entry.id, update);
            }
            Some(PluginPickerRow::Installed { id, .. }) => {
                install_or_update_from_marketplace(app, id, true);
            }
            None => {}
        },
        _ => {}
    }

    app.selected_plugin_row = list_state.selected();
    app.plugin_row_scroll = list_state.scroll();
    false
}

pub(crate) fn handle_plugin_approval_key(
    app: &mut TuiApp,
    code: KeyCode,
    _modifiers: KeyModifiers,
) -> bool {
    use crate::plugin_approval::PluginApprovalDecision;

    if app.pending_plugin_approvals.is_empty() {
        super::close_all_modals(app);
        return false;
    }

    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            let req = app.pending_plugin_approvals.remove(0);
            crate::plugin_approval::approve_plugin_install(app, req);
            app.plugin_approval_scroll = 0;
            if app.pending_plugin_approvals.is_empty() {
                super::close_all_modals(app);
            }
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            let req = app.pending_plugin_approvals.remove(0);
            crate::plugin_approval::notify_plugin_decision(
                app,
                &req,
                PluginApprovalDecision::Denied,
            );
            app.plugin_approval_scroll = 0;
            if app.pending_plugin_approvals.is_empty() {
                super::close_all_modals(app);
            }
        }
        KeyCode::Down => {
            app.plugin_approval_scroll = app.plugin_approval_scroll.saturating_add(1);
        }
        KeyCode::Up => {
            app.plugin_approval_scroll = app.plugin_approval_scroll.saturating_sub(1);
        }
        KeyCode::PageDown => {
            app.plugin_approval_scroll = app.plugin_approval_scroll.saturating_add(8);
        }
        KeyCode::PageUp => {
            app.plugin_approval_scroll = app.plugin_approval_scroll.saturating_sub(8);
        }
        _ => {}
    }
    false
}

pub(crate) fn handle_theme_picker_key(
    app: &mut TuiApp,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    let mut filtered = filtered_theme_options(&app.theme_filter);
    let selected_visible = filtered
        .iter()
        .position(|(orig_index, _)| *orig_index == app.selected_theme)
        .unwrap_or(0);
    let mut list_state = SelectListState::new(selected_visible, 0);
    match code {
        KeyCode::Esc => {
            app.theme_filter.clear();
            app.theme_filter_cursor = 0;
            super::close_active_modal(app);
        }
        KeyCode::Down => {
            list_state.select_next(filtered.len());
        }
        KeyCode::Up => {
            list_state.select_previous();
        }
        KeyCode::Enter => {
            if let Some(theme) = crate::theme::ThemeId::ALL.get(app.selected_theme) {
                app.set_theme(*theme);
            }
        }
        _ => {
            let before = app.theme_filter.clone();
            if handle_text_input_key(theme_filter_ref(app), code, modifiers, false)
                && app.theme_filter != before
            {
                filtered = filtered_theme_options(&app.theme_filter);
                list_state.reset();
                list_state.clamp(filtered.len());
            }
        }
    }
    if let Some((orig_index, _)) = filtered.get(list_state.selected()) {
        app.selected_theme = *orig_index;
    }
    false
}

pub(super) fn handle_mcp_key(app: &mut TuiApp, code: KeyCode, _modifiers: KeyModifiers) -> bool {
    let len = app
        .mcp_ui_state
        .live
        .len()
        .max(app.loaded_config.config.mcp.servers.len());

    match code {
        KeyCode::Esc => {
            if app.mcp_ui_state.is_focused_on_tools {
                app.mcp_ui_state.is_focused_on_tools = false;
            } else {
                super::close_active_modal(app);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if len > 0 {
                if app.mcp_ui_state.is_focused_on_tools {
                    let tool_len = app
                        .mcp_ui_state
                        .live
                        .get(app.mcp_ui_state.selected_server)
                        .map(|s| s.tools.len())
                        .unwrap_or(0);
                    if tool_len > 0 {
                        app.mcp_ui_state.selected_tool =
                            (app.mcp_ui_state.selected_tool + 1).min(tool_len - 1);
                    }
                } else {
                    app.mcp_ui_state.selected_server = app
                        .mcp_ui_state
                        .selected_server
                        .saturating_add(1)
                        .min(len - 1);
                    app.mcp_ui_state.selected_tool = 0;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if len > 0 {
                if app.mcp_ui_state.is_focused_on_tools {
                    app.mcp_ui_state.selected_tool =
                        app.mcp_ui_state.selected_tool.saturating_sub(1);
                } else {
                    app.mcp_ui_state.selected_server =
                        app.mcp_ui_state.selected_server.saturating_sub(1);
                    app.mcp_ui_state.selected_tool = 0;
                }
            }
        }
        KeyCode::Right | KeyCode::Char('l') => {
            app.mcp_ui_state.is_focused_on_tools = true;
            app.mcp_ui_state.selected_tool = 0;
        }
        KeyCode::Left | KeyCode::Char('h') => {
            app.mcp_ui_state.is_focused_on_tools = false;
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            crate::mcp_status::refresh_mcp_status(app);
        }
        KeyCode::Enter => {
            if !app.mcp_ui_state.is_focused_on_tools && len > 0 {
                let idx = app.mcp_ui_state.selected_server;
                if let Some(server) = app.loaded_config.config.mcp.servers.get_mut(idx) {
                    server.enabled = !server.enabled;
                    // Mirror into live view immediately.
                    if let Some(live) = app.mcp_ui_state.live.get_mut(idx) {
                        live.enabled = server.enabled;
                        if !server.enabled {
                            live.connected = false;
                            live.known = true;
                            live.tools.clear();
                        } else {
                            // Re-enabled: status unknown until next probe.
                            live.known = false;
                            live.connected = false;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    false
}

pub(crate) fn handle_background_commands_key(app: &mut TuiApp, code: KeyCode) -> bool {
    let len = app.background_commands.len();
    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Enter => {
            crate::background::open_background_command_output(app, app.bg_command_selected)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.bg_command_selected > 0 {
                app.bg_command_selected -= 1;
            }
            crate::background::clamp_background_selection(app);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.bg_command_selected + 1 < len {
                app.bg_command_selected += 1;
            }
            crate::background::clamp_background_selection(app);
        }
        KeyCode::Char('c') | KeyCode::Delete | KeyCode::Backspace => {
            crate::background::cancel_background_command_at(app, app.bg_command_selected);
        }
        KeyCode::Right | KeyCode::Char('l') => {
            // Arrow / chevron equivalent: open selected task.
            crate::background::open_background_command_output(app, app.bg_command_selected);
        }
        KeyCode::Char('r') => {
            crate::background::refresh_background_commands(app);
        }
        _ => {}
    }
    false
}

pub(crate) fn handle_background_command_output_key(app: &mut TuiApp, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
            super::replace_modal(app, ModalKind::BackgroundCommands);
        }
        KeyCode::Char('r') => crate::background::refresh_background_commands(app),
        KeyCode::Char('c') | KeyCode::Delete | KeyCode::Backspace => {
            crate::background::cancel_background_command_at(app, app.bg_command_selected);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.bg_command_output_follow = false;
            app.bg_command_output_scroll = app.bg_command_output_scroll.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.bg_command_output_follow = false;
            app.bg_command_output_scroll = app.bg_command_output_scroll.saturating_add(1);
        }
        KeyCode::PageUp => {
            app.bg_command_output_follow = false;
            app.bg_command_output_scroll = app.bg_command_output_scroll.saturating_sub(10);
        }
        KeyCode::PageDown => {
            app.bg_command_output_follow = false;
            app.bg_command_output_scroll = app.bg_command_output_scroll.saturating_add(10);
        }
        KeyCode::End | KeyCode::Char('f') => {
            app.bg_command_output_follow = true;
            app.bg_command_output_scroll = 0;
        }
        _ => {}
    }
    false
}

pub(crate) fn handle_model_routing_key(app: &mut TuiApp, code: KeyCode) -> bool {
    use crate::state::ModelRoutingTab;
    match code {
        KeyCode::Esc => {
            if app.setup_phase == Some(crate::state::SetupPhase::MemoryModel)
                && app.model_routing_tab == ModelRoutingTab::Agents
            {
                show_notification(
                    app,
                    "Setup",
                    "Choose a memory extraction model to continue setup.",
                );
            } else {
                super::close_active_modal(app);
            }
        }
        KeyCode::Left | KeyCode::Char('h') => {
            app.model_routing_tab = app.model_routing_tab.previous();
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
            app.model_routing_tab = app.model_routing_tab.next();
        }
        KeyCode::BackTab => {
            app.model_routing_tab = app.model_routing_tab.previous();
        }
        other => match app.model_routing_tab {
            ModelRoutingTab::Chat => match other {
                KeyCode::Enter => {
                    super::open_model_picker(app);
                }
                KeyCode::Char('e') => {
                    super::open_thinking_picker(app);
                }
                _ => {}
            },
            ModelRoutingTab::Agents => {
                // Reuse agent-list handling without Esc (already handled).
                handle_background_models_list_key(app, other);
            }
            ModelRoutingTab::Attachments => {
                handle_attachment_models_list_key(app, other);
            }
        },
    }
    false
}

/// List navigation for Agents tab (no Esc / no modal close).
fn handle_background_models_list_key(app: &mut TuiApp, code: KeyCode) {
    let len = BG_MODEL_TASKS.len();
    match code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.bg_models_selected > 0 {
                app.bg_models_selected -= 1;
            }
            clamp_bg_models_selection(app, len);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.bg_models_selected + 1 < len {
                app.bg_models_selected += 1;
            }
            clamp_bg_models_selection(app, len);
        }
        KeyCode::PageUp => {
            app.bg_models_selected = app.bg_models_selected.saturating_sub(3);
            clamp_bg_models_selection(app, len);
        }
        KeyCode::PageDown => {
            app.bg_models_selected = (app.bg_models_selected + 3).min(len.saturating_sub(1));
            clamp_bg_models_selection(app, len);
        }
        KeyCode::Home => {
            app.bg_models_selected = 0;
            clamp_bg_models_selection(app, len);
        }
        KeyCode::End => {
            app.bg_models_selected = len.saturating_sub(1);
            clamp_bg_models_selection(app, len);
        }
        KeyCode::Enter => {
            if let Some((task_id, _)) = BG_MODEL_TASKS.get(app.bg_models_selected) {
                app.bg_model_picker_active = true;
                app.attachment_model_picker_active = false;
                app.bg_model_picker_task = Some(task_id.to_string());
                open_bg_model_picker(app);
            }
        }
        KeyCode::Char('d') => {
            if let Some((task_id, _)) = BG_MODEL_TASKS.get(app.bg_models_selected) {
                if let Err(err) = app
                    .engine()
                    .clear_background_model(task_id, NaviConfigSaveTarget::Global)
                {
                    show_notification(
                        app,
                        "Agent Model Routes",
                        format!("Could not reset {task_id}: {err:#}"),
                    );
                    return;
                }
                clear_bg_model_override(app, task_id);
                show_notification(
                    app,
                    "Agent Model Routes",
                    format!("{task_id} reset to default."),
                );
            }
        }
        _ => {}
    }
}

/// Keep Agents selection in range and scroll the list so the row is visible.
fn clamp_bg_models_selection(app: &mut TuiApp, len: usize) {
    if len == 0 {
        app.bg_models_selected = 0;
        app.bg_models_scroll = 0;
        return;
    }
    app.bg_models_selected = app.bg_models_selected.min(len - 1);
    // Each task renders as 2 lines; keep ~4 tasks in the window.
    let visible_tasks = 4usize;
    if app.bg_models_selected < app.bg_models_scroll {
        app.bg_models_scroll = app.bg_models_selected;
    } else if app.bg_models_selected >= app.bg_models_scroll + visible_tasks {
        app.bg_models_scroll = app.bg_models_selected.saturating_sub(visible_tasks - 1);
    }
    app.bg_models_scroll = app.bg_models_scroll.min(len.saturating_sub(visible_tasks));
}

/// List navigation for Attachments tab (no Esc).
fn handle_attachment_models_list_key(app: &mut TuiApp, code: KeyCode) {
    const ATTACHMENT_MODALITIES: &[(&str, &str)] = &[
        ("image", "Image analysis fallback"),
        ("audio", "Audio analysis fallback"),
        ("video", "Video analysis fallback"),
        ("document", "Document analysis fallback"),
    ];
    let count = ATTACHMENT_MODALITIES.len();
    match code {
        KeyCode::Down | KeyCode::Char('j') => {
            if app.selected_attachment_model + 1 < count {
                app.selected_attachment_model += 1;
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.selected_attachment_model > 0 {
                app.selected_attachment_model -= 1;
            }
        }
        KeyCode::Enter => {
            if let Some((modality, _)) = ATTACHMENT_MODALITIES.get(app.selected_attachment_model) {
                app.attachment_model_picker_active = true;
                app.bg_model_picker_active = false;
                app.bg_model_picker_task = Some(modality.to_string());
                open_bg_model_picker(app);
            }
        }
        KeyCode::Char('d') => {
            if let Some((modality, _)) = ATTACHMENT_MODALITIES.get(app.selected_attachment_model) {
                if let Err(err) = app
                    .engine()
                    .clear_attachment_model(modality, NaviConfigSaveTarget::Global)
                {
                    show_notification(
                        app,
                        "Attachment Fallbacks",
                        format!("Could not reset {modality}: {err:#}"),
                    );
                    return;
                }
                clear_attachment_model_override(app, modality);
                show_notification(
                    app,
                    "Attachment Fallbacks",
                    format!("{} fallback reset to default.", modality),
                );
            }
        }
        _ => {}
    }
}

/// Open the shared model list used for agent routes and attachment fallbacks.
///
/// Selection starts on the first **model** row (often under "— Recent models —"),
/// never raw index `0` which may be absent from the filtered/available list —
/// that made Down a silent no-op.
fn open_bg_model_picker(app: &mut TuiApp) {
    app.model_scroll = 0;
    app.model_filter.clear();
    app.model_filter_cursor = 0;
    app.refresh_authenticated_providers();
    let rows = build_model_rows(app);
    app.bg_model_picker_selected = first_model_index(&rows).unwrap_or(0);
    sync_scroll_to_model_index(app, app.bg_model_picker_selected, &rows, 14);
    super::replace_modal(app, ModalKind::BgModelPicker);
}

const BG_MODEL_TASKS: &[(&str, &str)] = &[
    ("memory_extraction", "Automatic durable-memory extraction"),
    ("compaction", "Conversation summarization"),
    ("repo_search", "Repository exploration"),
    ("subagent_research", "Research subagents"),
    ("simple_code_edit", "Code edit subagents"),
];

pub(crate) fn handle_background_models_key(app: &mut TuiApp, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc => {
            if app.setup_phase == Some(crate::state::SetupPhase::MemoryModel) {
                show_notification(
                    app,
                    "Setup",
                    "Choose a memory extraction model to continue setup.",
                );
            } else {
                super::close_active_modal(app);
            }
        }
        // Reuse the same list navigation as the Model Routing → Agents tab.
        other => handle_background_models_list_key(app, other),
    }
    false
}

pub(crate) fn handle_bg_model_picker_key(
    app: &mut TuiApp,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    let rows = build_model_rows(app);
    let task_id = app.bg_model_picker_task.clone().unwrap_or_default();
    const VISIBLE_ROWS: u16 = 14;

    match code {
        KeyCode::Esc => {
            if app.setup_phase == Some(crate::state::SetupPhase::MemoryModel) {
                show_notification(
                    app,
                    "Setup",
                    "Choose a memory extraction model to continue setup.",
                );
                return false;
            }
            if app.attachment_model_picker_active {
                app.model_routing_tab = crate::state::ModelRoutingTab::Attachments;
            } else {
                app.model_routing_tab = crate::state::ModelRoutingTab::Agents;
            }
            app.attachment_model_picker_active = false;
            app.bg_model_picker_active = false;
            app.bg_model_picker_task = None;
            super::replace_modal(app, ModalKind::ModelRouting);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.bg_model_picker_selected =
                previous_model_index_from(app.bg_model_picker_selected, &rows);
            sync_scroll_to_model_index(
                app,
                app.bg_model_picker_selected,
                &rows,
                VISIBLE_ROWS.into(),
            );
        }
        KeyCode::Down | KeyCode::Char('j') => {
            // When selection is not in `rows` (stale 0 after open / filter),
            // next_model_index_from lands on the first model instead of no-op.
            app.bg_model_picker_selected =
                next_model_index_from(app.bg_model_picker_selected, &rows);
            sync_scroll_to_model_index(
                app,
                app.bg_model_picker_selected,
                &rows,
                VISIBLE_ROWS.into(),
            );
        }
        KeyCode::PageDown => {
            // Jump a few model rows for long catalogs.
            for _ in 0..5 {
                let next = next_model_index_from(app.bg_model_picker_selected, &rows);
                if next == app.bg_model_picker_selected {
                    break;
                }
                app.bg_model_picker_selected = next;
            }
            sync_scroll_to_model_index(
                app,
                app.bg_model_picker_selected,
                &rows,
                VISIBLE_ROWS.into(),
            );
        }
        KeyCode::PageUp => {
            for _ in 0..5 {
                let prev = previous_model_index_from(app.bg_model_picker_selected, &rows);
                if prev == app.bg_model_picker_selected {
                    break;
                }
                app.bg_model_picker_selected = prev;
            }
            sync_scroll_to_model_index(
                app,
                app.bg_model_picker_selected,
                &rows,
                VISIBLE_ROWS.into(),
            );
        }
        KeyCode::Enter => {
            if let Some(model) = app.models.get(app.bg_model_picker_selected) {
                let provider_id = model.provider_id.clone();
                let model_name = model.name.clone();
                if app.attachment_model_picker_active {
                    if let Err(err) = app.engine().set_attachment_model(
                        &task_id,
                        &provider_id,
                        &model_name,
                        NaviConfigSaveTarget::Global,
                    ) {
                        show_notification(
                            app,
                            "Attachment Fallbacks",
                            format!("Could not save {task_id}: {err:#}"),
                        );
                        return false;
                    }
                    set_attachment_model_override(app, &task_id, &provider_id, &model_name);
                    show_notification(
                        app,
                        "Attachment Fallbacks",
                        format!("{} fallback → {}:{}", task_id, provider_id, model_name),
                    );
                } else {
                    if let Err(err) = app.engine().set_background_model(
                        &task_id,
                        &provider_id,
                        &model_name,
                        NaviConfigSaveTarget::Global,
                    ) {
                        show_notification(
                            app,
                            "Agent Model Routes",
                            format!("Could not save {task_id}: {err:#}"),
                        );
                        return false;
                    }
                    set_bg_model_override(app, &task_id, &provider_id, &model_name);
                    show_notification(
                        app,
                        "Agent Model Routes",
                        format!("{} → {}:{}", task_id, provider_id, model_name),
                    );
                    if task_id == "memory_extraction"
                        && app.setup_phase == Some(crate::state::SetupPhase::MemoryModel)
                    {
                        super::close_all_modals(app);
                        crate::providers::maybe_start_setup_interview(app);
                        return false;
                    }
                }
            }
            if app.attachment_model_picker_active {
                app.model_routing_tab = crate::state::ModelRoutingTab::Attachments;
            } else {
                app.model_routing_tab = crate::state::ModelRoutingTab::Agents;
            }
            app.attachment_model_picker_active = false;
            app.bg_model_picker_active = false;
            app.bg_model_picker_task = None;
            super::replace_modal(app, ModalKind::ModelRouting);
        }
        KeyCode::Backspace => {
            let before = app.model_filter.clone();
            if handle_text_input_key(model_filter_ref(app), code, modifiers, false)
                && app.model_filter != before
            {
                app.model_scroll = 0;
                let rows = build_model_rows(app);
                app.bg_model_picker_selected =
                    first_model_index(&rows).unwrap_or(app.bg_model_picker_selected);
            }
        }
        KeyCode::Char('/') | KeyCode::Char('f') => {
            // Focus the filter input — handled by input routing.
        }
        _ => {
            let before = app.model_filter.clone();
            if handle_text_input_key(model_filter_ref(app), code, modifiers, false)
                && app.model_filter != before
            {
                app.model_scroll = 0;
                let rows = build_model_rows(app);
                app.bg_model_picker_selected =
                    first_model_index(&rows).unwrap_or(app.bg_model_picker_selected);
            }
        }
    }
    false
}

fn set_bg_model_override(app: &mut TuiApp, task: &str, provider: &str, model: &str) {
    use navi_sdk::BackgroundModelEntry;
    let bg = &mut app.loaded_config.config.background_models;
    let entry = BackgroundModelEntry {
        profile: None,
        provider: Some(provider.to_string()),
        model: Some(model.to_string()),
        fallback: None,
    };
    match task {
        "naming" => bg.naming = Some(entry),
        "memory_extraction" => bg.memory_extraction = Some(entry),
        "compaction" => bg.compaction = Some(entry),
        "repo_search" => bg.repo_search = Some(entry),
        "subagent_research" => bg.subagent_research = Some(entry),
        "simple_code_edit" => bg.simple_code_edit = Some(entry),
        _ => bg.default = Some(entry),
    }
}

fn clear_bg_model_override(app: &mut TuiApp, task: &str) {
    let bg = &mut app.loaded_config.config.background_models;
    match task {
        "naming" => bg.naming = None,
        "memory_extraction" => bg.memory_extraction = None,
        "compaction" => bg.compaction = None,
        "repo_search" => bg.repo_search = None,
        "subagent_research" => bg.subagent_research = None,
        "simple_code_edit" => bg.simple_code_edit = None,
        _ => bg.default = None,
    }
}

pub(crate) fn handle_attachment_models_key(app: &mut TuiApp, code: KeyCode) -> bool {
    const ATTACHMENT_MODALITIES: &[(&str, &str)] = &[
        ("image", "Image analysis fallback"),
        ("audio", "Audio analysis fallback"),
        ("video", "Video analysis fallback"),
        ("document", "Document analysis fallback"),
    ];
    let count = ATTACHMENT_MODALITIES.len();
    let mut list_state = SelectListState::new(app.selected_attachment_model, 0);

    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Down | KeyCode::Char('j') => {
            list_state.select_next(count);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            list_state.select_previous();
        }
        KeyCode::Enter => {
            if let Some((modality, _)) = ATTACHMENT_MODALITIES.get(app.selected_attachment_model) {
                app.attachment_model_picker_active = true;
                app.bg_model_picker_active = false;
                app.bg_model_picker_task = Some(modality.to_string());
                open_bg_model_picker(app);
            }
        }
        KeyCode::Char('d') => {
            if let Some((modality, _)) = ATTACHMENT_MODALITIES.get(app.selected_attachment_model) {
                if let Err(err) = app
                    .engine()
                    .clear_attachment_model(modality, NaviConfigSaveTarget::Global)
                {
                    show_notification(
                        app,
                        "Attachment Fallbacks",
                        format!("Could not reset {modality}: {err:#}"),
                    );
                    return false;
                }
                clear_attachment_model_override(app, modality);
                show_notification(
                    app,
                    "Attachment Fallbacks",
                    format!("{} fallback reset to default.", modality),
                );
            }
        }
        _ => {}
    }
    app.selected_attachment_model = list_state.selected();
    false
}

pub(crate) fn resolve_attachment_model_label(app: &TuiApp, modality: &str) -> String {
    let config = &app.loaded_config.config.attachment_models;
    let entry = match modality {
        "image" => config.image.as_ref(),
        "audio" => config.audio.as_ref(),
        "video" => config.video.as_ref(),
        "document" => config.document.as_ref(),
        _ => None,
    };
    if let Some(entry) = entry {
        return format!("{}:{}", entry.provider, entry.name);
    }
    "None (No Fallback)".to_string()
}

pub(crate) fn attachment_model_has_override(
    config: &navi_core::config::types::AttachmentModelsConfig,
    modality: &str,
) -> bool {
    match modality {
        "image" => config.image.is_some(),
        "audio" => config.audio.is_some(),
        "video" => config.video.is_some(),
        "document" => config.document.is_some(),
        _ => false,
    }
}

fn set_attachment_model_override(app: &mut TuiApp, modality: &str, provider: &str, model: &str) {
    use navi_core::config::types::ModelConfig;
    let entry = ModelConfig {
        provider: provider.to_string(),
        name: model.to_string(),
    };
    let config = &mut app.loaded_config.config.attachment_models;
    match modality {
        "image" => config.image = Some(entry),
        "audio" => config.audio = Some(entry),
        "video" => config.video = Some(entry),
        "document" => config.document = Some(entry),
        _ => {}
    }
}

fn clear_attachment_model_override(app: &mut TuiApp, modality: &str) {
    let config = &mut app.loaded_config.config.attachment_models;
    match modality {
        "image" => config.image = None,
        "audio" => config.audio = None,
        "video" => config.video = None,
        "document" => config.document = None,
        _ => {}
    }
}
