use ratatui::layout::Rect;
use ratatui::prelude::{Line, Modifier, Span, Style};
use ratatui::style::Color;
use ratatui::widgets::{Block, BorderType, Borders};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SyntectStyle, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

use std::sync::OnceLock;

use navi_core::{ToolInvocation, ToolResult};

use crate::state::{ChatMessage, ChatRole};
use crate::theme::*;
use crate::ui::layout::ModalSpec;
use crate::ui::list::SelectListState;

// ─── tool rendering ──────────────────────────────────────────────────────────

pub(crate) fn tool_compact_text(invocation: &ToolInvocation, result: &ToolResult) -> String {
    format!(
        "{} called · {}",
        invocation.tool_name,
        if result.ok { "success" } else { "error" }
    )
}

pub(crate) fn tool_full_content(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let mut content = format!(
        "{} {}\n\n",
        if result.ok { "✓" } else { "✗" },
        tool_compact_text(invocation, result),
    );

    if let Some(formatted) = formatted_tool_output(invocation, result) {
        content.push_str(&formatted);
    } else {
        content.push_str(&generic_tool_summary(invocation, result));
    }

    content
}

pub(crate) fn formatted_tool_output(
    invocation: &ToolInvocation,
    result: &ToolResult,
) -> Option<String> {
    let obj = result.output.as_object()?;
    let mut content = String::new();

    if let Some(error) = obj.get("error").and_then(|v| v.as_str()) {
        content.push_str(&format!("Error: {error}\n"));
        if invocation.tool_name == "bash" {
            let stdout = obj.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
            let stderr = obj.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
            if !stdout.is_empty() {
                content.push_str("\nStdout:\n```\n");
                content.push_str(stdout);
                if !stdout.ends_with('\n') {
                    content.push('\n');
                }
                content.push_str("```\n");
            }
            if !stderr.is_empty() {
                content.push_str("\nStderr:\n```\n");
                content.push_str(stderr);
                if !stderr.ends_with('\n') {
                    content.push('\n');
                }
                content.push_str("```\n");
            }
        }
        return Some(content);
    }

    if !result.ok && invocation.tool_name != "bash" {
        return None;
    }

    if invocation.tool_name == "read_file" || invocation.tool_name == "view_file" {
        let path = obj.get("path").and_then(|v| v.as_str())?;
        content.push_str(&format!("View {path}\n\n"));
        if let Some(file_content) = obj.get("content").and_then(|v| v.as_str()) {
            let language = language_for_path(path);
            content.push_str(&format!("```{language}\n"));
            content.push_str(file_content);
            if !file_content.ends_with('\n') {
                content.push('\n');
            }
            content.push_str("```\n");
        }
    } else if invocation.tool_name == "write_file" {
        let path = obj.get("path").and_then(|v| v.as_str())?;
        let added = invocation
            .input
            .get("content")
            .and_then(|v| v.as_str())
            .map(count_changed_lines)
            .unwrap_or(0);
        content.push_str(&format!("Edited {path} (+{added} -0)\n"));
    } else if invocation.tool_name == "apply_patch" {
        if let Some(patch) = invocation.input.get("patch").and_then(|v| v.as_str()) {
            let summaries = patch_edit_summaries(patch);
            if summaries.is_empty() {
                content.push_str("Applied patch\n");
            } else {
                for summary in summaries {
                    content.push_str(&summary);
                    content.push('\n');
                }
            }
        } else {
            content.push_str("Applied patch successfully\n");
        }
        let stdout = obj.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        let stderr = obj.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
        if !stdout.is_empty() {
            content.push_str("\nStdout:\n```\n");
            content.push_str(stdout);
            if !stdout.ends_with('\n') {
                content.push('\n');
            }
            content.push_str("```\n");
        }
        if !stderr.is_empty() {
            content.push_str("\nStderr:\n```\n");
            content.push_str(stderr);
            if !stderr.ends_with('\n') {
                content.push('\n');
            }
            content.push_str("```\n");
        }
    } else if invocation.tool_name == "bash" {
        let status = obj.get("status").and_then(|v| v.as_i64());
        if let Some(status_code) = status {
            content.push_str(&format!("Command exited with status {status_code}\n"));
        } else {
            content.push_str("Command completed\n");
        }
        let stdout = obj.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        let stderr = obj.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
        if !stdout.is_empty() {
            content.push_str("\nStdout:\n```\n");
            content.push_str(stdout);
            if !stdout.ends_with('\n') {
                content.push('\n');
            }
            content.push_str("```\n");
        }
        if !stderr.is_empty() {
            content.push_str("\nStderr:\n```\n");
            content.push_str(stderr);
            if !stderr.ends_with('\n') {
                content.push('\n');
            }
            content.push_str("```\n");
        }
    } else if invocation.tool_name == "grep" {
        content.push_str("Found matches:\n\n");
        if let Some(matches) = obj.get("matches").and_then(|v| v.as_array()) {
            for m in matches {
                if let Some(m_obj) = m.as_object() {
                    let path = m_obj.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    let line = m_obj.get("line").and_then(|v| v.as_u64()).unwrap_or(0);
                    let text = m_obj.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    content.push_str(&format!("{path}:{line}: {text}\n"));
                }
            }
        }
    } else if invocation.tool_name == "list_files" {
        content.push_str("List files\n\n");
        if let Some(files) = obj.get("files").and_then(|v| v.as_array()) {
            for (i, file) in files.iter().enumerate() {
                if let Some(file) = file.as_str() {
                    content.push_str(&format!("{:>4}  {}\n", i + 1, file));
                }
            }
        }
    } else {
        return None;
    }

    if obj.get("truncated").and_then(|v| v.as_bool()) == Some(true) {
        content.push_str("... (truncated)\n");
    }
    Some(content)
}

pub(crate) fn generic_tool_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    if result.ok {
        format!("{} completed successfully\n", invocation.tool_name)
    } else if let Some(error) = result.output.get("error").and_then(|v| v.as_str()) {
        format!("Error: {error}\n")
    } else {
        format!("{} failed\n", invocation.tool_name)
    }
}

pub(crate) fn count_changed_lines(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.lines().count().max(1)
    }
}

pub(crate) fn patch_edit_summaries(patch: &str) -> Vec<String> {
    let mut summaries = Vec::new();
    let mut current_path: Option<String> = None;
    let mut added = 0usize;
    let mut removed = 0usize;

    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            flush_patch_summary(&mut summaries, &mut current_path, &mut added, &mut removed);
            current_path = Some(path.to_string());
            continue;
        }
        if current_path.is_none() {
            if let Some(path) = line.strip_prefix("*** Update File: ") {
                current_path = Some(path.to_string());
                continue;
            }
            if let Some(path) = line.strip_prefix("*** Add File: ") {
                current_path = Some(path.to_string());
                continue;
            }
        }
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    flush_patch_summary(&mut summaries, &mut current_path, &mut added, &mut removed);

    summaries
}

pub(crate) fn flush_patch_summary(
    summaries: &mut Vec<String>,
    current_path: &mut Option<String>,
    added: &mut usize,
    removed: &mut usize,
) {
    if let Some(path) = current_path.take() {
        summaries.push(format!("Edited {path} (+{} -{})", *added, *removed));
        *added = 0;
        *removed = 0;
    }
}

pub(crate) fn language_for_path(path: &str) -> &'static str {
    match path
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or_default()
    {
        "rs" => "rust",
        "toml" => "toml",
        "json" => "json",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" => "typescript",
        "jsx" => "javascript",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cc" | "cpp" | "hpp" => "cpp",
        "sh" | "bash" => "bash",
        "zsh" => "zsh",
        "fish" => "fish",
        "md" | "markdown" => "markdown",
        "yaml" | "yml" => "yaml",
        "html" => "html",
        "css" => "css",
        "xml" => "xml",
        "sql" => "sql",
        _ => "",
    }
}

// ─── markdown rendering ──────────────────────────────────────────────────────

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
                    USER_ACCENT,
                    TEXT,
                    false,
                ));
            }
            ChatRole::Assistant => {
                if msg.is_compact_summary {
                    rendered_lines.push(Line::from(vec![
                        Span::styled(
                            " ◈ compacted ",
                            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            "─".repeat(chat_width.saturating_sub(14)),
                            Style::default().fg(GHOST),
                        ),
                    ]));
                }
                if let Some((invocation, result)) = tool_result_parts(msg) {
                    if full_tool_view {
                        rendered_lines.extend(render_markdown_lines(
                            &tool_full_content(invocation, result),
                            chat_width.saturating_sub(2),
                            TEXT,
                            TEXT,
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
                            MUTED,
                            MUTED,
                            true,
                        ));
                        if !msg.content.is_empty() {
                            rendered_lines.push(Line::from(""));
                        }
                    }
                    rendered_lines.extend(render_markdown_lines(
                        &msg.content,
                        chat_width.saturating_sub(2),
                        TEXT,
                        TEXT,
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
                    let dashes: String = std::iter::repeat('─').take(dash_count).collect();

                    rendered_lines.push(Line::from(vec![
                        Span::styled(format!(" {attr_text} "), Style::default().fg(MUTED)),
                        Span::styled(dashes, Style::default().fg(GHOST)),
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

pub(crate) fn tool_result_parts(message: &ChatMessage) -> Option<(&ToolInvocation, &ToolResult)> {
    match (&message.tool_invocation, &message.tool_result) {
        (Some(invocation), Some(result)) => Some((invocation, result)),
        _ => None,
    }
}

pub(crate) fn render_compact_tool_line(
    invocation: &ToolInvocation,
    result: &ToolResult,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "● ",
            Style::default().fg(if result.ok { Color::Green } else { Color::Red }),
        ),
        Span::styled(
            tool_compact_text(invocation, result),
            Style::default().fg(TEXT),
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

pub(crate) fn text_line(
    text: String,
    show_marker: bool,
    marker_color: Color,
    text_color: Color,
    italic: bool,
) -> Line<'static> {
    let mut spans = marker_spans(show_marker, marker_color);
    if !italic {
        if let Some(markdown_line) = markdown_prose_line(&text, text_color) {
            spans.extend(markdown_line);
            return Line::from(spans);
        }
    }

    let mut style = Style::default().fg(text_color);
    if italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    spans.push(Span::styled(text, style));
    Line::from(spans)
}

pub(crate) fn markdown_prose_line(text: &str, fallback: Color) -> Option<Vec<Span<'static>>> {
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
            Style::default().fg(PINK).add_modifier(Modifier::BOLD),
        ));
        spans.extend(
            inline_text_spans(&trimmed[heading + 1..], TEXT)
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
            Style::default().fg(PINK).add_modifier(Modifier::BOLD),
        ));
        spans.extend(inline_text_spans(rest, MUTED));
        return Some(spans);
    }

    if trimmed.starts_with('|') && trimmed.ends_with('|') {
        spans.extend(table_row_spans(&table_cells(trimmed), &[]));
        return Some(spans);
    }

    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        spans.push(Span::styled(
            "• ",
            Style::default().fg(PINK).add_modifier(Modifier::BOLD),
        ));
        spans.extend(inline_text_spans(&trimmed[2..], fallback));
        return Some(spans);
    }

    if let Some((marker, rest)) = ordered_list_marker(trimmed) {
        spans.push(Span::styled(
            marker,
            Style::default().fg(PINK).add_modifier(Modifier::BOLD),
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

pub(crate) fn is_table_line(text: &str) -> bool {
    text.starts_with('|') && text.ends_with('|') && text.matches('|').count() >= 2
}

pub(crate) fn is_table_separator(text: &str) -> bool {
    is_table_line(text)
        && table_cells(text).iter().all(|cell| {
            let cell = cell.trim();
            !cell.is_empty() && cell.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
        })
}

pub(crate) fn table_cells(text: &str) -> Vec<String> {
    text.trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

pub(crate) fn table_block_lines(
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
                        Style::default().fg(CODE_TYPE).add_modifier(Modifier::BOLD),
                    ));
                } else {
                    spans.push(Span::styled(
                        " ".repeat(label_width + 2),
                        Style::default().fg(GHOST),
                    ));
                }
                spans.extend(inline_text_spans(&value, TEXT));
                lines.push(Line::from(spans));
            }
        }
    }

    lines
}

pub(crate) fn table_row_spans(cells: &[String], widths: &[usize]) -> Vec<Span<'static>> {
    table_row_spans_with_header(cells, widths, false)
}

pub(crate) fn table_row_spans_with_header(
    cells: &[String],
    widths: &[usize],
    header: bool,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ", Style::default().fg(GHOST)));
        }
        let mut style = Style::default().fg(if header { CODE_TYPE } else { TEXT });
        if header {
            style = style.add_modifier(Modifier::BOLD);
        }
        spans.extend(inline_text_spans(
            cell,
            if header { CODE_TYPE } else { TEXT },
        ));
        let width = widths.get(index).copied().unwrap_or(0);
        let padding = width.saturating_sub(rendered_inline_width(cell));
        if padding > 0 {
            spans.push(Span::styled(" ".repeat(padding), style));
        }
    }
    spans
}

pub(crate) fn rendered_inline_width(text: &str) -> usize {
    inline_text_spans(text, TEXT)
        .iter()
        .map(|span| span.content.chars().count())
        .sum()
}

pub(crate) fn ordered_list_marker(text: &str) -> Option<(String, &str)> {
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

pub(crate) fn inline_text_spans(text: &str, fallback: Color) -> Vec<Span<'static>> {
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
                Style::default().fg(CODE_TYPE).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" (image: {url})"),
                Style::default().fg(MUTED),
            ));
            index += consumed;
            continue;
        }

        if let Some((label, url, consumed)) = inline_link(rest) {
            push_plain_span(&mut spans, &mut plain, fallback);
            spans.push(Span::styled(
                label.to_string(),
                Style::default().fg(CODE_TYPE).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" ({url})"),
                Style::default().fg(MUTED),
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

pub(crate) fn inline_delimited(rest: &str) -> Option<(usize, &str, Modifier, Color, bool)> {
    let patterns = [
        ("`", Modifier::empty(), CODE_STRING, false),
        ("***", Modifier::BOLD | Modifier::ITALIC, TEXT, true),
        ("___", Modifier::BOLD | Modifier::ITALIC, TEXT, true),
        ("**", Modifier::BOLD, TEXT, true),
        ("__", Modifier::BOLD, TEXT, true),
        ("~~", Modifier::CROSSED_OUT, MUTED, true),
        ("*", Modifier::ITALIC, MUTED, true),
        ("_", Modifier::ITALIC, MUTED, true),
    ];

    for (marker, modifier, color, recursive) in patterns {
        if let Some(after_start) = rest.strip_prefix(marker) {
            if let Some(end) = after_start.find(marker) {
                if end > 0 {
                    return Some((
                        marker.len(),
                        &after_start[..end],
                        modifier,
                        color,
                        recursive,
                    ));
                }
            }
        }
    }

    None
}

pub(crate) fn inline_escape(rest: &str) -> Option<(char, usize)> {
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

pub(crate) fn inline_image(rest: &str) -> Option<(&str, &str, usize)> {
    let after_bang = rest.strip_prefix('!')?;
    let (alt, url, consumed) = inline_link(after_bang)?;
    Some((alt, url, consumed + 1))
}

pub(crate) fn inline_link(rest: &str) -> Option<(&str, &str, usize)> {
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

pub(crate) fn push_plain_span(spans: &mut Vec<Span<'static>>, plain: &mut String, fallback: Color) {
    if plain.is_empty() {
        return;
    }
    spans.push(Span::styled(
        std::mem::take(plain),
        Style::default().fg(fallback),
    ));
}

pub(crate) fn markdown_boundary_line(
    language: &str,
    show_marker: bool,
    marker_color: Color,
) -> Line<'static> {
    let mut spans = marker_spans(show_marker, marker_color);
    let label = if language.is_empty() {
        "```".to_string()
    } else {
        format!("```{language}")
    };
    spans.push(Span::styled(label, Style::default().fg(GHOST)));
    Line::from(spans)
}

pub(crate) fn code_line(
    raw_line: &str,
    language: &str,
    show_marker: bool,
    marker_color: Color,
) -> Line<'static> {
    let mut spans = marker_spans(show_marker, marker_color);
    spans.extend(highlight_code_line(raw_line, language));
    Line::from(spans)
}

pub(crate) fn marker_spans(show_marker: bool, marker_color: Color) -> Vec<Span<'static>> {
    if show_marker {
        vec![Span::styled("│ ", Style::default().fg(marker_color))]
    } else {
        Vec::new()
    }
}

pub(crate) fn highlight_code_line(raw_line: &str, language: &str) -> Vec<Span<'static>> {
    let syntax_set = syntax_set();
    let syntax = syntax_set
        .find_syntax_by_token(language)
        .or_else(|| syntax_set.find_syntax_by_extension(language))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
    let mut highlighter = HighlightLines::new(syntax, syntax_theme());

    match highlighter.highlight_line(raw_line, syntax_set) {
        Ok(ranges) => ranges
            .into_iter()
            .map(|(style, text)| Span::styled(text.to_string(), syntect_style(style)))
            .collect(),
        Err(_) => vec![Span::styled(
            raw_line.to_string(),
            Style::default().fg(TEXT),
        )],
    }
}

pub(crate) fn syntect_style(style: SyntectStyle) -> Style {
    Style::default().fg(lain_code_color(style))
}

pub(crate) fn lain_code_color(style: SyntectStyle) -> Color {
    let color = style.foreground;
    if style
        .font_style
        .contains(syntect::highlighting::FontStyle::ITALIC)
        || (color.r < 118 && color.g < 118 && color.b < 118)
    {
        CODE_COMMENT
    } else if style
        .font_style
        .contains(syntect::highlighting::FontStyle::BOLD)
    {
        CODE_FUNC
    } else if color.r > 190 && color.b > 165 && color.g < 170 {
        CODE_KEYWORD
    } else if color.g > color.r.saturating_add(25) && color.g > color.b.saturating_add(5) {
        Color::Rgb(143, 232, 173)
    } else if color.b > color.r.saturating_add(25) && color.g > color.r.saturating_add(10) {
        CODE_TYPE
    } else if color.b > color.r.saturating_add(25) {
        CODE_NUMBER
    } else if color.r > 175 && color.g > 145 && color.b < 145 {
        CODE_CONST
    } else if color.r > 180 && color.b > 95 && color.g < 135 {
        CODE_OPERATOR
    } else if color.r < 175 && color.g < 175 && color.b < 175 {
        CODE_PUNCT
    } else if color.r > 200 && color.g > 200 && color.b > 200 {
        TEXT
    } else {
        Color::Rgb(
            boost_code_channel(color.r),
            boost_code_channel(color.g),
            boost_code_channel(color.b),
        )
    }
}

pub(crate) fn boost_code_channel(value: u8) -> u8 {
    value.max(96).saturating_add(22)
}

pub(crate) fn syntax_set() -> &'static SyntaxSet {
    static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

pub(crate) fn syntax_theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let themes = ThemeSet::load_defaults();
        themes
            .themes
            .get("base16-ocean.dark")
            .or_else(|| themes.themes.values().next())
            .cloned()
            .unwrap_or_default()
    })
}

// ─── text helpers ────────────────────────────────────────────────────────────

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
        for word in paragraph.split_whitespace() {
            if current_line.is_empty() {
                current_line = word.to_string();
            } else if current_line.chars().count() + 1 + word.chars().count() <= max_width {
                current_line.push(' ');
                current_line.push_str(word);
            } else {
                lines.push(current_line);
                current_line = word.to_string();
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

// ─── input helpers ───────────────────────────────────────────────────────────

pub(crate) fn split_input_spans<'a>(spans: Vec<Span<'a>>, continuation: &str) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    let mut current = Vec::new();

    for span in spans {
        let content = span.content.clone();
        let style = span.style;
        let mut parts = content.split('\n').peekable();
        while let Some(part) = parts.next() {
            if !part.is_empty() {
                current.push(Span::styled(part.to_string(), style));
            }
            if parts.peek().is_some() {
                lines.push(Line::from(current));
                current = Vec::new();
                current.push(Span::raw(continuation.to_string()));
            }
        }
    }

    if !current.is_empty() || lines.is_empty() {
        lines.push(Line::from(current));
    }

    lines
}

pub(crate) fn cursor_span(value: &str) -> Span<'_> {
    Span::styled(
        value,
        Style::default()
            .fg(BG)
            .bg(SIGNAL)
            .add_modifier(Modifier::BOLD),
    )
}

// ─── mask helpers ────────────────────────────────────────────────────────────

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

// ─── shared rendering helpers ────────────────────────────────────────────────

pub(crate) fn command_scroll_offset(selected: usize, visible_rows: usize) -> usize {
    SelectListState::scroll_offset_for_selected(selected, visible_rows)
}

pub(crate) fn modal_block(title: &'static str) -> Block<'static> {
    Block::new()
        .title(Line::from(vec![
            Span::styled(format!(" {title} "), Style::default().fg(RED)),
            Span::styled("  online", Style::default().fg(MUTED)),
        ]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().fg(TEXT).bg(PANEL))
}

pub(crate) fn truncate_display(value: &str, max_chars: usize) -> String {
    let mut result = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        result.push_str("\n<truncated>");
    }
    result
}

pub(crate) fn command_row(label: &str, shortcut: &str, width: usize) -> String {
    let shortcut_width = 12usize.min(width.saturating_sub(1));
    let label_width = width.saturating_sub(shortcut_width + 1);
    format!(
        "{:<label_width$} {:<shortcut_width$}",
        fit_text(label, label_width),
        fit_text(shortcut, shortcut_width)
    )
}

pub(crate) fn model_row_simple(name: &str, configured: bool, width: usize) -> String {
    let marker_width = 3usize.min(width);
    let name_width = width.saturating_sub(marker_width + 4);
    let marker = if configured { "✓" } else { "" };

    format!(
        "    {:<name_width$} {:<marker_width$}",
        fit_text(name, name_width),
        marker
    )
}

pub(crate) fn fit_text(value: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let mut text = value.chars().take(width).collect::<String>();
    if value.chars().count() > width && width > 1 {
        text.pop();
        text.push('…');
    }
    text
}

pub(crate) fn modal_rect(area: Rect, max_width: u16, height: u16) -> Rect {
    ModalSpec::fixed(max_width, height).rect(area)
}

#[cfg(test)]
mod tests {
    use ratatui::prelude::Modifier;

    use super::*;
    use crate::theme::TEXT;

    fn line_text(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn wrap_text_handles_long_lines() {
        let text = "Hello world this is a very long line that should wrap properly";
        let lines = wrap_text(text, 20);
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(line.chars().count() <= 20);
        }
    }

    #[test]
    fn wrap_text_preserves_newlines() {
        let text = "Line one\nLine two\nLine three";
        let lines = wrap_text(text, 50);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "Line one");
        assert_eq!(lines[1], "Line two");
        assert_eq!(lines[2], "Line three");
    }

    #[test]
    fn markdown_renderer_wraps_plain_text() {
        let lines = render_markdown_lines("hello world from navi", 12, TEXT, TEXT, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(rendered, vec!["hello world", "from navi"]);
    }

    #[test]
    fn markdown_renderer_preserves_fenced_code_blocks() {
        let lines = render_markdown_lines(
            "before\n```rust\nfn main() {}\n```\nafter",
            80,
            TEXT,
            TEXT,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec!["before", "```rust", "fn main() {}", "```", "after"]
        );
    }

    #[test]
    fn markdown_renderer_handles_unclosed_fence() {
        let lines = render_markdown_lines("```unknown\n  value", 80, TEXT, TEXT, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(rendered, vec!["```unknown", "  value"]);
    }

    #[test]
    fn markdown_renderer_renders_inline_markup() {
        let lines = render_markdown_lines(
            "**NAVI** is `wired` and [documented](https://example.test)",
            120,
            TEXT,
            TEXT,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec!["NAVI is wired and documented (https://example.test)"]
        );
        assert!(
            lines[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn markdown_renderer_handles_nested_and_extended_inline_markup() {
        let lines = render_markdown_lines(
            "**`NAVI`** uses ***strong emphasis***, ~~old text~~, ![diagram](file.png), and \\*literal\\*.",
            160,
            TEXT,
            TEXT,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec!["NAVI uses strong emphasis, old text, diagram (image: file.png), and *literal*."]
        );
        let navi = lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "NAVI")
            .expect("nested code span");
        assert_eq!(navi.style.fg, Some(CODE_STRING));
        assert!(navi.style.add_modifier.contains(Modifier::BOLD));

        let strong_emphasis = lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "strong emphasis")
            .expect("strong emphasis span");
        assert!(strong_emphasis.style.add_modifier.contains(Modifier::BOLD));
        assert!(
            strong_emphasis
                .style
                .add_modifier
                .contains(Modifier::ITALIC)
        );

        let old = lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "old text")
            .expect("strikethrough span");
        assert!(old.style.add_modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn markdown_renderer_handles_lists_and_quotes() {
        let lines = render_markdown_lines(
            "1. **Architecture**\n> signal in prose",
            120,
            TEXT,
            TEXT,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(rendered, vec!["1. Architecture", "▌ signal in prose"]);
    }

    #[test]
    fn markdown_renderer_consumes_headings_and_table_pipes() {
        let lines = render_markdown_lines(
            "## Project Overview\n\n| Crate | Purpose |\n|---|---|\n| `navi-cli` | Entry binary |",
            120,
            TEXT,
            TEXT,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "▣ Project Overview",
                "",
                "Crate     Purpose     ",
                "navi-cli  Entry binary",
            ]
        );
        assert!(!rendered.iter().any(|line| line.contains("##")));
        assert!(!rendered.iter().skip(2).any(|line| line.contains('|')));
    }

    #[test]
    fn markdown_renderer_stacks_wide_tables() {
        let lines = render_markdown_lines(
            "| Problema | Onde | Gravidade |\n|---|---|---|\n| OAuth Device Flow na TUI | navi-tui/src/runtime.rs contém HTTP calls, polling loop e JSON parsing | CRÍTICO |\n| Flat module tree | lib.rs re-exporta tudo num namespace plano | ALTO |",
            64,
            TEXT,
            TEXT,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.starts_with("Problema:")));
        assert!(rendered.iter().any(|line| line.starts_with("Onde:")));
        assert!(rendered.iter().any(|line| line.starts_with("Gravidade:")));
        assert!(rendered.iter().all(|line| !line.contains('|')));
        for line in rendered.iter().filter(|line| !line.is_empty()) {
            assert!(line.chars().count() <= 64, "line too wide: {line}");
        }
    }

    #[test]
    fn code_highlighting_uses_varied_colors() {
        let spans = highlight_code_line("fn main() { let value = \"x\"; }", "rust");
        let mut colors = Vec::new();
        for color in spans.iter().filter_map(|span| span.style.fg) {
            if !colors.contains(&color) {
                colors.push(color);
            }
        }

        assert!(colors.len() >= 3);
    }

    #[test]
    fn tool_compact_text_is_one_line_with_status() {
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "grep".to_string(),
            input: serde_json::json!({ "pattern": "NAVI" }),
        };
        let ok_result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!({ "matches": [] }),
        };
        let err_result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: false,
            output: serde_json::json!({ "error": "denied" }),
        };

        assert_eq!(
            tool_compact_text(&invocation, &ok_result),
            "grep called · success"
        );
        assert_eq!(
            tool_compact_text(&invocation, &err_result),
            "grep called · error"
        );
        assert!(!tool_compact_text(&invocation, &ok_result).contains('\n'));
    }

    #[test]
    fn tool_full_content_sanitizes_read_file_without_json_io() {
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({ "path": "Cargo.toml" }),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!({
                "path": "Cargo.toml",
                "content": "[workspace]\n",
                "truncated": false,
            }),
        };

        let content = tool_full_content(&invocation, &result);
        assert!(content.contains("read_file called · success"));
        assert!(content.contains("View Cargo.toml"));
        assert!(content.contains("[workspace]"));
        assert!(!content.contains("Input"));
        assert!(!content.contains("Output"));
        assert!(!content.contains("\"path\""));
    }

    #[test]
    fn read_file_tool_full_content_uses_fenced_code_for_highlighting() {
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({ "path": "src/lib.rs" }),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!({
                "path": "src/lib.rs",
                "content": "fn main() {}\n",
            }),
        };

        let content = tool_full_content(&invocation, &result);

        assert!(content.contains("```rust"));
        assert!(content.contains("fn main() {}"));
    }

    #[test]
    fn mask_key_hides_middle_characters() {
        let short = "sk-abc";
        assert_eq!(mask_key_segment(short), "sk-abc");

        let long = "sk-proj-abcdefghijklmnop";
        let masked = mask_key_segment(long);
        assert!(masked.starts_with("sk-pro"));
        assert!(masked.ends_with("mnop"));
        assert!(masked.contains('•'));
    }
}
