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

/// ASCII Ctrl+A..=Ctrl+Z → bytes 1..=26.
fn ctrl_byte_for(letter: char) -> Option<u8> {
    let upper = letter.to_ascii_uppercase();
    if !upper.is_ascii_uppercase() {
        return None;
    }
    Some((upper as u8).wrapping_sub(b'@'))
}

/// Match a Ctrl+letter chord across encodings terminals actually emit.
///
/// 1. `Char('x'|'X') + CONTROL` — modern / Kitty-disambiguated encoding
/// 2. ASCII control byte with CONTROL — e.g. `'\r'+CONTROL` for Ctrl+M
/// 3. Bare ASCII control byte without CONTROL — fallback when progressive
///    enhancement mid-session fails. Skip H/I/J/M so BS/Tab/LF/CR are safe.
///
/// ALT blocks all variants.
fn is_ctrl_chord(code: KeyCode, modifiers: KeyModifiers, letter: char) -> bool {
    if modifiers.contains(KeyModifiers::ALT) {
        return false;
    }
    let Some(ctrl_byte) = ctrl_byte_for(letter) else {
        return false;
    };
    let is_ctrl_char = matches!(code, KeyCode::Char(c) if c as u8 == ctrl_byte);

    if has_control(modifiers) {
        return ctrl_letter(code, letter) || is_ctrl_char;
    }

    if is_ctrl_char {
        return !matches!(letter.to_ascii_lowercase(), 'h' | 'i' | 'j' | 'm');
    }
    false
}

pub(super) fn route_global_key(
    app: &mut TuiApp,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> KeyOutcome {
    // Letter globals also match bare ASCII control bytes (see `is_ctrl_chord`).
    // Non-letter chords (Ctrl+Enter, Ctrl+., Ctrl+,) still require CONTROL.
    let control_held = has_control(modifiers);

    if control_held && is_copy_selection_key(code, modifiers) {
        if app.selected_chat_source.is_some() {
            crate::chat_blocks::copy_selected_block(app);
        } else if let Some(text) = selected_text(app) {
            copy_text_to_clipboard(app, &text);
        }
        return KeyOutcome::Handled;
    }

    // Plain Ctrl+C quits (not Ctrl+Shift+C). Bare ETX (0x03) also quits.
    if is_ctrl_chord(code, modifiers, 'c') && !modifiers.contains(KeyModifiers::SHIFT) {
        if !is_copy_selection_key(code, modifiers) {
            return super::apply_ui_effect(app, UiEffect::Quit);
        }
    }

    if is_ctrl_chord(code, modifiers, 'd') {
        if app.mode == crate::state::Mode::Providers {
            return KeyOutcome::Ignored;
        }
        let outcome = super::apply_ui_effect(app, UiEffect::ReplaceModal(ModalKind::Debug));
        tracing::info!("debug modal opened");
        return outcome;
    }

    if is_ctrl_chord(code, modifiers, 'g') {
        let mode = if app.yolo_mode {
            PermissionMode::Restricted
        } else {
            PermissionMode::Yolo
        };
        set_permission_mode(app, mode);
        return KeyOutcome::Handled;
    }

    if is_ctrl_chord(code, modifiers, 'p') {
        super::commands::open_command_palette(app);
        return KeyOutcome::Handled;
    }

    if is_ctrl_chord(code, modifiers, 'q') {
        return super::apply_ui_effect(app, UiEffect::ReplaceModal(ModalKind::MessageQueue));
    }

    // Ctrl+. needs Kitty (or tmux extended-keys) on many terminals. Ctrl+X is
    // a classic control character (0x18) that always works as a help fallback
    // — same maturity pattern as Grok Build.
    if (control_held && matches!(code, KeyCode::Char('.'))) || is_ctrl_chord(code, modifiers, 'x') {
        crate::view::help::open_help(app);
        return KeyOutcome::Handled;
    }

    // Ctrl+M — model picker.
    // Bare CR without CONTROL is intentionally NOT matched.
    if is_ctrl_chord(code, modifiers, 'm') {
        super::open_model_picker(app);
        return KeyOutcome::Handled;
    }

    if is_ctrl_chord(code, modifiers, 's') {
        super::open_sessions_picker(app);
        return KeyOutcome::Handled;
    }

    if is_ctrl_chord(code, modifiers, 'i') || is_ctrl_chord(code, modifiers, 'v') {
        if app.mode != crate::state::Mode::Normal {
            return KeyOutcome::Ignored;
        }
        let want_image_only = is_ctrl_chord(code, modifiers, 'i');
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
                if let Some(text) = crate::clipboard::try_read_clipboard_text() {
                    if !text.is_empty() {
                        crate::input::insert_input_text(app, &text);
                    }
                }
            }
        }
        return KeyOutcome::Handled;
    }

    if is_ctrl_chord(code, modifiers, 'o') {
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

    if is_ctrl_chord(code, modifiers, 't') {
        super::replace_modal(app, ModalKind::BackgroundCommands);
        app.bg_command_selected = 0;
        app.bg_command_scroll = 0;
        crate::background::refresh_background_commands(app);
        return KeyOutcome::Handled;
    }

    if is_ctrl_chord(code, modifiers, 'b') {
        super::open_model_routing(app, crate::state::ModelRoutingTab::Agents);
        app.bg_models_selected = 0;
        app.bg_models_scroll = 0;
        return KeyOutcome::Handled;
    }

    if control_held && matches!(code, KeyCode::Char(',')) {
        super::open_settings(app);
        return KeyOutcome::Handled;
    }

    if control_held && matches!(code, KeyCode::Enter) {
        if !matches!(
            app.mode,
            crate::state::Mode::Normal | crate::state::Mode::Setup
        ) {
            return KeyOutcome::Ignored;
        }
        if !app.pending_questions.is_empty() {
            return super::apply_ui_effect(app, UiEffect::ReplaceModal(ModalKind::Question));
        }
        if !app.input.trim().is_empty() || !app.pending_images.is_empty() {
            crate::chat::submit_message(app);
            return KeyOutcome::Handled;
        }
        super::open_model_picker(app);
        return KeyOutcome::Handled;
    }

    if is_ctrl_chord(code, modifiers, 'n') {
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
