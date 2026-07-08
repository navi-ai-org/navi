mod commands;
mod global;
mod input_modes;
pub(crate) mod modals;
mod provider_sync;
mod routing;

use crate::TuiApp;
use crate::providers::{build_model_rows, first_model_index};
use crate::session::load_saved_sessions;
use crate::state::{ModalKind, Mode};
use crate::ui::KeyOutcome;
use crate::ui::UiEffect;
use crossterm::event::{KeyCode, KeyModifiers};

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

pub(crate) fn open_modal(app: &mut TuiApp, modal: ModalKind) {
    app.modal_stack.open(modal);
    app.mode = modal.mode();
    app.hover_index = None;
}

pub(crate) fn replace_modal(app: &mut TuiApp, modal: ModalKind) {
    app.modal_stack.replace(Some(modal));
    app.mode = modal.mode();
    app.hover_index = None;
}

pub(crate) fn close_active_modal(app: &mut TuiApp) {
    let was_message_actions = app.mode == Mode::MessageActions;
    let was_oauth = app.mode == Mode::OAuth;
    app.modal_stack.close();
    app.mode = app
        .modal_stack
        .top()
        .map(ModalKind::mode)
        .unwrap_or(Mode::Normal);
    app.hover_index = None;
    if was_message_actions {
        app.message_action_target = None;
        app.selected_message_action = 0;
        app.hovered_chat_source = None;
    }
    if was_oauth {
        app.oauth_state = None;
    }
}

pub(crate) fn close_all_modals(app: &mut TuiApp) {
    app.modal_stack.clear();
    app.mode = Mode::Normal;
    app.hover_index = None;
    app.message_action_target = None;
    app.selected_message_action = 0;
    app.hovered_chat_source = None;
    app.oauth_state = None;
}

pub(crate) fn open_model_picker(app: &mut TuiApp) {
    replace_modal(app, ModalKind::Models);
    app.pending_model_selection = None;
    app.model_filter.clear();
    app.model_filter_cursor = 0;
    app.model_scroll = 0;
    app.refresh_authenticated_providers();

    let rows = build_model_rows(app);
    // Keep the current selection if it still exists in the filtered rows;
    // otherwise fall back to the first model.
    let currently_selected = app.selected_model;
    if crate::providers::selected_model_in_rows(&rows, currently_selected).is_some() {
        app.selected_model = currently_selected;
    } else {
        app.selected_model = first_model_index(&rows).unwrap_or(currently_selected);
    }
    app.model_scroll = 0;
    crate::providers::sync_scroll_to_selection(app, &rows, 14);
}

fn open_provider_settings(app: &mut TuiApp) {
    replace_modal(app, ModalKind::Providers);
    app.provider_filter.clear();
    app.provider_filter_cursor = 0;
    app.provider_settings_scroll = 0;
    // Land on the first non-header row so the highlight is on a selectable
    // provider instead of a "— Recent —" divider.
    let rows = app.filtered_providers();
    app.selected_provider_setting = rows
        .iter()
        .position(|row| matches!(row, crate::providers::ProviderListRow::Provider { .. }))
        .unwrap_or(0);
}

fn open_thinking_picker(app: &mut TuiApp) {
    replace_modal(app, ModalKind::Thinking);
    app.selected_thinking = app.thinking_level.index();
}

fn open_sessions_picker(app: &mut TuiApp) {
    app.saved_sessions = load_saved_sessions(&app.session_store);
    replace_modal(app, ModalKind::Sessions);
    app.selected_session = 0;
    app.session_scroll = 0;
    app.session_filter.clear();
    app.session_filter_cursor = 0;
}

fn open_skills_picker(app: &mut TuiApp) {
    app.refresh_skills();
    replace_modal(app, ModalKind::Skills);
    app.selected_skill = 0;
    app.skill_filter.clear();
    app.skill_filter_cursor = 0;
    app.skill_scroll = 0;
}

fn open_plugins_picker(app: &mut TuiApp) {
    replace_modal(app, ModalKind::Plugins);
    app.selected_plugin_row = 0;
    app.plugin_row_scroll = 0;
    crate::plugins::refresh_plugin_catalog(app);
}

// ─── routing dispatch ───────────────────────────────────────────────────────────

fn route_mode_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> KeyOutcome {
    let should_quit = match app.mode {
        Mode::Normal => self::input_modes::handle_normal_key(app, code, modifiers),
        Mode::Commands => self::commands::handle_command_key(app, code, modifiers),
        Mode::Models => self::modals::handle_model_key(app, code, modifiers),
        Mode::ApiKeyEntry => self::modals::handle_api_key_key(app, code, modifiers),
        Mode::Thinking => self::modals::handle_thinking_key(app, code),
        Mode::Sessions => self::modals::handle_sessions_key(app, code, modifiers),
        Mode::Settings => self::modals::handle_settings_key(app, code),
        Mode::Providers => self::modals::handle_providers_key(app, code, modifiers),
        Mode::Usage => self::modals::handle_usage_key(app, code),
        Mode::Debug => self::modals::handle_debug_key(app, code),
        Mode::Help => self::modals::handle_help_key(app, code),
        Mode::Skills => self::modals::handle_skills_key(app, code, modifiers),
        Mode::Plugins => self::modals::handle_plugins_key(app, code),
        Mode::PluginApproval => self::modals::handle_plugin_approval_key(app, code, modifiers),
        Mode::Question => self::modals::handle_question_key(app, code, modifiers),
        Mode::ThemePicker => self::modals::handle_theme_picker_key(app, code, modifiers),
        Mode::MessageActions => self::modals::handle_message_actions_key(app, code),
        Mode::Mcp => self::modals::handle_mcp_key(app, code, modifiers),
        Mode::OAuth => self::modals::handle_oauth_key(app, code, modifiers),
        Mode::BackgroundCommands => self::modals::handle_background_commands_key(app, code),
        Mode::BackgroundCommandOutput => {
            self::modals::handle_background_command_output_key(app, code)
        }
        Mode::BackgroundModels => self::modals::handle_background_models_key(app, code),
        Mode::BgModelPicker => self::modals::handle_bg_model_picker_key(app, code, modifiers),
        Mode::Setup => self::input_modes::handle_normal_key(app, code, modifiers),
        Mode::AttachmentModels => self::modals::handle_attachment_models_key(app, code),
        Mode::MessageQueue => self::modals::handle_message_queue_key(app, code, modifiers),
        Mode::QueuedMessageEdit => {
            self::modals::handle_queued_message_edit_key(app, code, modifiers)
        }
        Mode::ConfirmCancelTurn => self::modals::handle_confirm_cancel_turn_key(app, code),
        Mode::Plan | Mode::ConfirmPlan => {
            self::input_modes::handle_normal_key(app, code, modifiers)
        }
    };
    if should_quit {
        KeyOutcome::Quit
    } else {
        KeyOutcome::Handled
    }
}

// ─── re-exports ─────────────────────────────────────────────────────────────────

pub(crate) use modals::THINKING_OPTIONS;
pub(crate) use routing::handle_key;

// Test-only re-exports: sub-modules are private, so tests need these paths.
// Production code uses direct `self::` paths instead.
#[allow(unused_imports)]
pub(crate) use commands::{handle_command_key, run_selected_command};
#[allow(unused_imports)]
pub(crate) use input_modes::handle_normal_key;
#[allow(unused_imports)]
pub(crate) use modals::{handle_help_key, handle_model_key, handle_settings_key};
#[allow(unused_imports)]
pub(crate) use provider_sync::sync_models_tui;
