use crate::TuiApp;
use crate::tools::{approve_pending_tool, cancel_stream, deny_pending_tool};
use crate::ui::keymap::KeyOutcome;
use crossterm::event::{KeyCode, KeyModifiers};

use super::global::{is_copy_selection_key, route_global_key};

pub(crate) fn handle_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    route_key(app, code, modifiers).should_quit()
}

pub(crate) fn route_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> KeyOutcome {
    app.hover_index = None;

    if code != KeyCode::Esc {
        app.cancel_esc_pressed = false;
    }

    let approval = route_approval_key(app, code);
    if approval.is_handled() {
        return approval;
    }

    let normal_cancel = route_normal_cancel_key(app, code);
    if normal_cancel.is_handled() {
        return normal_cancel;
    }

    if app.mode == crate::state::Mode::Question && !is_question_global_control_key(code, modifiers)
    {
        return super::route_mode_key(app, code, modifiers);
    }

    let global = route_global_key(app, code, modifiers);
    if global.is_handled() {
        return global;
    }

    super::route_mode_key(app, code, modifiers)
}

fn is_question_global_control_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL)
        && (matches!(code, KeyCode::Char('c')) || is_copy_selection_key(code, modifiers))
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
    if app.mode == crate::state::Mode::Normal
        && code == KeyCode::Esc
        && (app.is_loading || app.has_async_task())
    {
        if app.cancel_esc_pressed {
            cancel_stream(app);
            app.cancel_esc_pressed = false;
        } else {
            app.cancel_esc_pressed = true;
            crate::notifications::show_notification(app, "Cancel", "Press Esc again to stop");
        }
        return KeyOutcome::Handled;
    }
    KeyOutcome::Ignored
}
