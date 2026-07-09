use crate::app::TuiApp;
use crate::ui::{TextInputRef, floor_char_boundary};
use crossterm::event::{KeyCode, KeyModifiers};

const PROMPT_WIDTH: usize = 0;
/// Max draft lines when the composer is focused .
/// Beyond this, content scrolls inside the box.
pub(crate) const COMPOSER_MAX_VISIBLE_LINES: usize = 12;
/// Collapsed height in content lines when scrollback is focused.
pub(crate) const COMPOSER_COLLAPSED_LINES: usize = 1;

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

pub(crate) fn command_filter_ref(app: &mut TuiApp) -> TextInputRef<'_> {
    TextInputRef::new(&mut app.command_filter, &mut app.command_filter_cursor)
}

pub(crate) fn model_filter_ref(app: &mut TuiApp) -> TextInputRef<'_> {
    TextInputRef::new(&mut app.model_filter, &mut app.model_filter_cursor)
}

pub(crate) fn provider_filter_ref(app: &mut TuiApp) -> TextInputRef<'_> {
    TextInputRef::new(&mut app.provider_filter, &mut app.provider_filter_cursor)
}

pub(crate) fn session_filter_ref(app: &mut TuiApp) -> TextInputRef<'_> {
    TextInputRef::new(&mut app.session_filter, &mut app.session_filter_cursor)
}

pub(crate) fn skill_filter_ref(app: &mut TuiApp) -> TextInputRef<'_> {
    TextInputRef::new(&mut app.skill_filter, &mut app.skill_filter_cursor)
}

pub(crate) fn theme_filter_ref(app: &mut TuiApp) -> TextInputRef<'_> {
    TextInputRef::new(&mut app.theme_filter, &mut app.theme_filter_cursor)
}

pub(crate) fn queued_edit_input_ref(app: &mut TuiApp) -> TextInputRef<'_> {
    TextInputRef::new(&mut app.queued_edit_text, &mut app.queued_edit_cursor)
}

pub(crate) fn handle_text_input_key(
    mut input: TextInputRef<'_>,
    code: KeyCode,
    modifiers: KeyModifiers,
    multiline: bool,
) -> bool {
    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('a') => input.move_to_start(),
            KeyCode::Char('e') => input.move_to_end(),
            KeyCode::Char('u') => input.delete_to_start(),
            KeyCode::Char('k') => input.delete_to_end(),
            KeyCode::Left => input.move_previous_hump(),
            KeyCode::Right => input.move_next_hump(),
            KeyCode::Backspace => input.delete_previous_hump(),
            KeyCode::Delete => input.delete_next_hump(),
            _ => return false,
        }
        return true;
    }

    if modifiers.contains(KeyModifiers::ALT) {
        match code {
            KeyCode::Backspace => input.delete_previous_space_word(),
            KeyCode::Left => input.move_previous_control_stop(),
            KeyCode::Right => input.move_next_control_stop(),
            _ => return false,
        }
        return true;
    }

    match code {
        KeyCode::Char(ch) => input.insert_char(ch),
        KeyCode::Backspace => input.delete_previous_char(),
        KeyCode::Delete => input.delete_next_char(),
        KeyCode::Left => input.move_previous_char(),
        KeyCode::Right => input.move_next_char(),
        KeyCode::Home => input.move_to_start(),
        KeyCode::End => input.move_to_end(),
        KeyCode::Enter if multiline => input.insert_char('\n'),
        _ => return false,
    }
    true
}

pub(crate) fn insert_input_char(app: &mut TuiApp, ch: char) {
    if ch.is_control() && !matches!(ch, '\n' | '\t') {
        return;
    }
    replace_input_selection(app);
    chat_input_ref(app).insert_char(ch);
}

pub(crate) fn insert_input_text(app: &mut TuiApp, text: &str) {
    let sanitized = strip_terminal_control_sequences(text);
    if sanitized.is_empty() {
        return;
    }
    replace_input_selection(app);
    chat_input_ref(app).insert_text(&sanitized);
}

pub(crate) fn insert_api_key_text(app: &mut TuiApp, text: &str) {
    let sanitized = strip_terminal_control_sequences(text);
    if !sanitized.is_empty() {
        api_key_input_ref(app).insert_text(&sanitized);
    }
}

pub(crate) fn insert_queued_edit_text(app: &mut TuiApp, text: &str) {
    let sanitized = strip_terminal_control_sequences(text);
    if !sanitized.is_empty() {
        queued_edit_input_ref(app).insert_text(&sanitized);
    }
}

fn strip_terminal_control_sequences(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek() {
                Some(&'[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(&']') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if next == '\u{07}' {
                            break;
                        }
                        if next == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(&'O') => {
                    chars.next();
                    chars.next();
                }
                Some(&'P') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if next == '\u{07}' {
                            break;
                        }
                        if next == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            }
            continue;
        }

        if ch.is_control() && !matches!(ch, '\n' | '\r' | '\t') {
            continue;
        }

        cleaned.push(ch);
    }

    cleaned
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
    fn insert_input_text_drops_csi_u_keyboard_sequences() {
        let mut app = crate::tests::test_app("");

        insert_input_text(&mut app, "ok \u{1b}[99;133u\u{1b}[127;133u text");

        assert_eq!(app.input, "ok  text");
    }

    #[test]
    fn insert_input_text_drops_sgr_mouse_sequences() {
        let mut app = crate::tests::test_app("");

        insert_input_text(&mut app, "ok \u{1b}[<0;41;12M\u{1b}[<0;42;12m text");

        assert_eq!(app.input, "ok  text");
    }

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
