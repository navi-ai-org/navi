use crate::TuiApp;
use crate::chat::start_new_session;
use crate::clipboard::try_read_clipboard_image;
use crate::mouse::{copy_text_to_clipboard, selected_text};
use crate::notifications::show_notification;
use crate::persistence::save_preferences;
use crate::state::ModalKind;
use crate::ui::KeyOutcome;
use crate::ui::UiEffect;
use crossterm::event::{KeyCode, KeyModifiers};
use navi_core::PermissionMode;

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
                let mode = if app.yolo_mode {
                    PermissionMode::Restricted
                } else {
                    PermissionMode::Yolo
                };
                set_permission_mode(app, mode);
                return KeyOutcome::Handled;
            }
            KeyCode::Char('p') => {
                let outcome =
                    super::apply_ui_effect(app, UiEffect::ReplaceModal(ModalKind::Commands));
                app.command_filter.clear();
                app.command_filter_cursor = 0;
                app.selected_command = crate::commands::first_selectable_command_row(
                    &crate::commands::command_rows(app),
                );
                app.command_scroll = 0;
                return outcome;
            }
            KeyCode::Char('q') => {
                return super::apply_ui_effect(
                    app,
                    UiEffect::ReplaceModal(ModalKind::MessageQueue),
                );
            }
            KeyCode::Char('.') => {
                crate::view::help::open_help(app);
                return KeyOutcome::Handled;
            }
            KeyCode::Char('m') => {
                super::open_model_picker(app);
                return KeyOutcome::Handled;
            }
            KeyCode::Char('s') => {
                super::open_sessions_picker(app);
                return KeyOutcome::Handled;
            }
            KeyCode::Char('i') | KeyCode::Char('v') => {
                if app.mode == crate::state::Mode::Normal && !app.is_loading {
                    match try_read_clipboard_image() {
                        Some(image) => {
                            app.pending_images.push(image);
                            let tag = format!("[Image {}]", app.pending_images.len());
                            crate::input::insert_input_text(app, &tag);
                            show_notification(app, "Image", format!("Attached as {}", tag));
                        }
                        None => {
                            show_notification(app, "Image", "No image found in clipboard.");
                        }
                    }
                }
                return KeyOutcome::Handled;
            }
            KeyCode::Char('o') | KeyCode::Char('O') => {
                // Smart toggle: expand-all ↔ smart defaults, without closing
                // the tool the user currently has selected/open.
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
            KeyCode::Char('t') => {
                super::replace_modal(app, ModalKind::BackgroundCommands);
                app.bg_command_selected = 0;
                app.bg_command_scroll = 0;
                crate::background::refresh_background_commands(app);
                return KeyOutcome::Handled;
            }
            KeyCode::Char('b') => {
                super::open_model_routing(app, crate::state::ModelRoutingTab::Agents);
                app.bg_models_selected = 0;
                app.bg_models_scroll = 0;
                return KeyOutcome::Handled;
            }
            KeyCode::Char(',') => {
                super::open_settings(app);
                return KeyOutcome::Handled;
            }
            KeyCode::Enter => {
                if !app.pending_questions.is_empty() {
                    return super::apply_ui_effect(
                        app,
                        UiEffect::ReplaceModal(ModalKind::Question),
                    );
                }
                if !app.input.trim().is_empty() || !app.pending_images.is_empty() {
                    crate::chat::submit_message(app);
                }
                return KeyOutcome::Handled;
            }
            KeyCode::Char('n') => {
                start_new_session(app);
                let outcome = super::apply_ui_effect(app, UiEffect::CloseAllModals);
                show_notification(app, "Layer", "New layer started.");
                return outcome;
            }
            _ => {}
        }
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

    if !modifiers.contains(KeyModifiers::CONTROL) {
        return KeyOutcome::Ignored;
    }

    if is_copy_selection_key(code, modifiers) {
        if let Some(text) = selected_text(app) {
            copy_text_to_clipboard(app, &text);
        }
        return KeyOutcome::Handled;
    }

    match code {
        KeyCode::Char('c') => super::apply_ui_effect(app, UiEffect::Quit),
        _ => KeyOutcome::Ignored,
    }
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
    if !app.is_loading {
        crate::providers::rebuild_provider(app);
    }
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
