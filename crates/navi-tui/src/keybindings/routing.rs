use crate::TuiApp;
use crate::tools::{approve_pending_tool, deny_pending_tool};
use crate::ui::KeyOutcome;
use crossterm::event::{KeyCode, KeyModifiers};

use super::global::{route_global_key, route_system_global_key};

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

    let system_global = route_system_global_key(app, code, modifiers);
    if system_global.is_handled() {
        return system_global;
    }

    if app.modal_stack.is_active() && app.mode != crate::state::Mode::Normal {
        return super::route_mode_key(app, code, modifiers);
    }

    let global = route_global_key(app, code, modifiers);
    if global.is_handled() {
        return global;
    }

    super::route_mode_key(app, code, modifiers)
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
        crate::keybindings::replace_modal(app, crate::state::ModalKind::ConfirmCancelTurn);
        return KeyOutcome::Handled;
    }
    KeyOutcome::Ignored
}
