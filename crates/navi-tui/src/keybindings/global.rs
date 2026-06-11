use crate::TuiApp;
use crate::chat::{reset_system_context, submit_message};
use crate::mouse::{copy_text_to_clipboard, selected_text};
use crate::notifications::show_notification;
use crate::persistence::save_preferences;
use crate::state::ModalKind;
use crate::ui::effect::UiEffect;
use crate::ui::keymap::KeyOutcome;
use crossterm::event::{KeyCode, KeyModifiers};

pub(super) fn route_global_key(
    app: &mut TuiApp,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> KeyOutcome {
    if modifiers.contains(KeyModifiers::CONTROL) {
        if is_copy_selection_key(code, modifiers) {
            if let Some(text) = selected_text(app) {
                copy_text_to_clipboard(app, &text);
            }
            return KeyOutcome::Handled;
        }

        match code {
            KeyCode::Char('c') => return super::apply_ui_effect(app, UiEffect::Quit),
            KeyCode::Char('d') => {
                if app.mode == crate::state::Mode::Providers {
                    return KeyOutcome::Ignored;
                }
                let outcome = super::apply_ui_effect(app, UiEffect::ReplaceModal(ModalKind::Debug));
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
                save_preferences(app);
                return KeyOutcome::Handled;
            }
            KeyCode::Char('p') => {
                let outcome =
                    super::apply_ui_effect(app, UiEffect::ReplaceModal(ModalKind::Commands));
                app.command_filter.clear();
                app.selected_command = 0;
                app.command_scroll = 0;
                return outcome;
            }
            KeyCode::Char('.') => {
                return super::apply_ui_effect(app, UiEffect::ReplaceModal(ModalKind::Help));
            }
            KeyCode::Char('m') => {
                super::open_model_picker(app);
                return KeyOutcome::Handled;
            }
            KeyCode::Char('s') => {
                super::open_sessions_picker(app);
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
                save_preferences(app);
                return KeyOutcome::Handled;
            }
            KeyCode::Char('j') | KeyCode::Char('\n') | KeyCode::Char('\r') | KeyCode::Enter => {
                if !app.pending_questions.is_empty() {
                    return super::apply_ui_effect(
                        app,
                        UiEffect::ReplaceModal(ModalKind::Question),
                    );
                }
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
                let outcome = super::apply_ui_effect(app, UiEffect::CloseAllModals);
                show_notification(app, "Layer", "New layer started.");
                return outcome;
            }
            _ => {}
        }
    }
    KeyOutcome::Ignored
}

pub(super) fn is_copy_selection_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    // Terminals differ: Ctrl+Shift+C may arrive as uppercase 'C' or as
    // lowercase 'c' plus an explicit SHIFT modifier. Plain Ctrl+C must quit.
    matches!(code, KeyCode::Char('C'))
        || (matches!(code, KeyCode::Char('c')) && modifiers.contains(KeyModifiers::SHIFT))
}
