use crate::TuiApp;
use crate::input::api_key_input_ref;
use crate::mouse::copy_text_to_clipboard;
use crate::notifications::show_notification;
use crate::persistence::{load_session, save_current_session, save_preferences};
use crate::providers::{
    ListRow, apply_model_selection, build_model_rows, first_model_index,
    model_is_available_for_selection, next_model_index, previous_model_index, rebuild_provider,
    save_api_key_and_rebuild, selected_model_in_rows, start_provider_oauth,
    sync_scroll_to_model_index, sync_scroll_to_selection,
};
use crate::session::load_saved_sessions;
use crate::state::{MessageAction, ModalKind, ThinkingLevel};
use crate::theme::filtered_theme_options;
use crate::ui::effect::UiEffect;
use crate::ui::list::SelectListState;
use crossterm::event::{KeyCode, KeyModifiers};
use navi_sdk::QuestionResponse;

use crate::runtime::spawn_runtime_task;

pub(crate) const THINKING_OPTIONS: &[ThinkingLevel] = &[
    ThinkingLevel::Adaptive,
    ThinkingLevel::Max,
    ThinkingLevel::High,
    ThinkingLevel::Medium,
    ThinkingLevel::Low,
    ThinkingLevel::Off,
];

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
    match code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') => {
            super::apply_ui_effect(app, UiEffect::CloseModal);
        }
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
    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Down => {
            app.selected_thinking = (app.selected_thinking + 1).min(THINKING_OPTIONS.len() - 1);
        }
        KeyCode::Up => {
            app.selected_thinking = app.selected_thinking.saturating_sub(1);
        }
        KeyCode::Enter => {
            let level = THINKING_OPTIONS[app.selected_thinking];
            app.thinking_level = level;
            app.selected_thinking = level.index();
            super::close_all_modals(app);
            show_notification(
                app,
                "Thinking",
                format!("Thinking set to {}.", level.label()),
            );
            save_preferences(app);
        }
        _ => {}
    }

    false
}

pub(crate) fn handle_settings_key(app: &mut TuiApp, code: KeyCode) -> bool {
    const SETTINGS_COUNT: usize = 4;
    const COMPACT_TOOL_LIMITS: &[usize] = &[3, 5, 8, 12, 20];
    let mut list_state = SelectListState::new(app.selected_setting, 0);
    match code {
        KeyCode::Esc => super::close_active_modal(app),
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
                save_preferences(app);
            }
            1 => {
                app.full_tool_view = !app.full_tool_view;
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
            2 => {
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
            3 => {
                app.theme_filter.clear();
                super::replace_modal(app, ModalKind::ThemePicker);
            }
            _ => {}
        },
        _ => {}
    }
    app.selected_setting = list_state.selected();
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
                    let _ = app.credential_store().set_project_account(
                        &app.project_dir,
                        &provider.id,
                        &account_id,
                    );
                    rebuild_provider(app);
                    super::close_active_modal(app);
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
            if let Some(provider) = provider_at_row(&list_rows, &catalog, current_row_pos).cloned()
            {
                start_provider_oauth(app, &provider);
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
        KeyCode::Char(ch) => {
            app.provider_filter.push(ch);
            reset_to_first = true;
        }
        KeyCode::Backspace => {
            app.provider_filter.pop();
            reset_to_first = true;
        }
        _ => {}
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
        _ => {}
    }
    false
}

pub(crate) fn handle_sessions_key(app: &mut TuiApp, code: KeyCode) -> bool {
    let sessions = app.filtered_sessions();
    let mut list_state = SelectListState::new(app.selected_session, app.session_scroll);
    match code {
        KeyCode::Esc => {
            app.session_filter.clear();
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
            let snapshot = sessions.get(app.selected_session).copied().cloned();
            drop(sessions);
            if let Some(snapshot) = snapshot {
                save_current_session(app);
                load_session(app, &snapshot);
            }
            app.session_filter.clear();
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
        KeyCode::Char(ch) => {
            app.session_filter.push(ch);
            app.session_scroll = 0;
            app.selected_session = 0;
        }
        KeyCode::Backspace => {
            app.session_filter.pop();
            app.session_scroll = 0;
            app.selected_session = 0;
        }
        _ => {}
    }
    app.selected_session = list_state.selected();
    app.session_scroll = list_state.scroll();

    false
}

pub(crate) fn handle_api_key_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
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
            _ => return false,
        }
    }

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

pub(crate) fn handle_skills_key(app: &mut TuiApp, code: KeyCode) -> bool {
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
        KeyCode::Char(ch) => {
            app.skill_filter.push(ch);
            app.skill_scroll = 0;
            app.selected_skill = 0;
        }
        KeyCode::Backspace => {
            app.skill_filter.pop();
            app.skill_scroll = 0;
            app.selected_skill = 0;
        }
        _ => {}
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
                super::close_all_modals(app);
                crate::providers::maybe_start_setup_interview(app);
            } else {
                app.pending_model_selection = Some(app.selected_model);
                super::replace_modal(app, ModalKind::ApiKeyEntry);
                app.api_key_input.clear();
                app.api_key_cursor = 0;
            }
        }
        _ => {}
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

pub(crate) fn handle_theme_picker_key(app: &mut TuiApp, code: KeyCode) -> bool {
    let mut filtered = filtered_theme_options(&app.theme_filter);
    let selected_visible = filtered
        .iter()
        .position(|(orig_index, _)| *orig_index == app.selected_theme)
        .unwrap_or(0);
    let mut list_state = SelectListState::new(selected_visible, 0);
    match code {
        KeyCode::Esc => {
            app.theme_filter.clear();
            super::close_active_modal(app);
        }
        KeyCode::Char(ch) => {
            app.theme_filter.push(ch);
            filtered = filtered_theme_options(&app.theme_filter);
            list_state.reset();
        }
        KeyCode::Backspace => {
            app.theme_filter.pop();
            filtered = filtered_theme_options(&app.theme_filter);
            list_state.clamp(filtered.len());
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
        _ => {}
    }
    if let Some((orig_index, _)) = filtered.get(list_state.selected()) {
        app.selected_theme = *orig_index;
    }
    false
}

pub(super) fn handle_mcp_key(app: &mut TuiApp, code: KeyCode, _modifiers: KeyModifiers) -> bool {
    let len = app.loaded_config.config.mcp.servers.len();

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
                    app.mcp_ui_state.selected_tool =
                        app.mcp_ui_state.selected_tool.saturating_add(1);
                } else {
                    app.mcp_ui_state.selected_server = app
                        .mcp_ui_state
                        .selected_server
                        .saturating_add(1)
                        .min(len - 1);
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
        KeyCode::Enter => {
            if !app.mcp_ui_state.is_focused_on_tools && len > 0 {
                let idx = app.mcp_ui_state.selected_server;
                if let Some(server) = app.loaded_config.config.mcp.servers.get_mut(idx) {
                    server.enabled = !server.enabled;
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
        KeyCode::Up | KeyCode::Char('k') => {
            if app.bg_command_selected > 0 {
                app.bg_command_selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.bg_command_selected + 1 < len {
                app.bg_command_selected += 1;
            }
        }
        KeyCode::Char('c') => {
            // Cancel selected background command
            if let Some(cmd) = app.background_commands.get(app.bg_command_selected) {
                if cmd.is_running() {
                    let task_id = cmd.task_id.clone();
                    let engine = app.engine();
                    let session_id = app.session_id.as_str().to_string();
                    spawn_runtime_task(async move {
                        let _ = engine
                            .cancel_background_command(&session_id, &task_id)
                            .await;
                    });
                }
            }
        }
        KeyCode::Char('r') => {
            // Refresh background commands
            let engine = app.engine();
            let session_id = app.session_id.as_str().to_string();
            let tx = app.async_sender();
            spawn_runtime_task(async move {
                if let Ok(commands) = engine.list_background_commands(&session_id).await {
                    let _ = tx.send(crate::dispatch::AsyncEvent::BackgroundCommandsUpdated(
                        commands,
                    ));
                }
            });
        }
        _ => {}
    }
    false
}

const BG_MODEL_TASKS: &[(&str, &str)] = &[
    ("naming", "Session title generation"),
    ("compaction", "Conversation summarization"),
    ("repo_search", "Repository exploration"),
    ("subagent_research", "Research subagents"),
    ("simple_code_edit", "Code edit subagents"),
];

pub(crate) fn handle_background_models_key(app: &mut TuiApp, code: KeyCode) -> bool {
    let len = BG_MODEL_TASKS.len();
    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Up | KeyCode::Char('k') => {
            if app.bg_models_selected > 0 {
                app.bg_models_selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.bg_models_selected + 1 < len {
                app.bg_models_selected += 1;
            }
        }
        KeyCode::Enter => {
            // Open model picker sub-modal for the selected task.
            if let Some((task_id, _)) = BG_MODEL_TASKS.get(app.bg_models_selected) {
                app.bg_model_picker_active = true;
                app.bg_model_picker_task = Some(task_id.to_string());
                app.bg_model_picker_selected = 0;
                app.model_scroll = 0;
                app.model_filter.clear();
                super::replace_modal(app, ModalKind::BgModelPicker);
                app.refresh_authenticated_providers();
            }
        }
        KeyCode::Char('d') => {
            // Reset selected task to default (remove override).
            if let Some((task_id, _)) = BG_MODEL_TASKS.get(app.bg_models_selected) {
                clear_bg_model_override(app, task_id);
                save_preferences(app);
                show_notification(
                    app,
                    "Background Agents",
                    format!("{task_id} reset to default."),
                );
            }
        }
        _ => {}
    }
    false
}

pub(crate) fn handle_bg_model_picker_key(app: &mut TuiApp, code: KeyCode) -> bool {
    let rows = build_model_rows(app);
    let task_id = app.bg_model_picker_task.clone().unwrap_or_default();
    const VISIBLE_ROWS: u16 = 14;

    match code {
        KeyCode::Esc => {
            // Go back to the background models list.
            app.bg_model_picker_active = false;
            app.bg_model_picker_task = None;
            super::replace_modal(app, ModalKind::BackgroundModels);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(current) = rows.iter().position(|row| match row {
                ListRow::Model { index } => *index == app.bg_model_picker_selected,
                _ => false,
            }) {
                if let Some(prev) = rows.iter().take(current).rev().find_map(|row| match row {
                    ListRow::Model { index } => Some(*index),
                    _ => None,
                }) {
                    app.bg_model_picker_selected = prev;
                }
            }
            sync_scroll_to_model_index(
                app,
                app.bg_model_picker_selected,
                &rows,
                VISIBLE_ROWS.into(),
            );
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(current) = rows.iter().position(|row| match row {
                ListRow::Model { index } => *index == app.bg_model_picker_selected,
                _ => false,
            }) {
                if let Some(next) = rows.iter().skip(current + 1).find_map(|row| match row {
                    ListRow::Model { index } => Some(*index),
                    _ => None,
                }) {
                    app.bg_model_picker_selected = next;
                }
            }
            sync_scroll_to_model_index(
                app,
                app.bg_model_picker_selected,
                &rows,
                VISIBLE_ROWS.into(),
            );
        }
        KeyCode::Enter => {
            // Apply the selected model to the background task.
            if let Some(model) = app.models.get(app.bg_model_picker_selected) {
                let provider_id = model.provider_id.clone();
                let model_name = model.name.clone();
                set_bg_model_override(app, &task_id, &provider_id, &model_name);
                save_preferences(app);
                show_notification(
                    app,
                    "Background Agents",
                    format!("{} → {}:{}", task_id, provider_id, model_name),
                );
            }
            app.bg_model_picker_active = false;
            app.bg_model_picker_task = None;
            super::replace_modal(app, ModalKind::BackgroundModels);
        }
        KeyCode::Backspace => {
            app.model_filter.pop();
            app.model_scroll = 0;
            let rows = build_model_rows(app);
            app.bg_model_picker_selected =
                first_model_index(&rows).unwrap_or(app.bg_model_picker_selected);
        }
        KeyCode::Char('/') | KeyCode::Char('f') => {
            // Focus the filter input — handled by input routing.
        }
        _ => {
            // Forward printable chars to the model filter.
            if let KeyCode::Char(c) = code {
                app.model_filter.push(c);
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
        "compaction" => bg.compaction = None,
        "repo_search" => bg.repo_search = None,
        "subagent_research" => bg.subagent_research = None,
        "simple_code_edit" => bg.simple_code_edit = None,
        _ => bg.default = None,
    }
}
