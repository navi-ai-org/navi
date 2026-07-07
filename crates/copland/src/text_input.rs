use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::prelude::{Line, Span};
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub struct TextInputRef<'a> {
    text: &'a mut String,
    cursor: &'a mut usize,
}

impl<'a> TextInputRef<'a> {
    pub fn new(text: &'a mut String, cursor: &'a mut usize) -> Self {
        Self { text, cursor }
    }

    pub fn insert_char(&mut self, ch: char) {
        self.clamp_cursor();
        self.text.insert(*self.cursor, ch);
        *self.cursor += ch.len_utf8();
    }

    /// Insert a multi-character string at the cursor position, filtering out
    /// control characters (except `\n` and `\t`). `\r\n` is normalized to
    /// `\n`; standalone `\r` is also converted to `\n`.
    pub fn insert_text(&mut self, text: &str) {
        self.clamp_cursor();
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\r' => {
                    // \r\n → skip \r, the following \n will be inserted.
                    // Standalone \r → convert to \n.
                    if chars.peek() != Some(&'\n') {
                        self.text.insert(*self.cursor, '\n');
                        *self.cursor += 1;
                    }
                }
                '\n' | '\t' => {
                    self.text.insert(*self.cursor, ch);
                    *self.cursor += ch.len_utf8();
                }
                c if c.is_control() => {}
                c => {
                    self.text.insert(*self.cursor, c);
                    *self.cursor += c.len_utf8();
                }
            }
        }
    }

    pub fn delete_previous_char(&mut self) {
        self.clamp_cursor();
        let prefix = &self.text[..*self.cursor];
        if prefix.ends_with(']') {
            if let Some(start_bracket) = prefix.rfind('[') {
                let tag_candidate = &prefix[start_bracket..];
                if tag_candidate.starts_with("[Image ") {
                    let digits_part = &tag_candidate[7..tag_candidate.len() - 1];
                    if !digits_part.is_empty() && digits_part.chars().all(|c| c.is_ascii_digit()) {
                        self.text.drain(start_bracket..*self.cursor);
                        *self.cursor = start_bracket;
                        return;
                    }
                }
            }
        }
        let Some(previous) = previous_char_boundary(self.text, *self.cursor) else {
            return;
        };
        self.text.drain(previous..*self.cursor);
        *self.cursor = previous;
    }

    pub fn delete_next_char(&mut self) {
        self.clamp_cursor();
        let suffix = &self.text[*self.cursor..];
        if suffix.starts_with("[Image ") {
            if let Some(end_bracket) = suffix.find(']') {
                let tag_candidate = &suffix[..=end_bracket];
                let digits_part = &tag_candidate[7..tag_candidate.len() - 1];
                if !digits_part.is_empty() && digits_part.chars().all(|c| c.is_ascii_digit()) {
                    let tag_len = tag_candidate.len();
                    self.text.drain(*self.cursor..*self.cursor + tag_len);
                    return;
                }
            }
        }
        let Some(next) = next_char_boundary(self.text, *self.cursor) else {
            return;
        };
        self.text.drain(*self.cursor..next);
    }

    pub fn move_previous_char(&mut self) {
        self.clamp_cursor();
        if let Some(previous) = previous_char_boundary(self.text, *self.cursor) {
            *self.cursor = previous;
        }
    }

    pub fn move_next_char(&mut self) {
        self.clamp_cursor();
        if let Some(next) = next_char_boundary(self.text, *self.cursor) {
            *self.cursor = next;
        }
    }

    pub fn move_previous_hump(&mut self) {
        self.clamp_cursor();
        *self.cursor = previous_hump_boundary(self.text, *self.cursor);
    }

    pub fn move_next_hump(&mut self) {
        self.clamp_cursor();
        *self.cursor = next_hump_boundary(self.text, *self.cursor);
    }

    pub fn move_previous_control_stop(&mut self) {
        self.clamp_cursor();
        *self.cursor = previous_control_boundary(self.text, *self.cursor);
    }

    pub fn move_next_control_stop(&mut self) {
        self.clamp_cursor();
        *self.cursor = next_control_boundary(self.text, *self.cursor);
    }

    pub fn delete_next_hump(&mut self) {
        self.clamp_cursor();
        let end = next_hump_boundary(self.text, *self.cursor);
        self.text.drain(*self.cursor..end);
    }

    pub fn delete_previous_hump(&mut self) {
        self.clamp_cursor();
        let start = previous_hump_boundary(self.text, *self.cursor);
        self.text.drain(start..*self.cursor);
        *self.cursor = start;
    }

    pub fn delete_previous_space_word(&mut self) {
        self.clamp_cursor();
        let start = previous_space_word_boundary(self.text, *self.cursor);
        self.text.drain(start..*self.cursor);
        *self.cursor = start;
    }

    pub fn move_to_start(&mut self) {
        *self.cursor = 0;
    }

    pub fn move_to_end(&mut self) {
        *self.cursor = self.text.len();
    }

    pub fn delete_to_start(&mut self) {
        self.clamp_cursor();
        self.text.drain(..*self.cursor);
        *self.cursor = 0;
    }

    pub fn delete_to_end(&mut self) {
        self.clamp_cursor();
        self.text.truncate(*self.cursor);
    }

    pub fn clear(&mut self) {
        self.text.clear();
        *self.cursor = 0;
    }

    fn clamp_cursor(&mut self) {
        *self.cursor = (*self.cursor).min(self.text.len());
        *self.cursor = floor_char_boundary(self.text, *self.cursor);
    }
}

pub fn floor_char_boundary(value: &str, mut cursor: usize) -> usize {
    cursor = cursor.min(value.len());
    while !value.is_char_boundary(cursor) {
        cursor = cursor.saturating_sub(1);
    }
    cursor
}

#[derive(Clone, Copy)]
pub struct TextInputRenderSpec<'a> {
    pub value: &'a str,
    pub cursor: usize,
    pub placeholder: &'a str,
    pub prefix: &'a str,
    pub focused: bool,
    pub text_style: Style,
    pub placeholder_style: Style,
    pub prefix_style: Style,
    pub cursor_style: Style,
    pub background_style: Style,
}

pub fn render_text_input_line(frame: &mut Frame<'_>, area: Rect, spec: TextInputRenderSpec<'_>) {
    let prefix_width = UnicodeWidthStr::width(spec.prefix);
    let content_width = area.width as usize;
    let value_width = content_width.saturating_sub(prefix_width).max(1);
    let cursor = floor_char_boundary(spec.value, spec.cursor.min(spec.value.len()));
    let window = input_window(spec.value, cursor, value_width);
    let visible_value = &spec.value[window.start..window.end];
    let cursor_offset = cursor.saturating_sub(window.start);

    let mut spans = vec![Span::styled(spec.prefix.to_string(), spec.prefix_style)];
    if spec.value.is_empty() {
        let placeholder = fit_display_width(spec.placeholder, value_width);
        if spec.focused {
            if placeholder.is_empty() {
                spans.push(Span::styled(" ", spec.cursor_style));
            } else {
                let next = next_char_boundary(&placeholder, 0).unwrap_or(placeholder.len());
                let (cursor_text, after) = placeholder.split_at(next);
                spans.push(Span::styled(cursor_text.to_string(), spec.cursor_style));
                spans.push(Span::styled(after.to_string(), spec.placeholder_style));
            }
        } else {
            spans.push(Span::styled(placeholder, spec.placeholder_style));
        }
    } else {
        let (before, rest) = visible_value.split_at(cursor_offset.min(visible_value.len()));
        spans.push(Span::styled(before.to_string(), spec.text_style));
        if spec.focused {
            if rest.is_empty() {
                spans.push(Span::styled(" ", spec.cursor_style));
            } else {
                let next =
                    next_char_boundary(visible_value, cursor_offset).unwrap_or(visible_value.len());
                let (cursor_text, after) = rest.split_at(next - cursor_offset);
                spans.push(Span::styled(cursor_text.to_string(), spec.cursor_style));
                spans.push(Span::styled(after.to_string(), spec.text_style));
            }
        } else {
            spans.push(Span::styled(rest.to_string(), spec.text_style));
        }
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(spec.background_style),
        area,
    );

    if spec.focused {
        let before_cursor = if spec.value.is_empty() {
            ""
        } else {
            &spec.value[window.start..cursor]
        };
        let cursor_x = area.x.saturating_add(
            (prefix_width + UnicodeWidthStr::width(before_cursor))
                .min(area.width.saturating_sub(1) as usize) as u16,
        );
        frame.set_cursor_position(Position::new(cursor_x, area.y));
    }
}

#[derive(Clone, Copy)]
struct InputWindow {
    start: usize,
    end: usize,
}

fn input_window(value: &str, cursor: usize, width: usize) -> InputWindow {
    if value.is_empty() {
        return InputWindow { start: 0, end: 0 };
    }
    let width = width.max(1);
    let cursor = floor_char_boundary(value, cursor);
    let mut start = 0;
    while UnicodeWidthStr::width(&value[start..cursor]) >= width {
        let Some(next) = next_char_boundary(value, start) else {
            break;
        };
        start = next;
    }

    let mut end = cursor;
    while end < value.len() {
        let Some(next) = next_char_boundary(value, end) else {
            break;
        };
        if UnicodeWidthStr::width(&value[start..next]) > width {
            break;
        }
        end = next;
    }
    if end == cursor && cursor < value.len() {
        end = next_char_boundary(value, cursor).unwrap_or(value.len());
    }

    InputWindow { start, end }
}

fn fit_display_width(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_string();
    }
    let mut result = String::new();
    let mut used = 0usize;
    for ch in value.chars() {
        let char_width = ch.width().unwrap_or(0);
        if used + char_width >= width {
            break;
        }
        result.push(ch);
        used += char_width;
    }
    result.push('…');
    result
}

pub fn previous_char_boundary(value: &str, cursor: usize) -> Option<usize> {
    value[..cursor]
        .char_indices()
        .last()
        .map(|(index, _)| index)
}

pub fn next_char_boundary(value: &str, cursor: usize) -> Option<usize> {
    value[cursor..]
        .char_indices()
        .nth(1)
        .map(|(index, _)| cursor + index)
        .or_else(|| (cursor < value.len()).then_some(value.len()))
}

pub fn previous_hump_boundary(value: &str, cursor: usize) -> usize {
    let chars = indexed_chars(value);
    let mut index = char_slot_at_byte(&chars, cursor);
    if index == 0 {
        return 0;
    }

    index -= 1;
    while index > 0 && is_separator(chars[index].1) {
        index -= 1;
    }
    while index > 0 && is_hump_continuation(&chars, index) {
        index -= 1;
    }

    chars.get(index).map(|(byte, _)| *byte).unwrap_or(0)
}

pub fn next_hump_boundary(value: &str, cursor: usize) -> usize {
    let chars = indexed_chars(value);
    let mut index = char_slot_at_byte(&chars, cursor);
    if index >= chars.len() {
        return value.len();
    }

    while index < chars.len() && is_separator(chars[index].1) {
        index += 1;
    }
    if index < chars.len() {
        index += 1;
    }
    while index < chars.len() && is_hump_continuation(&chars, index) {
        index += 1;
    }

    chars
        .get(index)
        .map(|(byte, _)| *byte)
        .unwrap_or(value.len())
}

pub fn previous_control_boundary(value: &str, cursor: usize) -> usize {
    let chars = indexed_chars(value);
    let mut index = char_slot_at_byte(&chars, cursor);
    if index == 0 {
        return 0;
    }

    index -= 1;
    if is_separator(chars[index].1) {
        return chars[index].0;
    }

    while index > 0 && is_hump_continuation(&chars, index) {
        index -= 1;
    }

    chars.get(index).map(|(byte, _)| *byte).unwrap_or(0)
}

pub fn next_control_boundary(value: &str, cursor: usize) -> usize {
    let chars = indexed_chars(value);
    let mut index = char_slot_at_byte(&chars, cursor);
    if index >= chars.len() {
        return value.len();
    }

    if is_separator(chars[index].1) {
        return next_char_boundary(value, cursor).unwrap_or(value.len());
    }

    index += 1;
    while index < chars.len() && is_hump_continuation(&chars, index) {
        index += 1;
    }

    chars
        .get(index)
        .map(|(byte, _)| *byte)
        .unwrap_or(value.len())
}

pub fn previous_space_word_boundary(value: &str, cursor: usize) -> usize {
    let chars = indexed_chars(value);
    let mut index = char_slot_at_byte(&chars, cursor);
    if index == 0 {
        return 0;
    }

    index -= 1;
    while index > 0 && chars[index].1.is_whitespace() {
        index -= 1;
    }
    while index > 0 && !chars[index - 1].1.is_whitespace() {
        index -= 1;
    }

    chars.get(index).map(|(byte, _)| *byte).unwrap_or(0)
}

fn indexed_chars(value: &str) -> Vec<(usize, char)> {
    value.char_indices().collect()
}

fn char_slot_at_byte(chars: &[(usize, char)], cursor: usize) -> usize {
    chars
        .iter()
        .position(|(byte, _)| *byte >= cursor)
        .unwrap_or(chars.len())
}

fn is_hump_continuation(chars: &[(usize, char)], index: usize) -> bool {
    let previous = chars[index - 1].1;
    let current = chars[index].1;
    let next = chars.get(index + 1).map(|(_, ch)| *ch);

    if is_separator(previous) || is_separator(current) {
        return false;
    }
    if previous.is_lowercase() && current.is_uppercase() {
        return false;
    }
    if previous.is_ascii_digit() != current.is_ascii_digit()
        && (previous.is_alphanumeric() || current.is_alphanumeric())
    {
        return false;
    }
    if previous.is_uppercase()
        && current.is_uppercase()
        && next.is_some_and(|next| next.is_lowercase())
    {
        return false;
    }

    true
}

fn is_separator(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '_' | '-' | '.' | '/' | '\\' | ':' | ';' | ',' | '(' | ')' | '[' | ']' | '{' | '}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_input_ref_edits_at_camel_humps() {
        let mut text = "fooBar_baz".to_string();
        let mut cursor = text.len();
        {
            let mut input = TextInputRef::new(&mut text, &mut cursor);
            input.move_previous_hump();
        }
        assert_eq!(cursor, 7);

        let mut text = "fooBar_baz".to_string();
        let mut cursor = text.len();
        {
            let mut input = TextInputRef::new(&mut text, &mut cursor);
            input.delete_previous_hump();
        }
        assert_eq!(text, "fooBar_");

        {
            let mut input = TextInputRef::new(&mut text, &mut cursor);
            input.insert_char('x');
        }
        assert_eq!(text, "fooBar_x");
    }

    #[test]
    fn insert_text_normalizes_line_endings() {
        let mut text = String::new();
        let mut cursor = 0;
        {
            let mut input = TextInputRef::new(&mut text, &mut cursor);
            input.insert_text("hello\r\nworld\rbye\n");
        }
        assert_eq!(text, "hello\nworld\nbye\n");
        assert_eq!(cursor, text.len());
    }

    #[test]
    fn insert_text_skips_control_chars() {
        let mut text = String::new();
        let mut cursor = 0;
        {
            let mut input = TextInputRef::new(&mut text, &mut cursor);
            input.insert_text("ab\x01\x02cd");
        }
        assert_eq!(text, "abcd");
    }

    #[test]
    fn insert_text_at_cursor_position() {
        let mut text = "hello".to_string();
        let mut cursor = 5;
        {
            let mut input = TextInputRef::new(&mut text, &mut cursor);
            input.insert_text(" world");
        }
        assert_eq!(text, "hello world");
        assert_eq!(cursor, 11);
    }
}
