use crate::TuiApp;
use crate::input::api_key_input_ref;
use crate::notifications::show_notification;
use crate::persistence::{load_session, save_current_session, save_preferences};
use crate::providers::{
    apply_model_selection, build_model_rows, first_model_index, model_is_available_for_selection,
    next_model_index, previous_model_index, save_api_key_and_rebuild, selected_model_in_rows,
    start_provider_oauth, sync_scroll_to_selection,
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
    let providers = app.filtered_providers();
    let mut list_state =
        SelectListState::new(app.selected_provider_setting, app.provider_settings_scroll);
    match code {
        KeyCode::Esc => {
            app.provider_filter.clear();
            super::close_active_modal(app);
        }
        KeyCode::Down => {
            list_state.select_next(providers.len());
            list_state.sync_scroll(12);
        }
        KeyCode::Up => {
            list_state.select_previous();
            list_state.sync_scroll(12);
        }
        KeyCode::Enter => {
            if let Some(provider) = providers.get(app.selected_provider_setting) {
                app.pending_provider_setup = Some(provider.id.clone());
                app.pending_model_selection = None;
                app.api_key_input.clear();
                app.api_key_cursor = 0;
                super::apply_ui_effect(app, UiEffect::OpenModal(ModalKind::ApiKeyEntry));
            }
        }
        KeyCode::Char('k') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(provider) = providers.get(app.selected_provider_setting) {
                app.pending_provider_setup = Some(provider.id.clone());
                app.pending_model_selection = None;
                app.api_key_input.clear();
                app.api_key_cursor = 0;
                super::apply_ui_effect(app, UiEffect::OpenModal(ModalKind::ApiKeyEntry));
            }
        }
        KeyCode::Char('o') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(provider) = providers.get(app.selected_provider_setting) {
                start_provider_oauth(app, provider);
            }
        }
        KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(provider) = providers.get(app.selected_provider_setting) {
                let provider_id = provider.id.clone();
                super::provider_sync::sync_provider_tui(app, &provider_id);
            }
        }
        KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(provider) = providers.get(app.selected_provider_setting) {
                let _ = app.credential_store().delete_api_key(&provider.id);
            }
        }
        KeyCode::Char(ch) => {
            app.provider_filter.push(ch);
            list_state = SelectListState::new(0, 0);
        }
        KeyCode::Backspace => {
            app.provider_filter.pop();
            list_state = SelectListState::new(0, 0);
        }
        _ => {}
    }
    app.selected_provider_setting = list_state.selected();
    app.provider_settings_scroll = list_state.scroll();
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
