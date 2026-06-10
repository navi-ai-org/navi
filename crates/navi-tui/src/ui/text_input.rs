pub(crate) struct TextInputRef<'a> {
    text: &'a mut String,
    cursor: &'a mut usize,
}

impl<'a> TextInputRef<'a> {
    pub(crate) fn new(text: &'a mut String, cursor: &'a mut usize) -> Self {
        Self { text, cursor }
    }

    pub(crate) fn insert_char(&mut self, ch: char) {
        self.clamp_cursor();
        self.text.insert(*self.cursor, ch);
        *self.cursor += ch.len_utf8();
    }

    /// Insert a multi-character string at the cursor position, filtering out
    /// control characters (except `\n` and `\t`). `\r\n` is normalized to
    /// `\n`; standalone `\r` is also converted to `\n`.
    pub(crate) fn insert_text(&mut self, text: &str) {
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

    pub(crate) fn delete_previous_char(&mut self) {
        self.clamp_cursor();
        let Some(previous) = previous_char_boundary(self.text, *self.cursor) else {
            return;
        };
        self.text.drain(previous..*self.cursor);
        *self.cursor = previous;
    }

    pub(crate) fn delete_next_char(&mut self) {
        self.clamp_cursor();
        let Some(next) = next_char_boundary(self.text, *self.cursor) else {
            return;
        };
        self.text.drain(*self.cursor..next);
    }

    pub(crate) fn move_previous_char(&mut self) {
        self.clamp_cursor();
        if let Some(previous) = previous_char_boundary(self.text, *self.cursor) {
            *self.cursor = previous;
        }
    }

    pub(crate) fn move_next_char(&mut self) {
        self.clamp_cursor();
        if let Some(next) = next_char_boundary(self.text, *self.cursor) {
            *self.cursor = next;
        }
    }

    pub(crate) fn move_previous_hump(&mut self) {
        self.clamp_cursor();
        *self.cursor = previous_hump_boundary(self.text, *self.cursor);
    }

    pub(crate) fn move_next_hump(&mut self) {
        self.clamp_cursor();
        *self.cursor = next_hump_boundary(self.text, *self.cursor);
    }

    pub(crate) fn move_previous_control_stop(&mut self) {
        self.clamp_cursor();
        *self.cursor = previous_control_boundary(self.text, *self.cursor);
    }

    pub(crate) fn move_next_control_stop(&mut self) {
        self.clamp_cursor();
        *self.cursor = next_control_boundary(self.text, *self.cursor);
    }

    pub(crate) fn delete_next_hump(&mut self) {
        self.clamp_cursor();
        let end = next_hump_boundary(self.text, *self.cursor);
        self.text.drain(*self.cursor..end);
    }

    pub(crate) fn delete_previous_hump(&mut self) {
        self.clamp_cursor();
        let start = previous_hump_boundary(self.text, *self.cursor);
        self.text.drain(start..*self.cursor);
        *self.cursor = start;
    }

    pub(crate) fn delete_previous_space_word(&mut self) {
        self.clamp_cursor();
        let start = previous_space_word_boundary(self.text, *self.cursor);
        self.text.drain(start..*self.cursor);
        *self.cursor = start;
    }

    pub(crate) fn move_to_start(&mut self) {
        *self.cursor = 0;
    }

    pub(crate) fn move_to_end(&mut self) {
        *self.cursor = self.text.len();
    }

    pub(crate) fn delete_to_start(&mut self) {
        self.clamp_cursor();
        self.text.drain(..*self.cursor);
        *self.cursor = 0;
    }

    pub(crate) fn delete_to_end(&mut self) {
        self.clamp_cursor();
        self.text.truncate(*self.cursor);
    }

    pub(crate) fn clear(&mut self) {
        self.text.clear();
        *self.cursor = 0;
    }

    fn clamp_cursor(&mut self) {
        *self.cursor = (*self.cursor).min(self.text.len());
        *self.cursor = floor_char_boundary(self.text, *self.cursor);
    }
}

pub(crate) fn floor_char_boundary(value: &str, mut cursor: usize) -> usize {
    cursor = cursor.min(value.len());
    while !value.is_char_boundary(cursor) {
        cursor = cursor.saturating_sub(1);
    }
    cursor
}

pub(crate) fn previous_char_boundary(value: &str, cursor: usize) -> Option<usize> {
    value[..cursor]
        .char_indices()
        .last()
        .map(|(index, _)| index)
}

pub(crate) fn next_char_boundary(value: &str, cursor: usize) -> Option<usize> {
    value[cursor..]
        .char_indices()
        .nth(1)
        .map(|(index, _)| cursor + index)
        .or_else(|| (cursor < value.len()).then_some(value.len()))
}

pub(crate) fn previous_hump_boundary(value: &str, cursor: usize) -> usize {
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

pub(crate) fn next_hump_boundary(value: &str, cursor: usize) -> usize {
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

pub(crate) fn previous_control_boundary(value: &str, cursor: usize) -> usize {
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

pub(crate) fn next_control_boundary(value: &str, cursor: usize) -> usize {
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

pub(crate) fn previous_space_word_boundary(value: &str, cursor: usize) -> usize {
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
