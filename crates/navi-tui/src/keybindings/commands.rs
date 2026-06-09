use crate::TuiApp;
use crate::chat::{reset_system_context, retry_last_response};
use crate::commands::{CommandAction, filtered_commands};
use crate::notifications::show_notification;
use crate::render::command_scroll_offset;
use crate::state::ModalKind;
use crate::ui::list::SelectListState;
use crossterm::event::KeyCode;

pub(crate) fn handle_command_key(app: &mut TuiApp, code: KeyCode) -> bool {
    const VISIBLE_ROWS: usize = 10;
    let mut list_state = SelectListState::new(app.selected_command, app.command_scroll);
    match code {
        KeyCode::Esc => super::close_active_modal(app),
        KeyCode::Char(ch) => {
            app.command_filter.push(ch);
            list_state.reset();
        }
        KeyCode::Backspace => {
            app.command_filter.pop();
            list_state.clamp(filtered_commands(app).len());
        }
        KeyCode::Down | KeyCode::Tab => {
            list_state.select_next(filtered_commands(app).len());
        }
        KeyCode::PageDown => {
            list_state.page_next(filtered_commands(app).len(), 8);
        }
        KeyCode::Up => {
            list_state.select_previous();
        }
        KeyCode::PageUp => {
            list_state.page_previous(8);
        }
        KeyCode::Enter => return run_selected_command(app),
        _ => {}
    }
    app.selected_command = list_state.selected();
    app.command_scroll = command_scroll_offset(app.selected_command, VISIBLE_ROWS);

    false
}

pub(crate) fn run_selected_command(app: &mut TuiApp) -> bool {
    let commands = filtered_commands(app);
    let Some(command) = commands.get(app.selected_command).copied() else {
        super::close_all_modals(app);
        return false;
    };

    match command.action {
        CommandAction::NewSession => {
            app.messages.clear();
            reset_system_context(app);
            app.input.clear();
            app.input_cursor = 0;
            app.scroll_offset = 0;
            super::close_all_modals(app);
        }
        CommandAction::SwitchModel => {
            super::open_model_picker(app);
        }
        CommandAction::RetryLast => {
            retry_last_response(app);
        }
        CommandAction::OpenThinking => {
            super::open_thinking_picker(app);
        }
        CommandAction::Compact => {
            if app.is_loading {
                show_notification(app, "Compact", "Cannot compact while a request is active.");
            } else {
                show_notification(
                    app,
                    "Compact",
                    "Compaction will trigger on next request if context is full.",
                );
                app.compact_state.last_input_tokens = Some(app.compact_state.context_window);
            }
            super::close_all_modals(app);
        }
        CommandAction::Sessions => {
            super::open_sessions_picker(app);
        }
        CommandAction::SyncModels => {
            super::provider_sync::sync_models_tui(app);
            super::close_all_modals(app);
        }
        CommandAction::Providers => {
            super::open_provider_settings(app);
        }
        CommandAction::Skills => {
            super::open_skills_picker(app);
        }
        CommandAction::Plugins => {
            super::open_plugins_picker(app);
        }
        CommandAction::Quit => return true,
        CommandAction::Settings => {
            super::replace_modal(app, ModalKind::Settings);
            app.selected_setting = 0;
        }
        _ => super::close_all_modals(app),
    }

    false
}
