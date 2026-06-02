use ratatui::prelude::{Line, Modifier, Span, Style};
use ratatui::style::Color;

use navi_sdk::{ToolInvocation, ToolResult};

use crate::state::{ChatMessage, ChatRole};
use crate::theme::*;

use super::syntax::highlight_code_line;
use super::text::wrap_text;
use super::tool::{tool_compact_text, tool_full_content};

pub(crate) fn build_chat_lines_for_messages<'a>(
    messages: impl IntoIterator<Item = &'a ChatMessage>,
    chat_width: usize,
    full_tool_view: bool,
    show_thinking: bool,
) -> Vec<Line<'static>> {
    let mut rendered_lines: Vec<Line<'static>> = Vec::new();

    for msg in messages {
        if is_empty_tool_placeholder(msg) {
            continue;
        }
        if !rendered_lines.is_empty() {
            rendered_lines.push(Line::from(""));
        }

        match msg.role {
            ChatRole::User => {
                rendered_lines.extend(render_markdown_lines(
                    &msg.content,
                    chat_width.saturating_sub(4),
                    user_accent(),
                    text(),
                    false,
                ));
            }
            ChatRole::Assistant => {
                if msg.is_compact_summary {
                    rendered_lines.push(Line::from(vec![
                        Span::styled(
                            " ◈ compacted ",
                            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            "─".repeat(chat_width.saturating_sub(14)),
                            Style::default().fg(ghost()),
                        ),
                    ]));
                }
                if let Some((invocation, result)) = tool_result_parts(msg) {
                    if full_tool_view {
                        rendered_lines.extend(render_markdown_lines(
                            &tool_full_content(invocation, result),
                            chat_width.saturating_sub(2),
                            text(),
                            text(),
                            false,
                        ));
                    } else {
                        rendered_lines.push(render_compact_tool_line(invocation, result));
                    }
                } else {
                    if show_thinking && !msg.thinking_content.is_empty() {
                        rendered_lines.extend(render_markdown_lines(
                            &msg.thinking_content,
                            chat_width.saturating_sub(4),
                            muted(),
                            muted(),
                            true,
                        ));
                        if !msg.content.is_empty() {
                            rendered_lines.push(Line::from(""));
                        }
                    }
                    rendered_lines.extend(render_markdown_lines(
                        &msg.content,
                        chat_width.saturating_sub(2),
                        text(),
                        text(),
                        false,
                    ));
                }

                if let (Some(model_label), Some(provider_label)) =
                    (&msg.model_label, &msg.provider_label)
                {
                    let elapsed = msg
                        .elapsed_ms
                        .map(|ms| {
                            if ms < 1000 {
                                format!("{ms}ms")
                            } else {
                                format!("{:.1}s", ms as f64 / 1000.0)
                            }
                        })
                        .unwrap_or_default();

                    let status = msg
                        .status
                        .as_ref()
                        .map(|status| format!(" • {status}"))
                        .unwrap_or_default();
                    let usage = msg
                        .usage_label
                        .as_ref()
                        .map(|usage| format!(" • {usage}"))
                        .unwrap_or_default();
                    let attr_text =
                        format!("◇ {model_label} via {provider_label} {elapsed}{status}{usage}");
                    let attr_len = attr_text.chars().count();
                    let dash_count = chat_width.saturating_sub(attr_len + 2);
                    let dashes: String = std::iter::repeat_n('─', dash_count).collect();

                    rendered_lines.push(Line::from(vec![
                        Span::styled(format!(" {attr_text} "), Style::default().fg(muted())),
                        Span::styled(dashes, Style::default().fg(ghost())),
                    ]));
                }
            }
        }
    }
    rendered_lines
}

pub(crate) fn is_empty_tool_placeholder(message: &ChatMessage) -> bool {
    message.role == ChatRole::Assistant
        && message.content.trim().is_empty()
        && message.thinking_content.trim().is_empty()
        && message.status.as_deref().is_some_and(|status| {
            status.starts_with("tool:")
                || status.starts_with("approval:")
                || status == "thinking"
                || status == "receiving"
        })
}

fn tool_result_parts(message: &ChatMessage) -> Option<(&ToolInvocation, &ToolResult)> {
    match (&message.tool_invocation, &message.tool_result) {
        (Some(invocation), Some(result)) => Some((invocation, result)),
        _ => None,
    }
}

fn render_compact_tool_line(invocation: &ToolInvocation, result: &ToolResult) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "● ",
            Style::default().fg(if result.ok { Color::Green } else { Color::Red }),
        ),
        Span::styled(
            tool_compact_text(invocation, result),
            Style::default().fg(text()),
        ),
    ])
}

pub(crate) fn render_markdown_lines(
    text: &str,
    max_width: usize,
    marker_color: Color,
    text_color: Color,
    italic: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code = false;
    let mut language = String::new();
    let show_marker = marker_color != text_color || italic;

    let raw_lines = text.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < raw_lines.len() {
        let raw_line = raw_lines[index];
        let trimmed = raw_line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            in_code = !in_code;
            language = if in_code {
                rest.split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string()
            } else {
                String::new()
            };
            lines.push(markdown_boundary_line(
                if in_code { rest.trim() } else { "" },
                show_marker,
                marker_color,
            ));
            index += 1;
            continue;
        }

        if in_code {
            lines.push(code_line(raw_line, &language, show_marker, marker_color));
            index += 1;
            continue;
        }

        if is_table_line(trimmed) {
            let mut table_rows = Vec::new();
            while index < raw_lines.len() && is_table_line(raw_lines[index].trim_start()) {
                let table_line = raw_lines[index].trim_start();
                if !is_table_separator(table_line) {
                    table_rows.push(table_line.to_string());
                }
                index += 1;
            }
            lines.extend(table_block_lines(
                &table_rows,
                show_marker,
                marker_color,
                max_width,
            ));
            continue;
        }

        let wrapped = wrap_text(raw_line, max_width);
        for line in wrapped {
            lines.push(text_line(
                line,
                show_marker,
                marker_color,
                text_color,
                italic,
            ));
        }
        index += 1;
    }

    if text.is_empty() {
        lines.push(text_line(
            String::new(),
            show_marker,
            marker_color,
            text_color,
            italic,
        ));
    }

    lines
}

fn text_line(
    text: String,
    show_marker: bool,
    marker_color: Color,
    text_color: Color,
    italic: bool,
) -> Line<'static> {
    let mut spans = marker_spans(show_marker, marker_color);
    if !italic && let Some(markdown_line) = markdown_prose_line(&text, text_color) {
        spans.extend(markdown_line);
        return Line::from(spans);
    }

    let mut style = Style::default().fg(text_color);
    if italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    spans.push(Span::styled(text, style));
    Line::from(spans)
}

fn markdown_prose_line(text: &str, fallback: Color) -> Option<Vec<Span<'static>>> {
    let trimmed = text.trim_start();
    let indent = text.len().saturating_sub(trimmed.len());
    let mut spans = Vec::new();
    if indent > 0 {
        spans.push(Span::styled(
            " ".repeat(indent),
            Style::default().fg(fallback),
        ));
    }

    let heading = trimmed.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&heading) && trimmed.chars().nth(heading) == Some(' ') {
        let prefix = match heading {
            1 => "█ ",
            2 => "▣ ",
            3 => "◆ ",
            _ => "◇ ",
        };
        spans.push(Span::styled(
            prefix,
            Style::default().fg(pink()).add_modifier(Modifier::BOLD),
        ));
        spans.extend(
            inline_text_spans(&trimmed[heading + 1..], crate::theme::text())
                .into_iter()
                .map(|mut span| {
                    span.style = span.style.add_modifier(Modifier::BOLD);
                    span
                }),
        );
        return Some(spans);
    }

    if let Some(rest) = trimmed.strip_prefix("> ") {
        spans.push(Span::styled(
            "▌ ",
            Style::default().fg(pink()).add_modifier(Modifier::BOLD),
        ));
        spans.extend(inline_text_spans(rest, muted()));
        return Some(spans);
    }

    if trimmed.starts_with('|') && trimmed.ends_with('|') {
        spans.extend(table_row_spans(&table_cells(trimmed), &[]));
        return Some(spans);
    }

    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        spans.push(Span::styled(
            "• ",
            Style::default().fg(pink()).add_modifier(Modifier::BOLD),
        ));
        spans.extend(inline_text_spans(&trimmed[2..], fallback));
        return Some(spans);
    }

    if let Some((marker, rest)) = ordered_list_marker(trimmed) {
        spans.push(Span::styled(
            marker,
            Style::default().fg(pink()).add_modifier(Modifier::BOLD),
        ));
        spans.extend(inline_text_spans(rest, fallback));
        return Some(spans);
    }

    let inline = inline_text_spans(trimmed, fallback);
    (inline.len() > 1).then(|| {
        spans.extend(inline);
        spans
    })
}

fn is_table_line(text: &str) -> bool {
    text.starts_with('|') && text.ends_with('|') && text.matches('|').count() >= 2
}

fn is_table_separator(text: &str) -> bool {
    is_table_line(text)
        && table_cells(text).iter().all(|cell| {
            let cell = cell.trim();
            !cell.is_empty() && cell.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
        })
}

fn table_cells(text: &str) -> Vec<String> {
    text.trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn table_block_lines(
    table_rows: &[String],
    show_marker: bool,
    marker_color: Color,
    max_width: usize,
) -> Vec<Line<'static>> {
    let rows = table_rows
        .iter()
        .map(|row| table_cells(row))
        .collect::<Vec<_>>();
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0; column_count];
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(rendered_inline_width(cell));
        }
    }

    let marker_width = if show_marker { 2 } else { 0 };
    let table_width =
        marker_width + widths.iter().sum::<usize>() + widths.len().saturating_sub(1) * 2;
    if table_width > max_width && rows.len() > 1 {
        return stacked_table_lines(&rows, show_marker, marker_color, max_width);
    }

    rows.iter()
        .enumerate()
        .map(|(row_index, cells)| {
            let mut spans = marker_spans(show_marker, marker_color);
            spans.extend(table_row_spans_with_header(cells, &widths, row_index == 0));
            Line::from(spans)
        })
        .collect()
}

fn stacked_table_lines(
    rows: &[Vec<String>],
    show_marker: bool,
    marker_color: Color,
    max_width: usize,
) -> Vec<Line<'static>> {
    let Some(headers) = rows.first() else {
        return Vec::new();
    };
    let marker_width = if show_marker { 2 } else { 0 };
    let content_width = max_width.saturating_sub(marker_width).max(16);
    let label_width = headers
        .iter()
        .map(|header| rendered_inline_width(header))
        .max()
        .unwrap_or(0)
        .min(content_width.saturating_sub(4));
    let value_width = content_width
        .saturating_sub(label_width)
        .saturating_sub(2)
        .max(8);

    let mut lines = Vec::new();
    for (row_index, row) in rows.iter().enumerate().skip(1) {
        if row_index > 1 {
            lines.push(Line::from(marker_spans(show_marker, marker_color)));
        }

        for (cell_index, header) in headers.iter().enumerate() {
            let Some(cell) = row.get(cell_index) else {
                continue;
            };
            if cell.is_empty() {
                continue;
            }

            let label = format!("{header}:");
            let wrapped = wrap_text(cell, value_width);
            for (line_index, value) in wrapped.into_iter().enumerate() {
                let mut spans = marker_spans(show_marker, marker_color);
                if line_index == 0 {
                    spans.push(Span::styled(
                        format!("{label:<label_width$}  "),
                        Style::default().fg(code_type()).add_modifier(Modifier::BOLD),
                    ));
                } else {
                    spans.push(Span::styled(
                        " ".repeat(label_width + 2),
                        Style::default().fg(ghost()),
                    ));
                }
                spans.extend(inline_text_spans(&value, text()));
                lines.push(Line::from(spans));
            }
        }
    }

    lines
}

fn table_row_spans(cells: &[String], widths: &[usize]) -> Vec<Span<'static>> {
    table_row_spans_with_header(cells, widths, false)
}

fn table_row_spans_with_header(
    cells: &[String],
    widths: &[usize],
    header: bool,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ", Style::default().fg(ghost())));
        }
        let mut style = Style::default().fg(if header { code_type() } else { text() });
        if header {
            style = style.add_modifier(Modifier::BOLD);
        }
        spans.extend(inline_text_spans(
            cell,
            if header { code_type() } else { text() },
        ));
        let width = widths.get(index).copied().unwrap_or(0);
        let padding = width.saturating_sub(rendered_inline_width(cell));
        if padding > 0 {
            spans.push(Span::styled(" ".repeat(padding), style));
        }
    }
    spans
}

fn rendered_inline_width(content: &str) -> usize {
    inline_text_spans(content, crate::theme::text())
        .iter()
        .map(|span| span.content.chars().count())
        .sum()
}

fn ordered_list_marker(text: &str) -> Option<(String, &str)> {
    let digit_len = text.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digit_len == 0 {
        return None;
    }

    let after_digits = text.get(digit_len..)?;
    let marker_len = if after_digits.starts_with(". ") || after_digits.starts_with(") ") {
        digit_len + 2
    } else {
        return None;
    };

    Some((text[..marker_len].to_string(), &text[marker_len..]))
}

fn inline_text_spans(text: &str, fallback: Color) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut plain = String::new();
    let mut index = 0;

    while index < text.len() {
        let rest = &text[index..];

        if let Some((marker_len, content, modifier, color, recursive)) = inline_delimited(rest) {
            push_plain_span(&mut spans, &mut plain, fallback);
            if recursive {
                spans.extend(
                    inline_text_spans(content, color)
                        .into_iter()
                        .map(|mut span| {
                            span.style = span.style.add_modifier(modifier);
                            span
                        }),
                );
            } else {
                spans.push(Span::styled(
                    content.to_string(),
                    Style::default().fg(color).add_modifier(modifier),
                ));
            }
            index += marker_len + content.len() + marker_len;
            continue;
        }

        if let Some((escaped, consumed)) = inline_escape(rest) {
            plain.push(escaped);
            index += consumed;
            continue;
        }

        if let Some((alt, url, consumed)) = inline_image(rest) {
            push_plain_span(&mut spans, &mut plain, fallback);
            spans.push(Span::styled(
                alt.to_string(),
                Style::default().fg(code_type()).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" (image: {url})"),
                Style::default().fg(muted()),
            ));
            index += consumed;
            continue;
        }

        if let Some((label, url, consumed)) = inline_link(rest) {
            push_plain_span(&mut spans, &mut plain, fallback);
            spans.push(Span::styled(
                label.to_string(),
                Style::default().fg(code_type()).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" ({url})"),
                Style::default().fg(muted()),
            ));
            index += consumed;
            continue;
        }

        if let Some(ch) = rest.chars().next() {
            plain.push(ch);
            index += ch.len_utf8();
        } else {
            break;
        }
    }
    push_plain_span(&mut spans, &mut plain, fallback);
    spans
}

fn inline_delimited(rest: &str) -> Option<(usize, &str, Modifier, Color, bool)> {
    let patterns = [
        ("`", Modifier::empty(), code_string(), false),
        ("***", Modifier::BOLD | Modifier::ITALIC, text(), true),
        ("___", Modifier::BOLD | Modifier::ITALIC, text(), true),
        ("**", Modifier::BOLD, text(), true),
        ("__", Modifier::BOLD, text(), true),
        ("~~", Modifier::CROSSED_OUT, muted(), true),
        ("*", Modifier::ITALIC, muted(), true),
        ("_", Modifier::ITALIC, muted(), true),
    ];

    for (marker, modifier, color, recursive) in patterns {
        if let Some(after_start) = rest.strip_prefix(marker)
            && let Some(end) = after_start.find(marker)
            && end > 0
        {
            return Some((
                marker.len(),
                &after_start[..end],
                modifier,
                color,
                recursive,
            ));
        }
    }

    None
}

fn inline_escape(rest: &str) -> Option<(char, usize)> {
    let escaped = rest.strip_prefix('\\')?.chars().next()?;
    if matches!(
        escaped,
        '\\' | '`'
            | '*'
            | '_'
            | '{'
            | '}'
            | '['
            | ']'
            | '('
            | ')'
            | '#'
            | '+'
            | '-'
            | '.'
            | '!'
            | '|'
            | '~'
    ) {
        Some((escaped, 1 + escaped.len_utf8()))
    } else {
        None
    }
}

fn inline_image(rest: &str) -> Option<(&str, &str, usize)> {
    let after_bang = rest.strip_prefix('!')?;
    let (alt, url, consumed) = inline_link(after_bang)?;
    Some((alt, url, consumed + 1))
}

fn inline_link(rest: &str) -> Option<(&str, &str, usize)> {
    let after_open = rest.strip_prefix('[')?;
    let label_end = after_open.find("](")?;
    let label = &after_open[..label_end];
    let after_label = &after_open[label_end + 2..];
    let url_end = after_label.find(')')?;
    let url = &after_label[..url_end];
    if label.is_empty() || url.is_empty() {
        return None;
    }
    Some((label, url, 1 + label_end + 2 + url_end + 1))
}

fn push_plain_span(spans: &mut Vec<Span<'static>>, plain: &mut String, fallback: Color) {
    if plain.is_empty() {
        return;
    }
    spans.push(Span::styled(
        std::mem::take(plain),
        Style::default().fg(fallback),
    ));
}

fn markdown_boundary_line(language: &str, show_marker: bool, marker_color: Color) -> Line<'static> {
    let mut spans = marker_spans(show_marker, marker_color);
    let label = if language.is_empty() {
        "```".to_string()
    } else {
        format!("```{language}")
    };
    spans.push(Span::styled(label, Style::default().fg(ghost())));
    Line::from(spans)
}

fn code_line(
    raw_line: &str,
    language: &str,
    show_marker: bool,
    marker_color: Color,
) -> Line<'static> {
    let mut spans = marker_spans(show_marker, marker_color);
    spans.extend(highlight_code_line(raw_line, language));
    Line::from(spans)
}

fn marker_spans(show_marker: bool, marker_color: Color) -> Vec<Span<'static>> {
    if show_marker {
        vec![Span::styled("│ ", Style::default().fg(marker_color))]
    } else {
        Vec::new()
    }
}
