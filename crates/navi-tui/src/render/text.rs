use ratatui::prelude::{Modifier, Span, Style};

use crate::theme::*;

/// Split styled spans into multiple lines that each fit within `max_width` columns.
pub(crate) fn wrap_spans_to_width(spans: &[Span<'static>], max_width: usize) -> Vec<Vec<Span<'static>>> {
    let max_width = max_width.max(1);
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut current_width = 0usize;

    for span in spans {
        if span.content.is_empty() {
            continue;
        }
        let style = span.style;
        let chars: Vec<char> = span.content.chars().collect();
        let mut start = 0usize;
        while start < chars.len() {
            if current_width >= max_width {
                lines.push(Vec::new());
                current_width = 0;
            }
            let remaining = max_width.saturating_sub(current_width);
            let take = remaining.min(chars.len() - start);
            let chunk: String = chars[start..start + take].iter().collect();
            lines
                .last_mut()
                .expect("at least one output line")
                .push(Span::styled(chunk, style));
            current_width += take;
            start += take;
        }
    }

    lines
}

pub(crate) fn display_width(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.chars().count()
    }
}

pub(crate) fn project_label() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "~".to_string())
}

pub(crate) fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    let max_width = max_width.max(10);
    let mut lines = Vec::new();

    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }

        let mut current_line = String::new();
        let mut current_width: usize = 0;
        for word in paragraph.split_whitespace() {
            let word_width = word.chars().count();
            if current_line.is_empty() {
                current_line = word.to_string();
                current_width = word_width;
            } else if current_width + 1 + word_width <= max_width {
                current_line.push(' ');
                current_line.push_str(word);
                current_width += 1 + word_width;
            } else {
                lines.push(current_line);
                current_line = word.to_string();
                current_width = word_width;
            }
        }
        if !current_line.is_empty() {
            lines.push(current_line);
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

pub(crate) fn cursor_span(value: impl Into<String>) -> Span<'static> {
    Span::styled(
        value.into(),
        Style::default()
            .fg(bg())
            .bg(signal())
            .add_modifier(Modifier::BOLD),
    )
}

pub(crate) fn mask_key_segment(segment: &str) -> String {
    let chars: Vec<char> = segment.chars().collect();
    if chars.len() <= 12 {
        return segment.to_string();
    }
    let mut result = String::new();
    for (i, ch) in chars.iter().enumerate() {
        if i < 6 || i >= chars.len() - 4 {
            result.push(*ch);
        } else {
            result.push('•');
        }
    }
    result
}
