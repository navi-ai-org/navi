mod commands;
mod global;
mod input_modes;
mod modals;
mod provider_sync;
mod routing;

use crate::TuiApp;
use crate::notifications::show_notification;
use crate::providers::{build_model_rows, first_model_index};
use crate::session::load_saved_sessions;
use crate::state::{ModalKind, Mode};
use crate::ui::effect::UiEffect;
use crate::ui::keymap::KeyOutcome;
use crossterm::event::{KeyCode, KeyModifiers};
use navi_sdk::AgentMode;

// ─── shared helpers ──────────────────────────────────────────────────────────────

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

pub(crate) fn open_model_picker(app: &mut TuiApp) {
    replace_modal(app, ModalKind::Models);
    app.pending_model_selection = None;
    app.model_filter.clear();
    app.model_scroll = 0;

    let rows = build_model_rows(app);
    app.selected_model = first_model_index(&rows).unwrap_or(app.selected_model);
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

fn open_sessions_picker(app: &mut TuiApp) {
    app.saved_sessions = load_saved_sessions(&app.session_store);
    replace_modal(app, ModalKind::Sessions);
    app.selected_session = 0;
    app.session_scroll = 0;
}

// ─── routing dispatch ───────────────────────────────────────────────────────────

fn route_mode_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> KeyOutcome {
    let should_quit = match app.mode {
        Mode::Normal => self::input_modes::handle_normal_key(app, code, modifiers),
        Mode::Commands => self::commands::handle_command_key(app, code),
        Mode::Models => self::modals::handle_model_key(app, code, modifiers),
        Mode::ApiKeyEntry => self::modals::handle_api_key_key(app, code, modifiers),
        Mode::Thinking => self::modals::handle_thinking_key(app, code),
        Mode::Sessions => self::modals::handle_sessions_key(app, code),
        Mode::Settings => self::modals::handle_settings_key(app, code),
        Mode::Providers => self::modals::handle_providers_key(app, code),
        Mode::Debug => self::modals::handle_debug_key(app, code),
        Mode::Help => self::modals::handle_help_key(app, code),
    };
    if should_quit {
        KeyOutcome::Quit
    } else {
        KeyOutcome::Handled
    }
}

// ─── re-exports ─────────────────────────────────────────────────────────────────

// Re-exported for use by tests. The compiler warns about unused items because
// some are only consumed in #[cfg(test)] code.
#[allow(unused_imports)]
pub(crate) use commands::{handle_command_key, run_selected_command};
#[allow(unused_imports)]
pub(crate) use input_modes::handle_normal_key;
#[allow(unused_imports)]
pub(crate) use modals::{THINKING_OPTIONS, handle_help_key, handle_model_key, handle_settings_key};
#[allow(unused_imports)]
pub(crate) use provider_sync::sync_models_tui;
pub(crate) use routing::handle_key;
