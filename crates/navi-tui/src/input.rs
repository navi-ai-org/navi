use crate::app::TuiApp;
use crate::ui::text_input::{TextInputRef, floor_char_boundary};

const PROMPT_WIDTH: usize = 2;
pub(crate) const COMPOSER_MAX_VISIBLE_LINES: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
struct VisualInputLine {
    start_byte: usize,
    end_byte: usize,
    cells: Vec<(usize, usize)>,
}

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

pub(crate) fn move_input_visual_line(app: &mut TuiApp, delta: isize) -> bool {
    let lines = visual_input_lines(&app.input, app.input_wrap_width);
    if lines.len() < 2 {
        return false;
    }

    let cursor = floor_char_boundary(&app.input, app.input_cursor);
    let (line_index, column) = visual_cursor_position(&lines, cursor);
    let target_index = if delta.is_negative() {
        line_index.checked_sub(1)
    } else {
        let next = line_index.saturating_add(1);
        (next < lines.len()).then_some(next)
    };
    let Some(target_index) = target_index else {
        return false;
    };

    app.input_cursor = byte_at_visual_column(&lines[target_index], column);
    true
}

pub(crate) fn input_visual_line_count(input: &str, width: usize) -> usize {
    visual_input_lines(input, width).len()
}

fn visual_input_lines(input: &str, width: usize) -> Vec<VisualInputLine> {
    let content_width = width.max(PROMPT_WIDTH + 1) - PROMPT_WIDTH;
    let mut lines = Vec::new();
    let mut start_byte = 0usize;
    let mut column = 0usize;
    let mut cells = Vec::new();

    for (byte, ch) in input.char_indices() {
        if ch == '\n' {
            lines.push(VisualInputLine {
                start_byte,
                end_byte: byte,
                cells: std::mem::take(&mut cells),
            });
            start_byte = byte + ch.len_utf8();
            column = 0;
            continue;
        }

        if column >= content_width {
            lines.push(VisualInputLine {
                start_byte,
                end_byte: byte,
                cells: std::mem::take(&mut cells),
            });
            start_byte = byte;
            column = 0;
        }

        cells.push((byte, column));
        column += 1;
    }

    lines.push(VisualInputLine {
        start_byte,
        end_byte: input.len(),
        cells,
    });
    lines
}

fn visual_cursor_position(lines: &[VisualInputLine], cursor: usize) -> (usize, usize) {
    let mut selected = 0usize;
    for (index, line) in lines.iter().enumerate() {
        if cursor >= line.start_byte && cursor <= line.end_byte {
            selected = index;
        }
    }

    let line = &lines[selected];
    let column = line
        .cells
        .iter()
        .find_map(|(byte, column)| (*byte == cursor).then_some(*column))
        .unwrap_or(line.cells.len());
    (selected, column)
}

fn byte_at_visual_column(line: &VisualInputLine, target_column: usize) -> usize {
    line.cells
        .iter()
        .find_map(|(byte, column)| (*column >= target_column).then_some(*byte))
        .unwrap_or(line.end_byte)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_line_count_wraps_and_preserves_trailing_newline() {
        assert_eq!(input_visual_line_count("", 12), 1);
        assert_eq!(input_visual_line_count("abcdefghij", 12), 1);
        assert_eq!(input_visual_line_count("abcdefghijk", 12), 2);
        assert_eq!(input_visual_line_count("abc\n", 12), 2);
    }

    #[test]
    fn visual_line_movement_moves_cursor_across_wrapped_lines() {
        let mut app = crate::tests::test_app("abcdefghijk");
        app.input_wrap_width = 12;
        app.input_cursor = app.input.len();

        assert!(move_input_visual_line(&mut app, -1));
        assert_eq!(app.input_cursor, 1);

        assert!(move_input_visual_line(&mut app, 1));
        assert_eq!(app.input_cursor, app.input.len());
    }
}
