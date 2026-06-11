use ratatui::prelude::{Line, Modifier, Span, Style};
use ratatui::style::Color;
use std::collections::{BTreeMap, HashMap, HashSet};

use navi_sdk::{ToolInvocation, ToolResult};

use crate::state::{ChatLineSource, ChatMessage, ChatRole};
use crate::theme::*;

use super::syntax::{CodeHighlighter, highlight_code_line};
use super::text::{display_width, wrap_spans_to_width, wrap_text};
use super::tool::{tool_compact_text, tool_full_content};

#[derive(Debug, Clone)]
pub(crate) struct ChatRenderOutput {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) sources: Vec<ChatLineSource>,
}

pub(crate) fn build_chat_render_for_messages(
    messages: &[ChatMessage],
    chat_width: usize,
    full_tool_view: bool,
    show_thinking: bool,
    compact_tool_visible_limit: usize,
    expanded_tool_results: &HashSet<String>,
    tool_render_cache: &mut HashMap<String, Vec<Line<'static>>>,
    tick: u64,
) -> ChatRenderOutput {
    let mut rendered_lines: Vec<Line<'static>> = Vec::new();
    let mut line_sources: Vec<ChatLineSource> = Vec::new();
    let messages = messages.iter().collect::<Vec<_>>();
    let latest_tool_group_start = latest_tool_group_start(&messages);
    let mut index = 0;

    while index < messages.len() {
        let msg = messages[index];
        if is_empty_tool_placeholder(msg) {
            let status = msg.status.as_deref().unwrap_or("working");
            let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let frame = frames[(tick / 6) as usize % frames.len()];

            if !rendered_lines.is_empty() {
                rendered_lines.push(Line::from(""));
                line_sources.push(ChatLineSource::None);
            }

            rendered_lines.push(Line::from(vec![Span::styled(
                format!("  {frame} {status}..."),
                Style::default().fg(accent()).add_modifier(Modifier::BOLD),
            )]));
            line_sources.push(ChatLineSource::Message(index));

            index += 1;
            continue;
        }
        if !full_tool_view && tool_result_parts(msg).is_some() {
            let mut group = Vec::new();
            let group_start = index;
            while index < messages.len() {
                let group_msg = messages[index];
                if is_transparent_tool_placeholder(group_msg) {
                    index += 1;
                    continue;
                }
                let Some(parts) = tool_result_parts(group_msg) else {
                    break;
                };
                group.push(parts);
                index += 1;
            }
            push_block_gap(&mut rendered_lines, &mut line_sources);
            let expanded = latest_tool_group_start == Some(group_start)
                || group
                    .iter()
                    .any(|(_, result)| expanded_tool_results.contains(&result.invocation_id));
            let rendered_group = render_compact_tool_group(
                &group,
                chat_width,
                expanded,
                compact_tool_visible_limit,
                expanded_tool_results,
                tool_render_cache,
            );
            for (line, source) in rendered_group {
                rendered_lines.push(line);
                line_sources.push(source);
            }
            continue;
        }

        if !rendered_lines.is_empty() {
            rendered_lines.push(Line::from(""));
            line_sources.push(ChatLineSource::None);
        }

        match msg.role {
            ChatRole::User => {
                let lines = render_user_message_lines(&msg.content, chat_width);
                for line in lines {
                    rendered_lines.push(line);
                    line_sources.push(ChatLineSource::Message(index));
                }
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
                    line_sources.push(ChatLineSource::Message(index));
                }
                if let Some((invocation, result)) = tool_result_parts(msg) {
                    if full_tool_view {
                        let source = ChatLineSource::ToolResult(result.invocation_id.clone());
                        let cache_key = format!("full|{}", result.invocation_id);
                        let rendered = if let Some(cached) = tool_render_cache.get(&cache_key) {
                            cached.clone()
                        } else {
                            let rendered = render_markdown_lines(
                                &tool_full_content(invocation, result),
                                chat_width.saturating_sub(2),
                                text(),
                                text(),
                                false,
                            );
                            tool_render_cache.insert(cache_key, rendered.clone());
                            rendered
                        };
                        push_sourced_lines(
                            &mut rendered_lines,
                            &mut line_sources,
                            rendered,
                            source,
                        );
                    } else {
                        rendered_lines.push(render_compact_tool_line(invocation, result));
                        line_sources.push(ChatLineSource::ToolResult(result.invocation_id.clone()));
                    }
                } else {
                    if show_thinking && !msg.thinking_content.is_empty() {
                        push_sourced_lines(
                            &mut rendered_lines,
                            &mut line_sources,
                            render_markdown_lines(
                                &msg.thinking_content,
                                chat_width.saturating_sub(4),
                                muted(),
                                muted(),
                                true,
                            ),
                            ChatLineSource::Message(index),
                        );
                        if !msg.content.is_empty() {
                            rendered_lines.push(Line::from(""));
                            line_sources.push(ChatLineSource::Message(index));
                        }
                    }
                    push_sourced_lines(
                        &mut rendered_lines,
                        &mut line_sources,
                        render_markdown_lines(
                            &msg.content,
                            chat_width.saturating_sub(2),
                            text(),
                            text(),
                            false,
                        ),
                        ChatLineSource::Message(index),
                    );
                }
            }
        }
        index += 1;
    }
    ChatRenderOutput {
        lines: rendered_lines,
        sources: line_sources,
    }
}

fn latest_tool_group_start(messages: &[&ChatMessage]) -> Option<usize> {
    let mut latest = None;
    let mut previous_was_tool = false;
    for (index, message) in messages.iter().enumerate() {
        if is_transparent_tool_placeholder(message) {
            continue;
        }
        let is_tool = tool_result_parts(message).is_some();
        if is_tool && !previous_was_tool {
            latest = Some(index);
        }
        previous_was_tool = is_tool;
    }
    latest
}

fn push_block_gap(lines: &mut Vec<Line<'static>>, sources: &mut Vec<ChatLineSource>) {
    if !lines.is_empty() {
        lines.push(Line::from(""));
        sources.push(ChatLineSource::None);
    }
}

fn push_sourced_lines(
    lines: &mut Vec<Line<'static>>,
    sources: &mut Vec<ChatLineSource>,
    next: Vec<Line<'static>>,
    source: ChatLineSource,
) {
    for line in next {
        lines.push(line);
        sources.push(source.clone());
    }
}

fn render_user_message_lines(text: &str, chat_width: usize) -> Vec<Line<'static>> {
    let width = chat_width.max(8);
    let wrapped = wrap_text(text, width.saturating_sub(4));
    wrapped
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let prefix = if index == 0 { "› " } else { "  " };
            let mut spans = vec![Span::styled(
                prefix,
                Style::default()
                    .fg(user_accent())
                    .bg(panel())
                    .add_modifier(Modifier::BOLD),
            )];
            spans.extend(
                inline_text_spans(&line, text_color_for_user())
                    .into_iter()
                    .map(|mut span| {
                        span.style = span.style.bg(panel());
                        span
                    }),
            );
            let used = spans
                .iter()
                .map(|span| display_width(&span.content))
                .sum::<usize>();
            if used < width {
                spans.push(Span::styled(
                    " ".repeat(width - used),
                    Style::default().fg(muted()).bg(panel()),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

fn text_color_for_user() -> Color {
    text()
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

fn is_transparent_tool_placeholder(message: &ChatMessage) -> bool {
    message.role == ChatRole::Assistant
        && message.content.trim().is_empty()
        && message.thinking_content.trim().is_empty()
        && message
            .status
            .as_deref()
            .is_some_and(|status| status.starts_with("tool:") || status.starts_with("approval:"))
}

fn tool_result_parts(message: &ChatMessage) -> Option<(&ToolInvocation, &ToolResult)> {
    match (&message.tool_invocation, &message.tool_result) {
        (Some(invocation), Some(result)) => Some((invocation, result)),
        _ => None,
    }
}

fn render_compact_tool_line(invocation: &ToolInvocation, result: &ToolResult) -> Line<'static> {
    render_compact_tool_line_with_width(invocation, result, usize::MAX)
}

fn render_compact_tool_group(
    tools: &[(&ToolInvocation, &ToolResult)],
    chat_width: usize,
    expanded: bool,
    visible_limit: usize,
    expanded_tool_results: &HashSet<String>,
    tool_render_cache: &mut HashMap<String, Vec<Line<'static>>>,
) -> Vec<(Line<'static>, ChatLineSource)> {
    let ids = tools
        .iter()
        .map(|(_, result)| result.invocation_id.clone())
        .collect::<Vec<_>>();
    if !expanded {
        return vec![(
            render_collapsed_tool_group(tools, chat_width),
            ChatLineSource::ToolGroup(ids),
        )];
    }

    let mut lines = Vec::new();
    let visible_limit = visible_limit.max(1);
    let hidden = tools.len().saturating_sub(visible_limit);
    if hidden > 0 {
        lines.push((
            Line::from(vec![
                Span::styled("  ", Style::default().fg(ghost())),
                Span::styled(
                    format!("{hidden} earlier tool calls"),
                    Style::default().fg(muted()).add_modifier(Modifier::BOLD),
                ),
            ]),
            ChatLineSource::ToolGroup(ids.clone()),
        ));
    }

    let start = tools.len().saturating_sub(visible_limit);
    for (invocation, result) in &tools[start..] {
        let source = ChatLineSource::ToolResult(result.invocation_id.clone());
        if expanded_tool_results.contains(&result.invocation_id) {
            lines.push((
                Line::from(Span::styled(
                    "Click to collapse",
                    Style::default().fg(muted()).add_modifier(Modifier::BOLD),
                )),
                source.clone(),
            ));
            let cache_key = result.invocation_id.clone();
            let rendered = if let Some(cached) = tool_render_cache.get(&cache_key) {
                cached.clone()
            } else {
                let rendered = render_markdown_lines(
                    &tool_full_content(invocation, result),
                    chat_width.saturating_sub(2),
                    text(),
                    text(),
                    false,
                );
                tool_render_cache.insert(cache_key, rendered.clone());
                rendered
            };
            for line in rendered {
                lines.push((line, source.clone()));
            }
        } else {
            lines.push((
                render_compact_tool_line_with_width(invocation, result, chat_width),
                source,
            ));
        }
    }
    lines
}

fn render_collapsed_tool_group(
    tools: &[(&ToolInvocation, &ToolResult)],
    chat_width: usize,
) -> Line<'static> {
    let marker = "◆ ";
    let marker_width = marker.chars().count();
    let text_width = chat_width.saturating_sub(marker_width).max(12);
    let errors = tools.iter().filter(|(_, result)| !result.ok).count();
    let mut counts = BTreeMap::new();
    for (invocation, _) in tools {
        *counts
            .entry(tool_group_label(&invocation.tool_name))
            .or_insert(0usize) += 1;
    }

    let mut detail = counts
        .into_iter()
        .map(|(label, count)| {
            if count == 1 {
                label
            } else {
                format!("{label} x{count}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    if errors > 0 {
        detail = format!(
            "{errors} error{} · {detail}",
            if errors == 1 { "" } else { "s" }
        );
    }

    let noun = if tools.len() == 1 { "tool" } else { "tools" };
    Line::from(vec![
        Span::styled(
            marker,
            Style::default().fg(if errors > 0 { Color::Red } else { Color::Green }),
        ),
        Span::styled(
            truncate_chars(&format!("{} {noun} · {detail}", tools.len()), text_width),
            Style::default().fg(muted()),
        ),
    ])
}

fn tool_group_label(tool_name: &str) -> String {
    match tool_name {
        "read_file" | "view_file" => "Read".to_string(),
        "write_file" => "Write".to_string(),
        "apply_patch" => "Edit".to_string(),
        "bash" => "Run".to_string(),
        "grep" => "Search".to_string(),
        "fs_browser" => "Browse".to_string(),
        other => other.replace('_', " "),
    }
}

fn render_compact_tool_line_with_width(
    invocation: &ToolInvocation,
    result: &ToolResult,
    chat_width: usize,
) -> Line<'static> {
    let marker = "◆ ";
    let marker_width = marker.chars().count();
    let text_width = chat_width.saturating_sub(marker_width).max(12);
    Line::from(vec![
        Span::styled(
            marker,
            Style::default().fg(if result.ok { Color::Green } else { Color::Red }),
        ),
        Span::styled(
            truncate_chars(&tool_compact_text(invocation, result), text_width),
            Style::default().fg(if result.ok { muted() } else { text() }),
        ),
    ])
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if display_width(value) <= max_chars {
        return value.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let mut text = value.chars().take(max_chars - 1).collect::<String>();
    text.push('…');
    text
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
    let mut code_highlighter: Option<CodeHighlighter> = None;
    let show_marker = marker_color != text_color || italic;

    let raw_lines = text.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < raw_lines.len() {
        let raw_line = raw_lines[index];
        let trimmed = raw_line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            let opening = !in_code;
            in_code = opening;
            language = if opening {
                rest.split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string()
            } else {
                String::new()
            };
            if opening {
                code_highlighter = Some(CodeHighlighter::new(&language));
            } else {
                code_highlighter = None;
            }
            lines.push(code_panel_padding_line(show_marker, marker_color));
            index += 1;
            continue;
        }

        if in_code {
            let bg = code_block_bg();
            let spans = if let Some(ref mut hl) = code_highlighter {
                hl.highlight_line(raw_line)
            } else {
                highlight_code_line(raw_line, &language)
            };
            let marker_width = if show_marker { 2 } else { 0 };
            let panel_prefix_width = code_panel_prefix_width();
            let content_width = max_width
                .saturating_sub(marker_width + panel_prefix_width + 1)
                .max(1);
            let wrapped = wrap_spans_to_width(&spans, content_width);
            let wrapped = if wrapped.is_empty() {
                vec![Vec::new()]
            } else {
                wrapped
            };
            for content_spans in wrapped {
                let mut line_spans = marker_spans(show_marker, marker_color);
                for span in &mut line_spans {
                    span.style = span.style.bg(bg);
                }
                line_spans.extend(code_panel_prefix_spans());
                for mut span in content_spans {
                    if span.style.bg.is_none() {
                        span.style = span.style.bg(bg);
                    }
                    line_spans.push(span);
                }
                lines.push(Line::from(line_spans).style(Style::default().bg(bg)));
            }
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
                        Style::default()
                            .fg(code_type())
                            .add_modifier(Modifier::BOLD),
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
        .map(|span| display_width(&span.content))
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
                Style::default()
                    .fg(code_type())
                    .add_modifier(Modifier::BOLD),
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
                Style::default()
                    .fg(code_type())
                    .add_modifier(Modifier::BOLD),
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

fn code_panel_padding_line(show_marker: bool, marker_color: Color) -> Line<'static> {
    let mut spans = marker_spans(show_marker, marker_color);
    let bg = code_block_bg();
    for span in &mut spans {
        span.style = span.style.bg(bg);
    }
    spans.extend(code_panel_prefix_spans());
    Line::from(spans).style(Style::default().bg(bg))
}

fn code_panel_prefix_spans() -> Vec<Span<'static>> {
    let bg = code_block_bg();
    vec![
        Span::styled("│", Style::default().fg(ghost()).bg(bg)),
        Span::styled("  ", Style::default().fg(text()).bg(bg)),
    ]
}

fn code_panel_prefix_width() -> usize {
    3
}

fn marker_spans(show_marker: bool, marker_color: Color) -> Vec<Span<'static>> {
    if show_marker {
        vec![Span::styled("│ ", Style::default().fg(marker_color))]
    } else {
        Vec::new()
    }
}
