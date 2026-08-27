use ratatui::prelude::Span;
use ratatui::style::Style;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Split styled spans into multiple lines that each fit within `max_width` columns.
pub(crate) fn wrap_spans_to_width(
    spans: &[Span<'static>],
    max_width: usize,
) -> Vec<Vec<Span<'static>>> {
    let max_width = max_width.max(1);
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut current_width = 0usize;

    for span in spans {
        if span.content.is_empty() {
            continue;
        }
        let style = span.style;
        let mut chunk = String::new();
        for ch in span.content.chars() {
            let char_width = ch.width().unwrap_or(0);
            if current_width > 0 && current_width + char_width > max_width {
                if !chunk.is_empty() {
                    // Invariant: `lines` starts as `vec![Vec::new()]` and only grows.
                    if let Some(line) = lines.last_mut() {
                        line.push(Span::styled(std::mem::take(&mut chunk), style));
                    }
                }
                lines.push(Vec::new());
                current_width = 0;
            }
            chunk.push(ch);
            current_width += char_width;
        }
        if !chunk.is_empty() {
            // Invariant: `lines` starts as `vec![Vec::new()]` and only grows.
            if let Some(line) = lines.last_mut() {
                line.push(Span::styled(chunk, style));
            }
        }
    }

    lines
}

/// Wrap inline prose while preserving span styles and preferring word boundaries.
pub(crate) fn wrap_inline_spans_to_width(
    spans: &[Span<'static>],
    max_width: usize,
) -> Vec<Vec<Span<'static>>> {
    let max_width = max_width.max(1);
    let units = spans
        .iter()
        .flat_map(|span| span.content.chars().map(move |ch| (ch, span.style)))
        .collect::<Vec<_>>();
    let mut lines = Vec::new();
    let mut current: Vec<(char, Style)> = Vec::new();
    let mut current_width = 0usize;

    for unit in units {
        let unit_width = unit.0.width().unwrap_or(0);
        if current_width > 0 && current_width + unit_width > max_width {
            if let Some(space_index) = current.iter().rposition(|(ch, _)| ch.is_whitespace()) {
                let mut carry = current.split_off(space_index + 1);
                current.truncate(space_index);
                trim_trailing_whitespace(&mut current);
                lines.push(styled_units_to_spans(std::mem::take(&mut current)));
                trim_leading_whitespace(&mut carry);
                current_width = units_width(&carry);
                current = carry;
            } else {
                lines.push(styled_units_to_spans(std::mem::take(&mut current)));
                current_width = 0;
            }
        }

        if current_width > 0 && current_width + unit_width > max_width {
            lines.push(styled_units_to_spans(std::mem::take(&mut current)));
            current_width = 0;
        }
        if current.is_empty() && unit.0.is_whitespace() {
            continue;
        }
        current.push(unit);
        current_width += unit_width;
    }

    trim_trailing_whitespace(&mut current);
    if !current.is_empty() || lines.is_empty() {
        lines.push(styled_units_to_spans(current));
    }
    lines
}

fn units_width(units: &[(char, Style)]) -> usize {
    units.iter().map(|(ch, _)| ch.width().unwrap_or(0)).sum()
}

fn trim_leading_whitespace(units: &mut Vec<(char, Style)>) {
    let count = units
        .iter()
        .take_while(|(ch, _)| ch.is_whitespace())
        .count();
    units.drain(..count);
}

fn trim_trailing_whitespace(units: &mut Vec<(char, Style)>) {
    while units.last().is_some_and(|(ch, _)| ch.is_whitespace()) {
        units.pop();
    }
}

fn styled_units_to_spans(units: Vec<(char, Style)>) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (ch, style) in units {
        if let Some(last) = spans.last_mut()
            && last.style == style
        {
            last.content.to_mut().push(ch);
            continue;
        }
        spans.push(Span::styled(ch.to_string(), style));
    }
    spans
}

pub(crate) fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
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
            let word_width = display_width(word);
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
