use crate::app::TuiApp;
use crate::ui::text_input::{TextInputRef, floor_char_boundary};

const PROMPT_WIDTH: usize = 0;
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
    replace_input_selection(app);
    chat_input_ref(app).insert_char(ch);
}

pub(crate) fn insert_input_text(app: &mut TuiApp, text: &str) {
    replace_input_selection(app);
    chat_input_ref(app).insert_text(text);
}

pub(crate) fn insert_api_key_text(app: &mut TuiApp, text: &str) {
    api_key_input_ref(app).insert_text(text);
}

pub(crate) fn delete_input_previous_char(app: &mut TuiApp) {
    if replace_input_selection(app) {
        return;
    }
    chat_input_ref(app).delete_previous_char();
}

pub(crate) fn delete_input_next_char(app: &mut TuiApp) {
    if replace_input_selection(app) {
        return;
    }
    chat_input_ref(app).delete_next_char();
}

pub(crate) fn move_input_previous_char(app: &mut TuiApp) {
    app.input_selection = None;
    chat_input_ref(app).move_previous_char();
}

pub(crate) fn move_input_next_char(app: &mut TuiApp) {
    app.input_selection = None;
    chat_input_ref(app).move_next_char();
}

pub(crate) fn select_all_input(app: &mut TuiApp) {
    if app.input.is_empty() {
        app.input_selection = None;
        app.input_cursor = 0;
        return;
    }
    app.input_selection = Some((0, app.input.len()));
    app.input_cursor = app.input.len();
}

pub(crate) fn selected_input_range(app: &TuiApp) -> Option<(usize, usize)> {
    let (start, end) = app.input_selection?;
    let start = floor_char_boundary(&app.input, start.min(app.input.len()));
    let end = floor_char_boundary(&app.input, end.min(app.input.len()));
    (start != end).then_some((start.min(end), start.max(end)))
}

fn replace_input_selection(app: &mut TuiApp) -> bool {
    let Some((start, end)) = selected_input_range(app) else {
        app.input_selection = None;
        return false;
    };
    app.input.drain(start..end);
    app.input_cursor = start;
    app.input_selection = None;
    true
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

pub(crate) fn input_visual_line_ranges(input: &str, width: usize) -> Vec<(usize, usize)> {
    visual_input_lines(input, width)
        .into_iter()
        .map(|line| (line.start_byte, line.end_byte))
        .collect()
}

fn visual_input_lines(input: &str, width: usize) -> Vec<VisualInputLine> {
    let content_width = width.max(PROMPT_WIDTH + 1) - PROMPT_WIDTH;
    let mut lines = Vec::new();

    for (start, end) in soft_wrap_ranges(input, content_width) {
        let cells = input[start..end]
            .char_indices()
            .enumerate()
            .map(|(column, (offset, _))| (start + offset, column))
            .collect();
        lines.push(VisualInputLine {
            start_byte: start,
            end_byte: end,
            cells,
        });
    }
    lines
}

fn soft_wrap_ranges(input: &str, width: usize) -> Vec<(usize, usize)> {
    if input.is_empty() {
        return vec![(0, 0)];
    }
    let width = width.max(1);
    let mut ranges = Vec::new();
    let mut logical_start = 0usize;

    for (byte, ch) in input.char_indices() {
        if ch == '\n' {
            push_wrapped_logical_line(input, logical_start, byte, width, &mut ranges);
            logical_start = byte + ch.len_utf8();
        }
    }
    push_wrapped_logical_line(input, logical_start, input.len(), width, &mut ranges);
    ranges
}

fn push_wrapped_logical_line(
    input: &str,
    mut start: usize,
    end: usize,
    width: usize,
    ranges: &mut Vec<(usize, usize)>,
) {
    if start == end {
        ranges.push((start, end));
        return;
    }

    while start < end {
        let mut count = 0usize;
        let mut hard_end = end;
        let mut last_whitespace = None;
        for (offset, ch) in input[start..end].char_indices() {
            if count == width {
                hard_end = start + offset;
                break;
            }
            if ch.is_whitespace() {
                last_whitespace = Some(start + offset);
            }
            count += 1;
        }

        if hard_end == end {
            ranges.push((start, end));
            break;
        }

        let line_end = last_whitespace
            .filter(|whitespace| *whitespace > start)
            .unwrap_or(hard_end);
        ranges.push((start, line_end));
        start = line_end;

        if start == hard_end {
            continue;
        }
    }
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
    app.input_selection = None;
    chat_input_ref(app).move_previous_hump();
}

pub(crate) fn move_input_next_hump(app: &mut TuiApp) {
    app.input_selection = None;
    chat_input_ref(app).move_next_hump();
}

pub(crate) fn move_input_previous_control_stop(app: &mut TuiApp) {
    app.input_selection = None;
    chat_input_ref(app).move_previous_control_stop();
}

pub(crate) fn move_input_next_control_stop(app: &mut TuiApp) {
    app.input_selection = None;
    chat_input_ref(app).move_next_control_stop();
}

pub(crate) fn delete_input_next_hump(app: &mut TuiApp) {
    if replace_input_selection(app) {
        return;
    }
    chat_input_ref(app).delete_next_hump();
}

pub(crate) fn delete_input_previous_hump(app: &mut TuiApp) {
    if replace_input_selection(app) {
        return;
    }
    chat_input_ref(app).delete_previous_hump();
}

pub(crate) fn delete_input_previous_space_word(app: &mut TuiApp) {
    if replace_input_selection(app) {
        return;
    }
    chat_input_ref(app).delete_previous_space_word();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_line_count_wraps_and_preserves_trailing_newline() {
        assert_eq!(input_visual_line_count("", 12), 1);
        assert_eq!(input_visual_line_count("abcdefghij", 12), 1);
        assert_eq!(input_visual_line_count("abcdefghijklm", 12), 2);
        assert_eq!(input_visual_line_count("hello world again", 12), 2);
        assert_eq!(input_visual_line_count("abc\n", 12), 2);
    }

    #[test]
    fn visual_line_movement_moves_cursor_across_wrapped_lines() {
        let mut app = crate::tests::test_app("abcdefghijklm");
        app.input_wrap_width = 10;
        app.input_cursor = app.input.len();

        assert!(move_input_visual_line(&mut app, -1));
        assert_eq!(app.input_cursor, 3);

        assert!(move_input_visual_line(&mut app, 1));
        assert_eq!(app.input_cursor, app.input.len());
    }

    #[test]
    fn visual_line_wrapping_prefers_word_boundaries() {
        let ranges = input_visual_line_ranges("hello world again", 12);
        let text = ranges
            .into_iter()
            .map(|(start, end)| &"hello world again"[start..end])
            .collect::<Vec<_>>();

        assert_eq!(text, vec!["hello world", " again"]);
    }
}
