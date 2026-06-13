use ratatui::prelude::{Line, Modifier, Span, Style};
use ratatui::style::Color;
use std::collections::{HashMap, HashSet};

use navi_sdk::{ToolInvocation, ToolResult};

use crate::state::{ChatLineSource, ChatMessage, ChatRole};
use crate::theme::*;

use super::syntax::{CodeHighlighter, highlight_code_line};
use super::text::{display_width, wrap_spans_to_width, wrap_text};
use super::tool::{tool_compact_text, tool_detail_block, tool_full_content};

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
    _compact_tool_visible_limit: usize,
    expanded_tool_results: &HashSet<String>,
    running_tools: &HashMap<String, ToolInvocation>,
    tool_render_cache: &mut HashMap<String, Vec<Line<'static>>>,
    loading_elapsed_ms: Option<u64>,
) -> ChatRenderOutput {
    let mut rendered_lines: Vec<Line<'static>> = Vec::new();
    let mut line_sources: Vec<ChatLineSource> = Vec::new();
    let mut index = 0;

    while index < messages.len() {
        let msg = &messages[index];
        if is_empty_tool_placeholder(msg) {
            if let Some(status) = msg.status.as_deref() {
                if status.starts_with("tool:") && !running_tools.is_empty() {
                    if !rendered_lines.is_empty() {
                        rendered_lines.push(Line::from(""));
                        line_sources.push(ChatLineSource::None);
                    }

                    for invocation in running_tools.values() {
                        rendered_lines.push(render_running_tool_line(
                            invocation,
                            chat_width,
                            loading_elapsed_ms,
                        ));
                        line_sources.push(ChatLineSource::Message(index));
                        push_min_card_lines(
                            &mut rendered_lines,
                            &mut line_sources,
                            ChatLineSource::Message(index),
                            chat_width,
                            interactive_bg(),
                            3,
                        );
                    }
                } else if matches!(status, "thinking" | "receiving")
                    || status.starts_with("approval:")
                    || status.starts_with("question")
                {
                    if !rendered_lines.is_empty() {
                        rendered_lines.push(Line::from(""));
                        line_sources.push(ChatLineSource::None);
                    }
                    rendered_lines.push(render_activity_line(
                        msg,
                        status,
                        running_tools,
                        chat_width,
                        loading_elapsed_ms,
                    ));
                    line_sources.push(ChatLineSource::Message(index));
                }
            }

            index += 1;
            continue;
        }
        if !full_tool_view && tool_result_parts(msg).is_some() {
            let Some((invocation, result)) = tool_result_parts(msg) else {
                index += 1;
                continue;
            };
            push_block_gap(&mut rendered_lines, &mut line_sources);
            let rendered_tool = render_compact_tool_result(
                invocation,
                result,
                chat_width,
                expanded_tool_results,
                tool_render_cache,
            );
            for (line, source) in rendered_tool {
                rendered_lines.push(line);
                line_sources.push(source);
            }
            index += 1;
            while index < messages.len() && is_transparent_tool_placeholder(&messages[index]) {
                index += 1;
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
                    if !show_thinking
                        && !msg.thinking_content.trim().is_empty()
                        && msg
                            .status
                            .as_deref()
                            .is_some_and(|status| status == "thinking")
                    {
                        rendered_lines.push(render_activity_line(
                            msg,
                            "thinking",
                            running_tools,
                            chat_width,
                            loading_elapsed_ms,
                        ));
                        line_sources.push(ChatLineSource::Message(index));
                        if msg.content.trim().is_empty() {
                            index += 1;
                            continue;
                        }
                        rendered_lines.push(Line::from(""));
                        line_sources.push(ChatLineSource::Message(index));
                    }
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

fn push_min_card_lines(
    lines: &mut Vec<Line<'static>>,
    sources: &mut Vec<ChatLineSource>,
    source: ChatLineSource,
    width: usize,
    bg: Color,
    count: usize,
) {
    for _ in 0..count {
        lines.push(blank_card_line(width, bg));
        sources.push(source.clone());
    }
}

fn blank_card_line(width: usize, bg: Color) -> Line<'static> {
    Line::from(vec![Span::styled(
        " ".repeat(width.max(1)),
        Style::default().fg(muted()).bg(bg),
    )])
}

fn render_running_tool_line(
    invocation: &ToolInvocation,
    chat_width: usize,
    loading_elapsed_ms: Option<u64>,
) -> Line<'static> {
    let action_color = tool_color(invocation.tool_name.as_str());
    let tool_label = tool_group_label(&invocation.tool_name);
    let mut detail = tool_compact_text(
        invocation,
        &ToolResult {
            invocation_id: invocation.id.clone(),
            ok: true,
            output: serde_json::json!({}),
        },
    );
    if let Some(rest) = detail.strip_prefix(&tool_label) {
        detail = rest.trim_start().to_string();
    }
    if detail.is_empty() {
        detail = running_tool_detail(invocation);
    }
    let elapsed = loading_elapsed_ms.map(format_elapsed).unwrap_or_default();
    let suffix = if elapsed.is_empty() {
        String::new()
    } else {
        format!(" · {elapsed}")
    };
    let content_width = chat_width.saturating_sub(6).max(12);
    let detail = truncate_chars(
        &detail,
        content_width.saturating_sub(tool_label.len() + suffix.len() + 2),
    );

    Line::from(vec![
        Span::styled("│ ", Style::default().fg(action_color)),
        Span::styled(
            "• ",
            Style::default()
                .fg(action_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{tool_label}:"),
            Style::default()
                .fg(action_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::default().fg(ghost())),
        Span::styled(detail, Style::default().fg(text())),
        Span::styled(suffix, Style::default().fg(code_number())),
    ])
}

fn render_activity_line(
    message: &ChatMessage,
    status: &str,
    running_tools: &HashMap<String, ToolInvocation>,
    chat_width: usize,
    loading_elapsed_ms: Option<u64>,
) -> Line<'static> {
    let (label, phase, color) = if status == "receiving" {
        ("Writing", "composing response".to_string(), accent())
    } else if status.starts_with("approval:") {
        (
            "Approval",
            status.trim_start_matches("approval: ").to_string(),
            code_const(),
        )
    } else if status.starts_with("question") {
        ("Question", "waiting for input".to_string(), code_const())
    } else if let Some(invocation) = running_tools.values().next() {
        (
            "Thinking",
            format!(
                "preparing {}",
                tool_group_label(&invocation.tool_name).to_lowercase()
            ),
            code_operator(),
        )
    } else {
        ("Thinking", thinking_phase(message), code_operator())
    };
    let elapsed = loading_elapsed_ms.map(format_elapsed).unwrap_or_default();
    let suffix = if elapsed.is_empty() {
        String::new()
    } else {
        format!(" · {elapsed}")
    };
    let phase = truncate_chars(
        &phase,
        chat_width
            .saturating_sub(label.len() + suffix.len() + 8)
            .max(12),
    );

    Line::from(vec![
        Span::styled("│ ", Style::default().fg(color)),
        Span::styled(
            "• ",
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{label}:"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::default().fg(ghost())),
        Span::styled(
            phase,
            Style::default().fg(text()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(suffix, Style::default().fg(code_number())),
    ])
}

fn running_tool_detail(invocation: &ToolInvocation) -> String {
    invocation
        .input
        .get("command")
        .or_else(|| invocation.input.get("path"))
        .or_else(|| invocation.input.get("query"))
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| "working".to_string())
}

fn thinking_phase(message: &ChatMessage) -> String {
    message
        .thinking_content
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(clean_activity_text)
        .filter(|line| !line.is_empty())
        .unwrap_or_else(|| "planning next step".to_string())
}

fn clean_activity_text(text: &str) -> String {
    text.trim_matches(|ch: char| matches!(ch, '#' | '*' | '-' | '`' | '>' | ' '))
        .trim()
        .to_string()
}

fn format_elapsed(ms: u64) -> String {
    let seconds = ms / 1_000;
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}

fn render_user_message_lines(text: &str, chat_width: usize) -> Vec<Line<'static>> {
    let width = chat_width.max(8);
    let wrapped = wrap_text(text, width.saturating_sub(4));
    let mut lines = wrapped
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let prefix = if index == 0 { "│ " } else { "  " };
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
        .collect::<Vec<_>>();
    while lines.len() < 4 {
        lines.push(blank_card_line(width, panel()));
    }
    lines
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

fn render_compact_tool_result(
    invocation: &ToolInvocation,
    result: &ToolResult,
    chat_width: usize,
    expanded_tool_results: &HashSet<String>,
    tool_render_cache: &mut HashMap<String, Vec<Line<'static>>>,
) -> Vec<(Line<'static>, ChatLineSource)> {
    let source = ChatLineSource::ToolResult(result.invocation_id.clone());
    let mut lines = Vec::new();

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
            source.clone(),
        ));
        if let Some(detail) = tool_detail_block(invocation, result) {
            let rendered =
                render_markdown_lines(&detail, chat_width.saturating_sub(2), text(), text(), false);
            for line in rendered {
                lines.push((line, source.clone()));
            }
        }
    }

    lines
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
    let marker = "";
    let marker_width = marker.chars().count();
    let text_width = chat_width.saturating_sub(marker_width).max(12);
    let status_color = if result.ok { accent() } else { red() };
    let label = truncate_chars(&tool_compact_text(invocation, result), text_width);
    let (action, detail) = label.split_once(' ').unwrap_or((&label, ""));
    let action_color = if result.ok {
        tool_color(invocation.tool_name.as_str())
    } else {
        red()
    };
    let mut spans = vec![
        Span::styled(marker, Style::default().fg(status_color)),
        Span::styled(
            tool_line_prefix(invocation, result),
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            action.to_string(),
            Style::default()
                .fg(action_color)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if !detail.is_empty() {
        spans.push(Span::styled(" ", Style::default().fg(ghost())));
        spans.extend(semantic_plain_spans(
            detail,
            if result.ok { muted() } else { text() },
        ));
    }
    Line::from(spans)
}

fn tool_line_prefix(invocation: &ToolInvocation, result: &ToolResult) -> &'static str {
    if !result.ok {
        return "✗ ";
    }
    if invocation.tool_name == "grep" {
        "* "
    } else {
        "→ "
    }
}

fn tool_color(tool_name: &str) -> Color {
    match tool_name {
        "read_file" | "view_file" | "grep" | "fs_browser" => code_type(),
        "write_file" | "apply_patch" => code_const(),
        "bash" => code_operator(),
        _ => accent(),
    }
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
            let spans = if is_diff_language(&language) {
                diff_line_spans(raw_line)
            } else if language.is_empty() {
                terminal_output_spans(raw_line)
            } else if let Some(ref mut hl) = code_highlighter {
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
            if recursive && contains_inline_markup(content) {
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

fn contains_inline_markup(text: &str) -> bool {
    text.contains('`')
        || text.contains('*')
        || text.contains('_')
        || text.contains("~~")
        || text.contains("](")
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
    spans.extend(semantic_plain_spans(&std::mem::take(plain), fallback));
}

fn semantic_plain_spans(text: &str, fallback: Color) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut token = String::new();
    let mut token_is_whitespace = None;

    for ch in text.chars() {
        let is_whitespace = ch.is_whitespace();
        if token_is_whitespace == Some(is_whitespace) || token.is_empty() {
            token.push(ch);
            token_is_whitespace = Some(is_whitespace);
            continue;
        }
        push_semantic_token(
            &mut spans,
            std::mem::take(&mut token),
            token_is_whitespace,
            fallback,
        );
        token.push(ch);
        token_is_whitespace = Some(is_whitespace);
    }
    push_semantic_token(&mut spans, token, token_is_whitespace, fallback);
    spans
}

fn push_semantic_token(
    spans: &mut Vec<Span<'static>>,
    token: String,
    token_is_whitespace: Option<bool>,
    fallback: Color,
) {
    if token.is_empty() {
        return;
    }
    if token_is_whitespace == Some(true) {
        spans.push(Span::styled(token, Style::default().fg(fallback)));
        return;
    }
    spans.push(Span::styled(
        token.clone(),
        semantic_token_style(&token, fallback),
    ));
}

fn semantic_token_style(token: &str, fallback: Color) -> Style {
    let core = token.trim_matches(|ch: char| {
        matches!(
            ch,
            ',' | '.' | ':' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\''
        )
    });
    let lower = core.to_ascii_lowercase();

    if matches!(
        lower.as_str(),
        "error" | "errors" | "failed" | "failure" | "panic" | "denied"
    ) || is_rust_error_code(core)
    {
        return Style::default().fg(red()).add_modifier(Modifier::BOLD);
    }
    if matches!(lower.as_str(), "warning" | "warnings" | "warn") {
        return Style::default()
            .fg(code_const())
            .add_modifier(Modifier::BOLD);
    }
    if matches!(
        lower.as_str(),
        "ok" | "success" | "successful" | "successfully" | "finished" | "completed" | "ready"
    ) {
        return Style::default().fg(accent()).add_modifier(Modifier::BOLD);
    }
    if matches!(
        core,
        "Checking" | "Compiling" | "Running" | "Finished" | "Command" | "Stdout" | "Stderr"
    ) {
        return Style::default()
            .fg(code_func())
            .add_modifier(Modifier::BOLD);
    }
    if is_likely_path(core) {
        return Style::default().fg(code_type());
    }
    if core.starts_with("navi-") || core.starts_with("NAVI") {
        return Style::default()
            .fg(code_string())
            .add_modifier(Modifier::BOLD);
    }
    if core.starts_with('-') && core.len() > 1 {
        return Style::default().fg(code_operator());
    }
    if is_command_name(core) {
        return Style::default()
            .fg(code_func())
            .add_modifier(Modifier::BOLD);
    }
    if is_number_like(core) || core.starts_with('v') && is_number_like(&core[1..]) {
        return Style::default().fg(code_number());
    }
    if is_env_like(core) {
        return Style::default()
            .fg(code_const())
            .add_modifier(Modifier::BOLD);
    }
    Style::default().fg(fallback)
}

fn terminal_output_spans(raw_line: &str) -> Vec<Span<'static>> {
    let bg = code_block_bg();
    semantic_plain_spans(raw_line, text())
        .into_iter()
        .map(|mut span| {
            span.style = span.style.bg(bg);
            span
        })
        .collect()
}

fn is_diff_language(language: &str) -> bool {
    matches!(language, "diff" | "patch")
}

fn diff_line_spans(raw_line: &str) -> Vec<Span<'static>> {
    let (bg, marker_color, content_color, bold) = if raw_line.starts_with("@@") {
        (diff_hunk_bg(), code_const(), code_const(), true)
    } else if raw_line.starts_with("diff ") || raw_line.starts_with("index ") {
        (diff_meta_bg(), code_func(), code_func(), true)
    } else if raw_line.starts_with("+++") || raw_line.starts_with("---") {
        (diff_meta_bg(), code_type(), code_type(), true)
    } else if raw_line.starts_with('+') {
        (diff_add_bg(), accent(), text(), false)
    } else if raw_line.starts_with('-') {
        (diff_remove_bg(), red(), text(), false)
    } else {
        (code_block_bg(), ghost(), text(), false)
    };

    if let Some((marker, rest)) = diff_marker_and_rest(raw_line) {
        let mut spans = vec![Span::styled(
            marker.to_string(),
            Style::default()
                .fg(marker_color)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        )];
        spans.extend(highlight_diff_code(rest, content_color, bg));
        return spans;
    }

    semantic_plain_spans(raw_line, content_color)
        .into_iter()
        .map(|mut span| {
            span.style = span.style.bg(bg);
            if bold {
                span.style = span.style.add_modifier(Modifier::BOLD);
            }
            span
        })
        .collect()
}

fn diff_marker_and_rest(raw_line: &str) -> Option<(char, &str)> {
    if raw_line.starts_with("+++") || raw_line.starts_with("---") {
        return None;
    }
    let marker = raw_line.chars().next()?;
    if matches!(marker, '+' | '-' | ' ') {
        Some((marker, &raw_line[marker.len_utf8()..]))
    } else {
        None
    }
}

fn highlight_diff_code(rest: &str, fallback: Color, bg: Color) -> Vec<Span<'static>> {
    let highlighted = highlight_code_line(rest, "rust");
    let mut spans = if highlighted.len() == 1 && highlighted[0].style.fg == Some(text()) {
        semantic_plain_spans(rest, fallback)
    } else {
        highlighted
    };
    for span in &mut spans {
        span.style = span.style.bg(bg);
        if span.style.fg.is_none() {
            span.style = span.style.fg(fallback);
        }
    }
    spans
}

fn diff_add_bg() -> Color {
    Color::Rgb(18, 49, 43)
}

fn diff_remove_bg() -> Color {
    Color::Rgb(54, 31, 43)
}

fn diff_hunk_bg() -> Color {
    Color::Rgb(35, 41, 64)
}

fn diff_meta_bg() -> Color {
    Color::Rgb(27, 34, 48)
}

fn is_likely_path(token: &str) -> bool {
    token.contains('/')
        || token.starts_with("~/")
        || [
            ".rs", ".toml", ".json", ".md", ".lock", ".yaml", ".yml", ".tsx", ".ts", ".js",
        ]
        .iter()
        .any(|suffix| token.ends_with(suffix))
}

fn is_rust_error_code(token: &str) -> bool {
    token.len() == 5 && token.starts_with('E') && token[1..].chars().all(|ch| ch.is_ascii_digit())
}

fn is_command_name(token: &str) -> bool {
    matches!(
        token,
        "cargo" | "just" | "git" | "npm" | "pnpm" | "bun" | "node" | "rustc" | "rg"
    )
}

fn is_number_like(token: &str) -> bool {
    let trimmed = token.trim_end_matches(|ch: char| matches!(ch, '%' | 'K' | 'M' | 's' | 'm'));
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, '.' | '_' | '/'))
        && trimmed.chars().any(|ch| ch.is_ascii_digit())
}

fn is_env_like(token: &str) -> bool {
    token.len() > 3
        && token.contains('_')
        && token
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
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
