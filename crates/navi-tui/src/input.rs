use crate::app::TuiApp;
use crate::ui::text_input::TextInputRef;

pub(crate) fn chat_input_ref(app: &mut TuiApp) -> TextInputRef<'_> {
    TextInputRef::new(&mut app.input, &mut app.input_cursor)
}

pub(crate) fn api_key_input_ref(app: &mut TuiApp) -> TextInputRef<'_> {
    TextInputRef::new(&mut app.api_key_input, &mut app.api_key_cursor)
}

pub(crate) fn insert_input_char(app: &mut TuiApp, ch: char) {
    chat_input_ref(app).insert_char(ch);
}

pub(crate) fn delete_input_previous_char(app: &mut TuiApp) {
    chat_input_ref(app).delete_previous_char();
}

pub(crate) fn delete_input_next_char(app: &mut TuiApp) {
    chat_input_ref(app).delete_next_char();
}

pub(crate) fn move_input_previous_char(app: &mut TuiApp) {
    chat_input_ref(app).move_previous_char();
}

pub(crate) fn move_input_next_char(app: &mut TuiApp) {
    chat_input_ref(app).move_next_char();
}

pub(crate) fn move_input_previous_hump(app: &mut TuiApp) {
    chat_input_ref(app).move_previous_hump();
}

pub(crate) fn move_input_next_hump(app: &mut TuiApp) {
    chat_input_ref(app).move_next_hump();
}

pub(crate) fn move_input_previous_control_stop(app: &mut TuiApp) {
    chat_input_ref(app).move_previous_control_stop();
}

pub(crate) fn move_input_next_control_stop(app: &mut TuiApp) {
    chat_input_ref(app).move_next_control_stop();
}

pub(crate) fn delete_input_next_hump(app: &mut TuiApp) {
    chat_input_ref(app).delete_next_hump();
}

pub(crate) fn delete_input_previous_hump(app: &mut TuiApp) {
    chat_input_ref(app).delete_previous_hump();
}

pub(crate) fn delete_input_previous_space_word(app: &mut TuiApp) {
    chat_input_ref(app).delete_previous_space_word();
}
