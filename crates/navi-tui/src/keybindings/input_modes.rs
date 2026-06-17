use crate::TuiApp;
use crate::chat::submit_message;
use crate::input::{
    chat_input_ref, delete_input_next_char, delete_input_next_hump, delete_input_previous_char,
    delete_input_previous_hump, delete_input_previous_space_word, insert_input_char,
    move_input_next_char, move_input_next_control_stop, move_input_next_hump,
    move_input_previous_char, move_input_previous_control_stop, move_input_previous_hump,
    move_input_visual_line, select_all_input,
};
use crate::state::ModalKind;
use crossterm::event::{KeyCode, KeyModifiers};

pub(crate) fn handle_normal_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Left | KeyCode::Char('b') => move_input_previous_control_stop(app),
            KeyCode::Right | KeyCode::Char('f') => move_input_next_control_stop(app),
            KeyCode::Backspace
            | KeyCode::Char('h')
            | KeyCode::Char('w')
            | KeyCode::Char('\u{7f}') => delete_input_previous_hump(app),
            KeyCode::Delete => delete_input_next_hump(app),
            KeyCode::Char('a') => select_all_input(app),
            KeyCode::Char('e') => {
                app.input_cursor = app.input.len();
                app.input_selection = None;
            }
            KeyCode::Char('j') | KeyCode::Char('\n') | KeyCode::Char('\r') => {
                insert_input_char(app, '\n')
            }
            KeyCode::Char('u') => {
                app.input.drain(..app.input_cursor);
                app.input_cursor = 0;
                app.input_selection = None;
            }
            KeyCode::Char('k') => {
                chat_input_ref(app).delete_to_end();
                app.input_selection = None;
            }
            _ => return false,
        }
        return false;
    }

    if modifiers.contains(KeyModifiers::ALT) {
        match code {
            KeyCode::Left | KeyCode::Char('b') | KeyCode::Char(',') => {
                move_input_previous_hump(app)
            }
            KeyCode::Right | KeyCode::Char('f') | KeyCode::Char('.') => move_input_next_hump(app),
            KeyCode::Backspace | KeyCode::Char('h') | KeyCode::Char('\u{7f}') => {
                delete_input_previous_space_word(app)
            }
            KeyCode::Delete | KeyCode::Char('d') => delete_input_next_hump(app),
            _ => return false,
        }
        return false;
    }

    match code {
        KeyCode::Char('/') if app.input.is_empty() => {
            super::replace_modal(app, ModalKind::Commands);
            app.command_filter.clear();
            app.selected_command = 0;
            app.command_scroll = 0;
        }
        KeyCode::Char('?') if app.input.is_empty() => {
            super::replace_modal(app, ModalKind::Help);
        }
        KeyCode::Char(ch) => insert_input_char(app, ch),
        KeyCode::Backspace => {
            if app.input.is_empty() && !app.pending_images.is_empty() {
                app.pending_images.pop();
            } else {
                delete_input_previous_char(app);
            }
        }
        KeyCode::Delete => {
            delete_input_next_char(app);
        }
        KeyCode::Left => {
            move_input_previous_char(app);
        }
        KeyCode::Right => {
            move_input_next_char(app);
        }
        KeyCode::Home => {
            app.input_cursor = 0;
            app.input_selection = None;
        }
        KeyCode::End => {
            app.input_cursor = app.input.len();
            app.input_selection = None;
        }
        KeyCode::Up if !move_input_visual_line(app, -1) => {
            app.scroll_offset = app.scroll_offset.saturating_add(3);
        }
        KeyCode::Down if !move_input_visual_line(app, 1) => {
            app.scroll_offset = app.scroll_offset.saturating_sub(3);
        }
        KeyCode::PageUp => {
            app.scroll_offset = app.scroll_offset.saturating_add(15);
        }
        KeyCode::PageDown => {
            app.scroll_offset = app.scroll_offset.saturating_sub(15);
        }
        KeyCode::Enter if modifiers.contains(KeyModifiers::SHIFT) => {
            insert_input_char(app, '\n');
        }
        KeyCode::Enter => {
            if !app.pending_questions.is_empty() {
                super::replace_modal(app, ModalKind::Question);
            } else if (!app.input.trim().is_empty() || !app.pending_images.is_empty())
                && !app.is_loading
            {
                submit_message(app);
            }
        }
        KeyCode::Esc => {
            app.scroll_offset = 0;
        }
        _ => {}
    }

    false
}
