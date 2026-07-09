use ratatui::prelude::{Line, Modifier, Span, Style};
use ratatui::style::Color;
use std::collections::{HashMap, HashSet};

use navi_sdk::{ToolInvocation, ToolResult};

use crate::state::{ChatLineSource, ChatMessage, ChatRole};
use crate::theme::*;

use super::syntax::{CodeHighlighter, highlight_code_line};
use super::text::{display_width, wrap_inline_spans_to_width, wrap_spans_to_width, wrap_text};
use super::tool::tool_compact_text;

const USER_MESSAGE_VERTICAL_PADDING: usize = 0;

/// Blank line between structural markdown blocks (prose ↔ table/code/heading).
/// Mirrors Grok's outer_vpad / block breathing room.
const MD_BLOCK_V_GAP: usize = 1;
/// Shared content column: user text (after `› `), tools (`◆ `), assistant prose.
/// Keeps scrollback on two visual columns — gutter | content — like Grok.
const CONTENT_GUTTER: usize = 2;
/// Left pad for structural blocks so tables/code align under tool diamonds (`◆ `).
const MD_BLOCK_H_PAD: usize = CONTENT_GUTTER;
/// Inner horizontal pad inside each table cell.
const TABLE_CELL_H_PAD: usize = 1;
/// Width of the dim column separator (` │ `).
const TABLE_COL_SEP_WIDTH: usize = 3;
const TABLE_COL_SEP: &str = " │ ";

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
    collapsed_tool_results: &HashSet<String>,
    running_tools: &HashMap<String, ToolInvocation>,
    subagent_activity: &HashMap<String, String>,
    tool_render_cache: &mut HashMap<String, Vec<Line<'static>>>,
    loading_elapsed_ms: Option<u64>,
) -> ChatRenderOutput {
    let mut rendered_lines: Vec<Line<'static>> = Vec::new();
    let mut line_sources: Vec<ChatLineSource> = Vec::new();
    let mut index = 0;

    while index < messages.len() {
        let msg = &messages[index];
        if is_empty_tool_placeholder(msg) {
            index += 1;
            continue;
        }
        // Unified tool path: compact header + optional body (auto / user / expand-all).
        if tool_result_parts(msg).is_some() {
            let Some((invocation, result)) = tool_result_parts(msg) else {
                index += 1;
                continue;
            };
            push_block_gap(&mut rendered_lines, &mut line_sources);
            let rendered_tool = render_compact_tool_result(
                invocation,
                result,
                chat_width,
                full_tool_view,
                expanded_tool_results,
                collapsed_tool_results,
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
                // Grok-style sticky prompt: `› text…` left, clock right-aligned.
                let lines = render_user_message_lines(&msg.content, chat_width, msg.sent_at);
                for line in lines {
                    rendered_lines.push(line);
                    line_sources.push(ChatLineSource::Message(index));
                }
                // Only append chips when the stored text has no tags (legacy sessions).
                if !msg.content.contains("[Image ") && !user_image_labels(msg).is_empty() {
                    for (image_index, label) in user_image_labels(msg).iter().enumerate() {
                        let tag = if label.starts_with("[Image ") {
                            label.clone()
                        } else {
                            format!("[Image {}]", image_index + 1)
                        };
                        rendered_lines.push(user_image_tag_line(&tag, chat_width));
                        line_sources.push(ChatLineSource::Message(index));
                    }
                }
            }
            ChatRole::Assistant => {
                if msg.is_recap {
                    rendered_lines.push(Line::from(vec![
                        Span::styled(
                            " ◈ recap ",
                            Style::default().fg(signal()).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            "─".repeat(chat_width.saturating_sub(10)),
                            Style::default().fg(ghost()),
                        ),
                    ]));
                    line_sources.push(ChatLineSource::Message(index));
                    for line in wrap_text(&msg.content, chat_width.saturating_sub(2)) {
                        rendered_lines.push(Line::from(vec![Span::styled(
                            format!("  {line}"),
                            Style::default().fg(muted()),
                        )]));
                        line_sources.push(ChatLineSource::Message(index));
                    }
                    continue;
                }
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
                if !show_thinking
                    && !msg.thinking_content.trim().is_empty()
                    && msg
                        .status
                        .as_deref()
                        .is_some_and(|status| status == "thinking")
                {
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
                // Assistant prose/tables/code all start at the shared content
                // column (CONTENT_GUTTER) — same as user text after `› ` and
                // tool labels after `◆ `.
                let assistant_lines = render_markdown_lines(
                    &msg.content,
                    chat_width,
                    text(),
                    text(),
                    false,
                );
                push_sourced_lines(
                    &mut rendered_lines,
                    &mut line_sources,
                    assistant_lines,
                    ChatLineSource::Message(index),
                );
            }
        }
        index += 1;
    }
    push_running_subagents(
        &mut rendered_lines,
        &mut line_sources,
        running_tools,
        subagent_activity,
        chat_width,
        loading_elapsed_ms,
    );
    ChatRenderOutput {
        lines: rendered_lines,
        sources: line_sources,
    }
}

fn user_image_tag_line(tag: &str, chat_width: usize) -> Line<'static> {
    // Align under user prompt content column (`› ` → two spaces).
    let width = chat_width.max(8);
    let mut spans = vec![
        Span::styled(
            " ".repeat(CONTENT_GUTTER),
            Style::default().fg(ghost()).bg(panel()),
        ),
        Span::styled(
            tag.to_string(),
            Style::default().bg(code_const()).fg(Color::Black),
        ),
    ];
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
}

fn user_image_labels(msg: &ChatMessage) -> Vec<String> {
    if !msg.image_labels.is_empty() {
        return msg.image_labels.clone();
    }
    msg.images
        .iter()
        .map(|image| format!("[Image {}]", image.index.max(1)))
        .collect()
}

/// True when `text` is exactly an `[Image N]` chip (optionally padded).
pub(crate) fn is_image_tag(text: &str) -> bool {
    let trimmed = text.trim();
    let bytes = trimmed.as_bytes();
    if !bytes.starts_with(b"[Image ") || !bytes.ends_with(b"]") || bytes.len() < 9 {
        return false;
    }
    let digits = &bytes[7..bytes.len() - 1];
    !digits.is_empty() && digits.iter().all(|b| b.is_ascii_digit())
}

/// Parse 1-based image index from an `[Image N]` tag.
pub(crate) fn parse_image_tag_index(text: &str) -> Option<usize> {
    let trimmed = text.trim();
    let inner = trimmed.strip_prefix("[Image ")?.strip_suffix(']')?;
    let index = inner.parse::<usize>().ok()?;
    (index >= 1).then_some(index)
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

/// Grok-style user prompt row:
/// ```text
/// › message text that wraps…                         4:25 AM
///   continued on next line without the clock
/// ```
///
/// Two columns: prefix+text on the left, wall-clock on the right of the first line.
fn render_user_message_lines(
    text: &str,
    chat_width: usize,
    sent_at: Option<std::time::SystemTime>,
) -> Vec<Line<'static>> {
    let width = chat_width.max(16);
    let prefix = "› ";
    let prefix_w = display_width(prefix);
    let time_label = format_message_clock(sent_at);
    let time_w = if time_label.is_empty() {
        0
    } else {
        display_width(&time_label).saturating_add(1) // leading space
    };
    let content_width = width
        .saturating_sub(prefix_w)
        .saturating_sub(time_w)
        .max(8);

    let display = text.trim();
    if display.is_empty() {
        return Vec::new();
    }
    let wrapped = wrap_text(display, content_width);
    let mut lines = Vec::new();

    for _ in 0..USER_MESSAGE_VERTICAL_PADDING {
        lines.push(user_blank_card_line(width));
    }

    for (i, line) in wrapped.into_iter().enumerate() {
        let is_first = i == 0;
        let mut spans = vec![
            Span::styled(
                if is_first {
                    prefix.to_string()
                } else {
                    " ".repeat(prefix_w)
                },
                Style::default()
                    .fg(if is_first { user_accent() } else { ghost() })
                    .bg(panel())
                    .add_modifier(if is_first {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
        ];
        spans.extend(
            style_user_line_with_image_tags(&line, text_color_for_user())
                .into_iter()
                .map(|mut s| {
                    if s.style.bg.is_none() {
                        s.style = s.style.bg(panel());
                    }
                    s
                }),
        );

        let used: usize = spans.iter().map(|s| display_width(&s.content)).sum();
        let clock_w = if is_first {
            display_width(&time_label)
        } else {
            0
        };
        let pad = width.saturating_sub(used).saturating_sub(clock_w);
        if pad > 0 {
            spans.push(Span::styled(
                " ".repeat(pad),
                Style::default().fg(muted()).bg(panel()),
            ));
        }
        if is_first && !time_label.is_empty() {
            spans.push(Span::styled(
                time_label.clone(),
                Style::default().fg(ghost()).bg(panel()),
            ));
        }
        // Ensure full-width sticky bar.
        let used2: usize = spans.iter().map(|s| display_width(&s.content)).sum();
        if used2 < width {
            spans.push(Span::styled(
                " ".repeat(width - used2),
                Style::default().bg(panel()),
            ));
        }
        lines.push(Line::from(spans));
    }

    for _ in 0..USER_MESSAGE_VERTICAL_PADDING {
        lines.push(user_blank_card_line(width));
    }
    lines
}

/// Format wall clock like Grok: `4:25 AM`. Empty if unknown.
fn format_message_clock(sent_at: Option<std::time::SystemTime>) -> String {
    let Some(sent_at) = sent_at else {
        return String::new();
    };
    let Ok(duration) = sent_at.duration_since(std::time::UNIX_EPOCH) else {
        return String::new();
    };
    let Ok(odt) = time::OffsetDateTime::from_unix_timestamp(duration.as_secs() as i64) else {
        return String::new();
    };
    let local = time::UtcOffset::current_local_offset()
        .map(|offset| odt.to_offset(offset))
        .unwrap_or(odt);
    let hour24 = local.hour();
    let minute = local.minute();
    let (hour12, ampm) = match hour24 {
        0 => (12, "AM"),
        1..=11 => (hour24, "AM"),
        12 => (12, "PM"),
        _ => (hour24 - 12, "PM"),
    };
    format!("{hour12}:{minute:02} {ampm}")
}

/// Style user prose while keeping `[Image N]` chips as solid highlighted spans
/// (same look as the composer input).
fn style_user_line_with_image_tags(line: &str, fallback: Color) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut idx = 0usize;
    while idx < line.len() {
        let rest = &line[idx..];
        let rest_bytes = rest.as_bytes();
        if rest_bytes.starts_with(b"[Image ") {
            let mut check_idx = 7;
            let mut has_digits = false;
            while check_idx < rest_bytes.len() && rest_bytes[check_idx].is_ascii_digit() {
                has_digits = true;
                check_idx += 1;
            }
            if has_digits && check_idx < rest_bytes.len() && rest_bytes[check_idx] == b']' {
                let tag_end = idx + check_idx + 1;
                let tag = &line[idx..tag_end];
                spans.push(Span::styled(
                    tag.to_string(),
                    Style::default().bg(code_const()).fg(Color::Black),
                ));
                idx = tag_end;
                continue;
            }
        }

        // Take the next run of non-tag text as one unit for markdown inline styling.
        let next_tag = rest.find("[Image ").unwrap_or(rest.len());
        let chunk = &rest[..next_tag];
        if !chunk.is_empty() {
            spans.extend(
                inline_text_spans(chunk, fallback)
                    .into_iter()
                    .map(|mut span| {
                        if span.style.bg.is_none() {
                            span.style = span.style.bg(panel());
                        }
                        span
                    }),
            );
            idx += chunk.len();
        } else {
            // Avoid infinite loop on a bare '[' that is not an image tag.
            let ch = rest.chars().next().unwrap_or(' ');
            spans.push(Span::styled(
                ch.to_string(),
                Style::default().fg(fallback).bg(panel()),
            ));
            idx += ch.len_utf8();
        }
    }
    spans
}

/// Full-width blank strip of the user sticky bar (panel background).
fn user_blank_card_line(width: usize) -> Line<'static> {
    Line::from(Span::styled(
        " ".repeat(width.max(1)),
        Style::default().fg(muted()).bg(panel()),
    ))
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

fn push_running_subagents(
    rendered_lines: &mut Vec<Line<'static>>,
    line_sources: &mut Vec<ChatLineSource>,
    running_tools: &HashMap<String, ToolInvocation>,
    subagent_activity: &HashMap<String, String>,
    chat_width: usize,
    loading_elapsed_ms: Option<u64>,
) {
    let mut subagents = running_tools
        .values()
        .filter(|invocation| invocation.tool_name == "subagent")
        .collect::<Vec<_>>();
    subagents.sort_by(|left, right| left.id.cmp(&right.id));

    for invocation in subagents {
        push_block_gap(rendered_lines, line_sources);
        let task = subagent_task_label(invocation);
        let detail = subagent_activity
            .get(&invocation.id)
            .cloned()
            .unwrap_or_else(|| subagent_detail_label(invocation, &task));
        let spinner = super::status::running_diamond_prefix(loading_elapsed_ms.unwrap_or_default());
        let width = chat_width.max(12);
        let label_width = width.saturating_sub(display_width(spinner) + 3).max(8);
        let detail_width = width.saturating_sub(4).max(8);

        rendered_lines.push(Line::from(vec![
            Span::styled(
                spinner.to_string(),
                Style::default()
                    .fg(code_operator())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Subagent Task ", Style::default().fg(muted())),
            Span::styled("— ", Style::default().fg(ghost())),
            Span::styled(
                truncate_chars(&task, label_width),
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
        ]));
        line_sources.push(ChatLineSource::Subagent(invocation.id.clone()));

        if !detail.is_empty() {
            // Indent under the diamond — no vertical trail/corner stroke.
            rendered_lines.push(Line::from(vec![
                Span::styled("  ↳ ", Style::default().fg(ghost())),
                Span::styled(
                    truncate_chars(&detail, detail_width),
                    Style::default().fg(muted()),
                ),
            ]));
            line_sources.push(ChatLineSource::Subagent(invocation.id.clone()));
        }
    }
}

fn subagent_task_label(invocation: &ToolInvocation) -> String {
    invocation
        .input
        .get("description")
        .and_then(|value| value.as_str())
        .or_else(|| {
            invocation
                .input
                .get("prompt")
                .and_then(|value| value.as_str())
        })
        .map(one_line)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Working".to_string())
}

fn subagent_detail_label(invocation: &ToolInvocation, task: &str) -> String {
    let prompt = invocation
        .input
        .get("prompt")
        .and_then(|value| value.as_str())
        .map(one_line)
        .unwrap_or_default();
    if prompt.is_empty() || prompt == task {
        invocation
            .input
            .get("profile")
            .and_then(|value| value.as_str())
            .map(|profile| format!("Profile {profile}"))
            .unwrap_or_default()
    } else {
        prompt
    }
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn render_compact_tool_result(
    invocation: &ToolInvocation,
    result: &ToolResult,
    chat_width: usize,
    full_tool_view: bool,
    expanded_tool_results: &HashSet<String>,
    collapsed_tool_results: &HashSet<String>,
    tool_render_cache: &mut HashMap<String, Vec<Line<'static>>>,
) -> Vec<(Line<'static>, ChatLineSource)> {
    use super::tool_policy::{tool_auto_expand, tool_body_reason, tool_body_visible, ToolBodyReason};

    let source = if invocation.tool_name == "subagent" {
        ChatLineSource::Subagent(result.invocation_id.clone())
    } else {
        ChatLineSource::ToolResult(result.invocation_id.clone())
    };
    let mut lines = Vec::new();

    let show_body = tool_body_visible(
        invocation,
        result,
        full_tool_view,
        expanded_tool_results,
        collapsed_tool_results,
    );
    let reason = tool_body_reason(
        invocation,
        result,
        full_tool_view,
        expanded_tool_results,
        collapsed_tool_results,
    );

    // Always show the one-line header (Grok tool card title).
    lines.push((
        render_compact_tool_line_with_width(invocation, result, chat_width),
        source.clone(),
    ));

    if show_body {
        let hint = match reason {
            ToolBodyReason::AutoUseful => "auto · click to collapse",
            ToolBodyReason::ExpandAll => "expand-all · click to collapse",
            ToolBodyReason::UserExpanded => "click to collapse",
            _ => "click to collapse",
        };
        lines.push((
            Line::from(Span::styled(
                format!("  {hint}"),
                Style::default().fg(ghost()),
            )),
            source.clone(),
        ));
        let cache_key = format!(
            "{}|{}|{}",
            result.invocation_id,
            full_tool_view,
            tool_auto_expand(invocation, result)
        );
        let rendered = if let Some(cached) = tool_render_cache.get(&cache_key) {
            cached.clone()
        } else {
            let body = super::tool::tool_body_content(invocation, result);
            // Full width: markdown applies CONTENT_GUTTER itself so body
            // text lines up under the tool diamond prefix.
            let rendered = render_markdown_lines(&body, chat_width, text(), text(), false);
            tool_render_cache.insert(cache_key, rendered.clone());
            rendered
        };
        for line in rendered {
            lines.push((line, source.clone()));
        }
    }

    lines
}

fn render_compact_tool_line_with_width(
    invocation: &ToolInvocation,
    result: &ToolResult,
    chat_width: usize,
) -> Line<'static> {
    // Grok-style diamond bullet only — no left quote-bar / corner stroke.
    let prefix = super::status::settled_diamond_prefix(result.ok);
    let text_width = chat_width.saturating_sub(display_width(prefix)).max(12);
    let status_color = super::status::settled_diamond_color(result.ok, Color::Green, red());
    let label = truncate_chars(&tool_compact_text(invocation, result), text_width);
    let (action, detail) = label.split_once(' ').unwrap_or((&label, ""));
    let action_color = if result.ok {
        tool_color(invocation.tool_name.as_str())
    } else {
        red()
    };
    let mut spans = vec![
        Span::styled(
            prefix,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MdBlockKind {
    None,
    Prose,
    Heading,
    List,
    Table,
    Code,
    Rule,
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
    // Diamond only on the first marked line; later lines indent without a trail.
    let mut lead_marker = show_marker;
    let mut last_block = MdBlockKind::None;

    let raw_lines = text.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < raw_lines.len() {
        let raw_line = raw_lines[index];
        let trimmed = raw_line.trim_start();

        if trimmed.is_empty() && !in_code {
            // Preserve author blank lines, but collapse runs so we don't double
            // the structural gaps we insert ourselves.
            if last_block != MdBlockKind::None && !line_is_blank(lines.last()) {
                lines.push(Line::from(""));
            }
            last_block = MdBlockKind::None;
            index += 1;
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("```") {
            let opening = !in_code;
            if opening {
                ensure_md_block_gap(&mut lines, last_block, MdBlockKind::Code);
            }
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
            let is_first = take_lead_marker(&mut lead_marker, show_marker);
            lines.push(code_panel_padding_line_at(
                show_marker,
                marker_color,
                is_first,
            ));
            last_block = MdBlockKind::Code;
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
            let marker_width = if show_marker { 2 } else { CONTENT_GUTTER };
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
                let is_first = take_lead_marker(&mut lead_marker, show_marker);
                let mut line_spans = marker_spans_at(show_marker, marker_color, is_first);
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
            last_block = MdBlockKind::Code;
            index += 1;
            continue;
        }

        if is_thematic_break(trimmed) {
            ensure_md_block_gap(&mut lines, last_block, MdBlockKind::Rule);
            let is_first = take_lead_marker(&mut lead_marker, show_marker);
            lines.push(thematic_break_line_at(
                show_marker,
                marker_color,
                max_width,
                is_first,
            ));
            last_block = MdBlockKind::Rule;
            index += 1;
            continue;
        }

        if is_table_line(trimmed) {
            ensure_md_block_gap(&mut lines, last_block, MdBlockKind::Table);
            let mut table_rows = Vec::new();
            while index < raw_lines.len() && is_table_line(raw_lines[index].trim_start()) {
                let table_line = raw_lines[index].trim_start();
                if !is_table_separator(table_line) {
                    table_rows.push(table_line.to_string());
                }
                index += 1;
            }
            lines.extend(table_block_lines_at(
                &table_rows,
                show_marker,
                marker_color,
                max_width,
                &mut lead_marker,
            ));
            last_block = MdBlockKind::Table;
            continue;
        }

        let block = classify_prose_line(trimmed);
        ensure_md_block_gap(&mut lines, last_block, block);

        // Reserve shared content gutter even without a diamond marker so prose
        // lines up with tables/tools/user prompts (`› ` / `◆ ` = 2 cols).
        let gutter = if show_marker { 2 } else { CONTENT_GUTTER };
        let content_width = max_width.saturating_sub(gutter).max(1);
        let wrapped = wrap_text(raw_line, content_width);
        for line in wrapped {
            let is_first = take_lead_marker(&mut lead_marker, show_marker);
            lines.push(text_line_at(
                line,
                show_marker,
                marker_color,
                text_color,
                italic,
                is_first,
            ));
        }
        last_block = block;
        index += 1;
    }

    if text.is_empty() {
        let is_first = take_lead_marker(&mut lead_marker, show_marker);
        lines.push(text_line_at(
            String::new(),
            show_marker,
            marker_color,
            text_color,
            italic,
            is_first,
        ));
    }

    lines
}

fn classify_prose_line(trimmed: &str) -> MdBlockKind {
    let heading = trimmed.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&heading) && trimmed.chars().nth(heading) == Some(' ') {
        return MdBlockKind::Heading;
    }
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") || ordered_list_marker(trimmed).is_some()
    {
        return MdBlockKind::List;
    }
    if trimmed.starts_with("> ") {
        return MdBlockKind::Prose;
    }
    MdBlockKind::Prose
}

fn needs_md_block_gap(prev: MdBlockKind, next: MdBlockKind) -> bool {
    if prev == MdBlockKind::None || next == MdBlockKind::None {
        return false;
    }
    if prev == next {
        // Keep list items / plain prose tight; gap stacked tables, code fences, rules, headings.
        return matches!(
            next,
            MdBlockKind::Table | MdBlockKind::Code | MdBlockKind::Rule | MdBlockKind::Heading
        );
    }
    // Breathe between different structural kinds (prose↔table, list↔code, …).
    match (prev, next) {
        (MdBlockKind::List, MdBlockKind::List) => false,
        (MdBlockKind::Prose, MdBlockKind::Prose) => false,
        _ => true,
    }
}

fn ensure_md_block_gap(lines: &mut Vec<Line<'static>>, prev: MdBlockKind, next: MdBlockKind) {
    if MD_BLOCK_V_GAP == 0 || !needs_md_block_gap(prev, next) {
        return;
    }
    if line_is_blank(lines.last()) {
        return;
    }
    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
}

fn line_is_blank(line: Option<&Line<'static>>) -> bool {
    let Some(line) = line else {
        return true;
    };
    line.spans
        .iter()
        .all(|span| span.content.chars().all(|ch| ch.is_whitespace()))
}

fn take_lead_marker(lead_marker: &mut bool, show_marker: bool) -> bool {
    if !show_marker {
        return false;
    }
    if *lead_marker {
        *lead_marker = false;
        true
    } else {
        false
    }
}

fn text_line_at(
    text: String,
    show_marker: bool,
    marker_color: Color,
    text_color: Color,
    italic: bool,
    is_first_line: bool,
) -> Line<'static> {
    let mut spans = marker_spans_at(show_marker, marker_color, is_first_line);
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
        // Diamond family for headings (no vertical quote-bar).
        let prefix = match heading {
            1 | 2 | 3 => format!("{} ", super::status::DIAMOND),
            _ => format!("{} ", super::status::DIAMOND_HOLLOW),
        };
        spans.push(Span::styled(
            prefix,
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
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
        // Indent quote body — no continuous left rail.
        spans.push(Span::styled(
            format!("{} ", super::status::DIAMOND_HOLLOW),
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ));
        spans.extend(inline_text_spans(rest, muted()));
        return Some(spans);
    }

    if trimmed.starts_with('|') && trimmed.ends_with('|') {
        spans.extend(table_row_spans(&table_cells(trimmed), &[]));
        return Some(spans);
    }

    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        // Hollow diamond list bullet — same family as status diamonds, no trail.
        spans.push(Span::styled(
            format!("{} ", super::status::DIAMOND_HOLLOW),
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ));
        spans.extend(inline_text_spans(&trimmed[2..], fallback));
        return Some(spans);
    }

    if let Some((marker, rest)) = ordered_list_marker(trimmed) {
        spans.push(Span::styled(
            marker,
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
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

fn table_block_lines_at(
    table_rows: &[String],
    show_marker: bool,
    marker_color: Color,
    max_width: usize,
    lead_marker: &mut bool,
) -> Vec<Line<'static>> {
    const MAX_TABLE_WIDTH: usize = 140;
    const MIN_COLUMN_WIDTH: usize = 6;

    let rows = table_rows
        .iter()
        .map(|row| table_cells(row))
        .collect::<Vec<_>>();
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    // Content width before cell pad; pad is added to the column budget later.
    let mut content_widths = vec![0; column_count];
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            content_widths[index] = content_widths[index].max(rendered_inline_width(cell));
        }
    }

    // Align tables under tool diamonds: marker (`◆ `) or MD_BLOCK_H_PAD spaces.
    let gutter_width = if show_marker { 2 } else { MD_BLOCK_H_PAD };
    let available_width = max_width.saturating_sub(gutter_width).min(MAX_TABLE_WIDTH);
    let separators_width = content_widths
        .len()
        .saturating_sub(1)
        .saturating_mul(TABLE_COL_SEP_WIDTH);
    // Each column needs room for inner cell pad on both sides.
    let pad_budget = content_widths
        .len()
        .saturating_mul(TABLE_CELL_H_PAD.saturating_mul(2));
    let columns_width = available_width
        .saturating_sub(separators_width)
        .saturating_sub(pad_budget);
    let minimum_widths = rows
        .first()
        .map(|headers| {
            (0..column_count)
                .map(|index| {
                    headers
                        .get(index)
                        .map(|header| rendered_inline_width(header))
                        .unwrap_or(0)
                        .max(MIN_COLUMN_WIDTH)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let natural_sum: usize = content_widths.iter().sum();
    // Stack when the grid would be too cramped: either min widths don't fit, or
    // natural content is much wider than the budget (avoids unreadable thin columns).
    let should_stack = rows.len() > 1
        && (columns_width < minimum_widths.iter().sum::<usize>()
            || natural_sum > columns_width.saturating_mul(3) / 2);
    if should_stack {
        return stacked_table_lines_at(&rows, show_marker, marker_color, max_width, lead_marker);
    }

    shrink_table_widths(&mut content_widths, &minimum_widths, columns_width);

    let mut lines = Vec::new();
    for (row_index, cells) in rows.iter().enumerate() {
        lines.extend(wrapped_table_row_lines_at(
            cells,
            &content_widths,
            row_index == 0,
            show_marker,
            marker_color,
            lead_marker,
        ));
        if row_index == 0 {
            let is_first = take_lead_marker(lead_marker, show_marker);
            lines.push(table_header_rule_at(
                &content_widths,
                show_marker,
                marker_color,
                is_first,
            ));
        }
    }
    lines
}

fn shrink_table_widths(widths: &mut [usize], minimum_widths: &[usize], target: usize) {
    while widths.iter().sum::<usize>() > target {
        let Some((index, _)) = widths
            .iter()
            .enumerate()
            .filter(|(index, width)| **width > minimum_widths.get(*index).copied().unwrap_or(1))
            .max_by_key(|(index, width)| {
                width.saturating_sub(minimum_widths.get(*index).copied().unwrap_or(1))
            })
        else {
            break;
        };
        widths[index] = widths[index].saturating_sub(1);
    }
}

fn wrapped_table_row_lines_at(
    cells: &[String],
    widths: &[usize],
    header: bool,
    show_marker: bool,
    marker_color: Color,
    lead_marker: &mut bool,
) -> Vec<Line<'static>> {
    let color = if header { accent() } else { text() };
    let cell_pad = " ".repeat(TABLE_CELL_H_PAD);
    let wrapped_cells = widths
        .iter()
        .enumerate()
        .map(|(index, width)| {
            wrapped_table_cell(
                cells.get(index).map(String::as_str).unwrap_or_default(),
                *width,
                color,
                header,
            )
        })
        .collect::<Vec<_>>();
    let row_height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1);

    (0..row_height)
        .map(|line_index| {
            let is_first = take_lead_marker(lead_marker, show_marker);
            // Shared content gutter (diamond or CONTENT_GUTTER spaces).
            let mut spans = marker_spans_at(show_marker, marker_color, is_first);
            for (column_index, width) in widths.iter().copied().enumerate() {
                if column_index > 0 {
                    spans.push(Span::styled(
                        TABLE_COL_SEP,
                        Style::default().fg(ghost()),
                    ));
                }
                // Inner cell pad (Grok block_pad feel) without a left-edge rail.
                spans.push(Span::styled(
                    cell_pad.clone(),
                    Style::default().fg(color),
                ));
                let cell_line = wrapped_cells
                    .get(column_index)
                    .and_then(|lines| lines.get(line_index))
                    .cloned()
                    .unwrap_or_default();
                let used = cell_line
                    .iter()
                    .map(|span| display_width(&span.content))
                    .sum::<usize>();
                spans.extend(cell_line);
                if used < width {
                    spans.push(Span::styled(
                        " ".repeat(width - used),
                        Style::default().fg(color),
                    ));
                }
                spans.push(Span::styled(
                    cell_pad.clone(),
                    Style::default().fg(color),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

fn wrapped_table_cell(
    content: &str,
    width: usize,
    color: Color,
    bold: bool,
) -> Vec<Vec<Span<'static>>> {
    let mut spans = inline_text_spans(content, color);
    if bold {
        for span in &mut spans {
            span.style = span.style.add_modifier(Modifier::BOLD);
        }
    }
    wrap_inline_spans_to_width(&spans, width)
}

fn table_header_rule_at(
    widths: &[usize],
    show_marker: bool,
    marker_color: Color,
    is_first_line: bool,
) -> Line<'static> {
    let mut spans = marker_spans_at(show_marker, marker_color, is_first_line);
    let cell_rule = TABLE_CELL_H_PAD.saturating_mul(2);
    for (index, width) in widths.iter().copied().enumerate() {
        if index > 0 {
            // Match TABLE_COL_SEP width (` │ ` → `─┼─`).
            spans.push(Span::styled("─┼─", Style::default().fg(ghost())));
        }
        spans.push(Span::styled(
            "─".repeat(width + cell_rule),
            Style::default().fg(ghost()),
        ));
    }
    Line::from(spans)
}

fn is_thematic_break(text: &str) -> bool {
    let compact = text
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    let Some(marker) = compact.chars().next() else {
        return false;
    };
    compact.len() >= 3
        && matches!(marker, '-' | '*' | '_')
        && compact.chars().all(|ch| ch == marker)
}

fn thematic_break_line_at(
    show_marker: bool,
    marker_color: Color,
    max_width: usize,
    is_first_line: bool,
) -> Line<'static> {
    let mut spans = marker_spans_at(show_marker, marker_color, is_first_line);
    let marker_width = if show_marker { 2 } else { CONTENT_GUTTER };
    let width = max_width.saturating_sub(marker_width).min(96).max(3);
    spans.push(Span::styled(
        "─".repeat(width),
        Style::default().fg(ghost()),
    ));
    Line::from(spans)
}

fn stacked_table_lines_at(
    rows: &[Vec<String>],
    show_marker: bool,
    marker_color: Color,
    max_width: usize,
    lead_marker: &mut bool,
) -> Vec<Line<'static>> {
    let Some(headers) = rows.first() else {
        return Vec::new();
    };
    let gutter_width = if show_marker { 2 } else { MD_BLOCK_H_PAD };
    let content_width = max_width.saturating_sub(gutter_width).max(16);
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
            // Row gap without a vertical trail.
            lines.push(Line::from(""));
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
                let is_first = take_lead_marker(lead_marker, show_marker);
                let mut spans = marker_spans_at(show_marker, marker_color, is_first);
                if line_index == 0 {
                    spans.push(Span::styled(
                        format!("{label:<label_width$}  "),
                        Style::default().fg(accent()).add_modifier(Modifier::BOLD),
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

        if rest.starts_with("[Image ") {
            let bytes = rest.as_bytes();
            let mut check_idx = 7;
            let mut has_digits = false;
            while check_idx < rest.len() && bytes[check_idx].is_ascii_digit() {
                has_digits = true;
                check_idx += 1;
            }
            if has_digits && check_idx < rest.len() && bytes[check_idx] == b']' {
                let tag_end = check_idx + 1;
                let tag_text = &rest[..tag_end];
                push_plain_span(&mut spans, &mut plain, fallback);
                spans.push(Span::styled(
                    tag_text.to_string(),
                    Style::default().bg(code_const()).fg(Color::Black),
                ));
                index += tag_end;
                continue;
            }
        }

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

fn code_panel_padding_line_at(
    show_marker: bool,
    marker_color: Color,
    is_first_line: bool,
) -> Line<'static> {
    let mut spans = marker_spans_at(show_marker, marker_color, is_first_line);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ChatImage, ChatMessage, ChatRole};

    #[test]
    fn user_images_render_as_input_style_tags() {
        let mut message = ChatMessage::new(ChatRole::User, "describe [Image 1]".to_string());
        message.image_labels.push("[Image 1]".to_string());
        message.images.push(ChatImage {
            index: 1,
            media_type: "image/png".to_string(),
            width: Some(100),
            height: Some(80),
            data: "abc".to_string(),
            label: "PNG".to_string(),
        });

        let output = build_chat_render_for_messages(
            &[message],
            80,
            false,
            false,
            0,
            &HashSet::new(),
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            &mut HashMap::new(),
            None,
        );

        assert!(output.lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref() == "[Image 1]")
        }));
        assert!(!output.lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.contains("[1] PNG"))
        }));
    }

    #[test]
    fn user_message_renders_prefix_and_right_clock() {
        // Fixed instant; clock string is local-TZ dependent but always h:mm AM/PM.
        let sent_at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_577_880_000);
        let lines = render_user_message_lines("hello world", 40, Some(sent_at));
        assert!(!lines.is_empty());
        let first = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(
            first.starts_with('›') || first.starts_with("› "),
            "expected › prefix, got {first:?}"
        );
        assert!(
            first.contains("AM") || first.contains("PM"),
            "expected right-aligned clock, got {first:?}"
        );
        assert!(
            first.contains("hello world"),
            "expected user text, got {first:?}"
        );
        // Clock should sit near the right edge (after padding).
        let clock_pos = first.rfind("AM").or_else(|| first.rfind("PM")).unwrap();
        assert!(
            clock_pos > first.find("hello").unwrap_or(0),
            "clock should be right of text: {first:?}"
        );
        // Full-width sticky bar reserves room: clock near the end.
        assert!(
            clock_pos >= 30,
            "clock should be right-aligned in the bar: {first:?}"
        );
    }

    #[test]
    fn user_message_without_stamp_has_no_clock() {
        let lines = render_user_message_lines("hello world", 40, None);
        let first = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(first.starts_with('›') || first.starts_with("› "));
        assert!(!first.contains("AM") && !first.contains("PM"));
        assert!(first.contains("hello world"));
    }

    #[test]
    fn format_message_clock_empty_without_stamp() {
        assert_eq!(format_message_clock(None), "");
    }

    #[test]
    fn format_message_clock_is_12h_ampm() {
        let stamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_704_067_200);
        let label = format_message_clock(Some(stamp));
        // Always "h:mm AM|PM" regardless of timezone.
        assert!(
            label.ends_with(" AM") || label.ends_with(" PM"),
            "unexpected clock format: {label:?}"
        );
        assert!(label.contains(':'), "missing minutes separator: {label:?}");
    }

    #[test]
    fn assistant_prose_uses_content_gutter() {
        let lines = render_markdown_lines("Hello assistant.", 40, text(), text(), false);
        let first = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(
            first.starts_with(&" ".repeat(CONTENT_GUTTER)),
            "assistant prose must start with content gutter, got {first:?}"
        );
        assert!(first.contains("Hello assistant."));
    }

    #[test]
    fn is_image_tag_recognizes_chip_text() {
        assert!(is_image_tag("[Image 1]"));
        assert!(is_image_tag("  [Image 12]  "));
        assert!(!is_image_tag("[Image]"));
        assert!(!is_image_tag("Image 1"));
        assert_eq!(parse_image_tag_index("[Image 3]"), Some(3));
    }

    #[test]
    fn thematic_break_is_rendered_as_a_rule() {
        let lines = render_markdown_lines("---", 80, text(), text(), false);

        assert_eq!(lines.len(), 1);
        let rendered = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        // Rule sits in the content column after the shared gutter.
        assert_eq!(
            rendered,
            format!("{}{}", " ".repeat(CONTENT_GUTTER), "─".repeat(80 - CONTENT_GUTTER))
        );
        assert!(!rendered.contains("---"));
    }

    #[test]
    fn wide_tables_are_bounded_and_wrap_long_cells() {
        let markdown = "| Empresa | Pessoa(s) | Posição |\n\
                        | --- | --- | --- |\n\
                        | Trainline ⛵ | Yan Pitangui | #23 |\n\
                        | **ABLA ONE / Microsoft FoundersHub** | Bruno (bmtec) | #47, #48 |\n\
                        | Open to work 🔍 | BGLuis, tqrcisio, rlevider, rust-ivf, oliveirajhony, gogoncalves, dalvorsn, bmtec, nathan | #25, #28, #30, #36, #37, #38, #40, #45, #47 |";
        let lines = render_markdown_lines(markdown, 220, text(), text(), false);

        assert!(
            lines.len() > 3,
            "long cells should wrap onto continuation lines"
        );
        assert!(
            lines
                .iter()
                .any(|line| { line.spans.iter().any(|span| span.content.contains('│')) })
        );
        assert!(
            lines
                .iter()
                .any(|line| { line.spans.iter().any(|span| span.content.contains('┼')) })
        );
        // Content is capped at 140; gutter (MD_BLOCK_H_PAD) sits outside that budget.
        let max_line = 140 + MD_BLOCK_H_PAD;
        assert!(lines.iter().all(|line| {
            line.spans
                .iter()
                .map(|span| display_width(&span.content))
                .sum::<usize>()
                <= max_line
        }));
        assert!(
            !lines
                .iter()
                .any(|line| { line.spans.iter().any(|span| span.content.contains("**")) })
        );

        let separator_positions = lines
            .iter()
            .filter(|line| line.spans.iter().any(|span| span.content.contains('│')))
            .map(|line| {
                let rendered = line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>();
                let mut positions = Vec::new();
                let mut width = 0;
                for ch in rendered.chars() {
                    if ch == '│' {
                        positions.push(width);
                    }
                    width += display_width(&ch.to_string());
                }
                positions
            })
            .collect::<Vec<_>>();
        assert!(separator_positions.len() > 1);
        assert!(
            separator_positions
                .windows(2)
                .all(|pair| pair[0] == pair[1])
        );
    }

    #[test]
    fn markdown_inserts_gap_between_prose_and_table() {
        let markdown = "Intro paragraph.\n\
                        | A | B |\n\
                        | --- | --- |\n\
                        | 1 | 2 |\n\
                        Closing paragraph.";
        let lines = render_markdown_lines(markdown, 80, text(), text(), false);
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        let intro = rendered.iter().position(|l| l.contains("Intro")).unwrap();
        let table = rendered
            .iter()
            .position(|l| l.contains('│') || l.contains(" A "))
            .unwrap();
        let closing = rendered.iter().position(|l| l.contains("Closing")).unwrap();

        assert!(table > intro);
        assert!(
            rendered[intro + 1..table]
                .iter()
                .any(|l| l.trim().is_empty()),
            "expected blank line between prose and table: {rendered:?}"
        );
        assert!(closing > table);
        assert!(
            rendered[table + 1..closing]
                .iter()
                .any(|l| l.trim().is_empty()),
            "expected blank line between table and prose: {rendered:?}"
        );
        // No corner trail on table gutter — pad spaces only.
        assert!(
            !rendered.iter().any(|l| l.starts_with('│')),
            "table must not start with a quote-bar trail"
        );
    }

    #[test]
    fn table_gutter_aligns_with_block_pad_not_trail() {
        let markdown = "| Name | Value |\n| --- | --- |\n| alpha | 1 |";
        let lines = render_markdown_lines(markdown, 80, text(), text(), false);
        let first = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(
            first.starts_with(&" ".repeat(MD_BLOCK_H_PAD)),
            "expected {MD_BLOCK_H_PAD}-space gutter, got {first:?}"
        );
        assert!(!first.starts_with('│'));
        assert!(!first.starts_with('┃'));
    }
}

fn code_panel_prefix_width() -> usize {
    3
}

/// Left gutter for chat content blocks.
///
/// - Marked (thinking): diamond on the first line, plain indent after.
/// - Unmarked (assistant/tool body): always `CONTENT_GUTTER` spaces so prose,
///   tables, and code share one content column with user `› ` and tool `◆ `.
/// Never draws a vertical bar / corner stroke (Grok quote-bar).
fn marker_spans_at(
    show_marker: bool,
    marker_color: Color,
    is_first_line: bool,
) -> Vec<Span<'static>> {
    if !show_marker {
        return vec![Span::styled(
            " ".repeat(CONTENT_GUTTER),
            Style::default().fg(ghost()),
        )];
    }
    if is_first_line {
        vec![Span::styled(
            format!("{} ", super::status::settled_diamond()),
            Style::default().fg(marker_color),
        )]
    } else {
        // Align under the diamond without drawing a trail.
        vec![Span::styled("  ", Style::default().fg(marker_color))]
    }
}
