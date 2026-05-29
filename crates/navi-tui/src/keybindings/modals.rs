use crate::TuiApp;
use crate::input::api_key_input_ref;
use crate::notifications::show_notification;
use crate::persistence::{load_session, save_current_session};
use crate::providers::{
    apply_model_selection, build_model_rows, first_model_index, model_is_available_for_selection,
    next_model_index, previous_model_index, save_api_key_and_rebuild, selected_model_in_rows,
    start_provider_oauth, sync_scroll_to_selection,
};
use crate::session::load_saved_sessions;
use crate::state::{ModalKind, ThinkingLevel};
use crate::ui::effect::UiEffect;
use crate::ui::list::SelectListState;
use crossterm::event::{KeyCode, KeyModifiers};
use navi_sdk::provider_catalog;

pub(crate) const THINKING_OPTIONS: &[ThinkingLevel] = &[
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
            super::close_all_modals(app);
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

pub(crate) fn handle_providers_key(app: &mut TuiApp, code: KeyCode) -> bool {
    let providers = provider_catalog(&app.loaded_config.config);
    let mut list_state =
        SelectListState::new(app.selected_provider_setting, app.provider_settings_scroll);
    match code {
        KeyCode::Esc => super::close_active_modal(app),
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
                super::apply_ui_effect(app, UiEffect::OpenModal(ModalKind::ApiKeyEntry));
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
                super::provider_sync::sync_provider_tui(app, &provider_id);
            }
        }
        _ => {}
    }
    app.selected_provider_setting = list_state.selected();
    app.provider_settings_scroll = list_state.scroll();
    false
}

pub(crate) fn handle_sessions_key(app: &mut TuiApp, code: KeyCode) -> bool {
    let mut list_state = SelectListState::new(app.selected_session, app.session_scroll);
    match code {
        KeyCode::Esc => super::close_active_modal(app),
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
            super::close_all_modals(app);
        }
        KeyCode::Delete => {
            if let Some(snapshot) = app.saved_sessions.get(app.selected_session) {
                let _ = app.engine().delete_saved_session(snapshot.id.as_str());
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

pub(crate) fn handle_model_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    let rows = build_model_rows(app);
    let visible_rows = 14u16;
    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
            super::provider_sync::sync_models_tui(app);
            super::close_all_modals(app);
        }
        KeyCode::Char('e') if modifiers.contains(KeyModifiers::CONTROL) => {
            if selected_model_in_rows(&rows, app.selected_model).is_some() {
                app.pending_model_selection = Some(app.selected_model);
                super::replace_modal(app, ModalKind::ApiKeyEntry);
                app.api_key_input.clear();
                app.api_key_cursor = 0;
            }
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
