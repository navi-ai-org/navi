use crate::TuiApp;
use crate::chat::start_new_session;
use crate::clipboard::try_read_clipboard_image;
use crate::mouse::{copy_text_to_clipboard, selected_text};
use crate::notifications::show_notification;
use crate::persistence::save_preferences;
use crate::runtime::spawn_runtime_task;
use crate::state::ModalKind;
use crate::ui::KeyOutcome;
use crate::ui::UiEffect;
use crossterm::event::{KeyCode, KeyModifiers};
use navi_core::PermissionMode;

/// True when CONTROL is held (SHIFT allowed for case variants; ALT blocks).
fn has_control(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) && !modifiers.contains(KeyModifiers::ALT)
}

/// Match `Ctrl+letter` case-insensitively. Terminals often send uppercase
/// (`Char('M')`) for Ctrl+M rather than lowercase + CONTROL.
fn ctrl_letter(code: KeyCode, letter: char) -> bool {
    matches!(code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&letter))
}

pub(super) fn route_global_key(
    app: &mut TuiApp,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> KeyOutcome {
    if !has_control(modifiers) {
        return KeyOutcome::Ignored;
    }

    if is_copy_selection_key(code, modifiers) {
        // Prefer the selected chat cell (full block). Fall back to drag selection.
        if app.selected_chat_source.is_some() {
            crate::chat_blocks::copy_selected_block(app);
        } else if let Some(text) = selected_text(app) {
            copy_text_to_clipboard(app, &text);
        }
        return KeyOutcome::Handled;
    }

    // Plain Ctrl+C quits (not Ctrl+Shift+C — that's copy above).
    if matches!(code, KeyCode::Char('c')) && !modifiers.contains(KeyModifiers::SHIFT) {
        return super::apply_ui_effect(app, UiEffect::Quit);
    }

    if ctrl_letter(code, 'd') {
        if app.mode == crate::state::Mode::Providers {
            return KeyOutcome::Ignored;
        }
        let outcome = super::apply_ui_effect(app, UiEffect::ReplaceModal(ModalKind::Debug));
        tracing::info!("debug modal opened");
        return outcome;
    }

    if ctrl_letter(code, 'g') {
        let mode = if app.yolo_mode {
            PermissionMode::Restricted
        } else {
            PermissionMode::Yolo
        };
        set_permission_mode(app, mode);
        return KeyOutcome::Handled;
    }

    if ctrl_letter(code, 'p') {
        super::commands::open_command_palette(app);
        return KeyOutcome::Handled;
    }

    if ctrl_letter(code, 'q') {
        return super::apply_ui_effect(app, UiEffect::ReplaceModal(ModalKind::MessageQueue));
    }

    if matches!(code, KeyCode::Char('.')) {
        crate::view::help::open_help(app);
        return KeyOutcome::Handled;
    }

    if ctrl_letter(code, 'm') {
        super::open_model_picker(app);
        return KeyOutcome::Handled;
    }

    if ctrl_letter(code, 's') {
        super::open_sessions_picker(app);
        return KeyOutcome::Handled;
    }

    if ctrl_letter(code, 'i') || ctrl_letter(code, 'v') {
        // Paste into the normal chat composer only. In other modes (OAuth
        // paste, text fields, …) yield so the modal/mode handler can run.
        // Allowed while streaming — drafts queue on submit behind the active turn.
        if app.mode != crate::state::Mode::Normal {
            return KeyOutcome::Ignored;
        }
        let want_image_only = ctrl_letter(code, 'i');
        match try_read_clipboard_image() {
            Some(image) => {
                app.pending_images.push(image);
                let tag = format!("[Image {}]", app.pending_images.len());
                crate::input::insert_input_text(app, &tag);
                show_notification(app, "Image", format!("Attached as {}", tag));
            }
            None if want_image_only => {
                show_notification(app, "Image", "No image found in clipboard.");
            }
            None => {
                // Ctrl+V with no image: paste clipboard text into the composer.
                if let Some(text) = crate::clipboard::try_read_clipboard_text() {
                    if !text.is_empty() {
                        crate::input::insert_input_text(app, &text);
                    }
                }
            }
        }
        return KeyOutcome::Handled;
    }

    if ctrl_letter(code, 'o') {
        // Providers: start OAuth. OAuth modal: reopen browser. Do not steal
        // those for the chat-wide tool-output expand toggle.
        if matches!(
            app.mode,
            crate::state::Mode::Providers | crate::state::Mode::OAuth
        ) {
            return KeyOutcome::Ignored;
        }
        let pin = app
            .selected_chat_source
            .as_ref()
            .and_then(crate::render::tool_policy::selected_tool_id)
            .map(str::to_string);
        let expand_all = crate::render::tool_policy::toggle_expand_all_mode(
            &mut app.full_tool_view,
            &mut app.expanded_tool_results,
            &mut app.collapsed_tool_results,
            pin.as_deref(),
        );
        app.chat_render_cache.borrow_mut().signature_hash = 0;
        show_notification(
            app,
            "Tools",
            if expand_all {
                "Expand all tool output."
            } else {
                "Smart tool output (useful open, rest collapsed)."
            },
        );
        save_preferences(app);
        return KeyOutcome::Handled;
    }

    if ctrl_letter(code, 't') {
        super::replace_modal(app, ModalKind::BackgroundCommands);
        app.bg_command_selected = 0;
        app.bg_command_scroll = 0;
        crate::background::refresh_background_commands(app);
        return KeyOutcome::Handled;
    }

    if ctrl_letter(code, 'b') {
        super::open_model_routing(app, crate::state::ModelRoutingTab::Agents);
        app.bg_models_selected = 0;
        app.bg_models_scroll = 0;
        return KeyOutcome::Handled;
    }

    if matches!(code, KeyCode::Char(',')) {
        super::open_settings(app);
        return KeyOutcome::Handled;
    }

    if matches!(code, KeyCode::Enter) {
        // Ctrl+Enter: reopen question or send prompt (chat only when not in a
        // text-entry modal that would want the binding for itself).
        if !app.pending_questions.is_empty() {
            return super::apply_ui_effect(app, UiEffect::ReplaceModal(ModalKind::Question));
        }
        if matches!(
            app.mode,
            crate::state::Mode::Normal | crate::state::Mode::Setup
        ) && (!app.input.trim().is_empty() || !app.pending_images.is_empty())
        {
            crate::chat::submit_message(app);
        }
        return KeyOutcome::Handled;
    }

    if ctrl_letter(code, 'n') {
        start_new_session(app);
        let outcome = super::apply_ui_effect(app, UiEffect::CloseAllModals);
        show_notification(app, "Layer", "New layer started.");
        return outcome;
    }

    KeyOutcome::Ignored
}

pub(super) fn route_system_global_key(
    app: &mut TuiApp,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> KeyOutcome {
    if is_permission_mode_cycle_key(code, modifiers) {
        cycle_permission_mode(app);
        return KeyOutcome::Handled;
    }

    if !has_control(modifiers) {
        return KeyOutcome::Ignored;
    }

    if is_copy_selection_key(code, modifiers) {
        // Prefer the selected chat cell (full block). Fall back to drag selection.
        if app.selected_chat_source.is_some() {
            crate::chat_blocks::copy_selected_block(app);
        } else if let Some(text) = selected_text(app) {
            copy_text_to_clipboard(app, &text);
        }
        return KeyOutcome::Handled;
    }

    // Ctrl+C quit (lowercase only — uppercase / shift is copy).
    if matches!(code, KeyCode::Char('c')) && !modifiers.contains(KeyModifiers::SHIFT) {
        return super::apply_ui_effect(app, UiEffect::Quit);
    }

    KeyOutcome::Ignored
}

pub(super) fn is_copy_selection_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    // Terminals differ: Ctrl+Shift+C may arrive as uppercase 'C' or as
    // lowercase 'c' plus an explicit SHIFT modifier. Plain Ctrl+C must quit.
    matches!(code, KeyCode::Char('C'))
        || (matches!(code, KeyCode::Char('c')) && modifiers.contains(KeyModifiers::SHIFT))
}

fn is_permission_mode_cycle_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::BackTab)
        || (matches!(code, KeyCode::Tab) && modifiers.contains(KeyModifiers::SHIFT))
}

fn cycle_permission_mode(app: &mut TuiApp) {
    let next = match current_permission_mode(app) {
        PermissionMode::Restricted => PermissionMode::AcceptEdits,
        PermissionMode::AcceptEdits => PermissionMode::Auto,
        PermissionMode::Auto => PermissionMode::Yolo,
        PermissionMode::Yolo => PermissionMode::Restricted,
    };
    set_permission_mode(app, next);
}

fn set_permission_mode(app: &mut TuiApp, mode: PermissionMode) {
    set_permission_mode_for_command(app, mode);
}

/// Public to command palette / settings (same side effects as ctrl+g / shift+tab).
pub(super) fn set_permission_mode_for_command(app: &mut TuiApp, mode: PermissionMode) {
    app.loaded_config.config.security.permission_mode = mode;
    app.loaded_config.config.tui.yolo_mode = matches!(mode, PermissionMode::Yolo);
    app.yolo_mode = matches!(mode, PermissionMode::Yolo);
    tracing::info!(
        mode = permission_mode_label(mode),
        "permission mode changed"
    );
    show_notification(
        app,
        "Permissions",
        format!("Mode: {}.", permission_mode_label(mode)),
    );
    save_preferences(app);

    // Update the live engine + active session tool policies without dropping
    // the current session (rebuild_provider would create a fresh engine).
    let engine = app.engine();
    spawn_runtime_task(async move {
        if let Err(err) = engine.set_permission_mode(mode).await {
            tracing::warn!(error = %err, "failed to apply permission mode to engine");
        }
    });
}

pub(super) fn cycle_permission_mode_for_command(app: &mut TuiApp) {
    cycle_permission_mode(app);
}

pub(crate) fn permission_mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Restricted => "restricted",
        PermissionMode::AcceptEdits => "accept-edits",
        PermissionMode::Auto => "auto",
        PermissionMode::Yolo => "yolo",
    }
}

pub(crate) fn current_permission_mode(app: &TuiApp) -> PermissionMode {
    if app.yolo_mode {
        PermissionMode::Yolo
    } else {
        app.loaded_config.config.security.permission_mode
    }
}
