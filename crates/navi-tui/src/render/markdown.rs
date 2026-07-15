use ratatui::prelude::{Line, Modifier, Span, Style};
use ratatui::style::Color;
use std::collections::{HashMap, HashSet};

use navi_sdk::{ToolInvocation, ToolResult};

use crate::state::{ChatLineSource, ChatMessage, ChatRole};
use crate::theme::*;

use super::syntax::{CodeHighlighter, highlight_code_line};
use super::text::{display_width, wrap_inline_spans_to_width, wrap_spans_to_width, wrap_text};
use super::tool::{tool_compact_text, tool_running_text};

/// Blank line between structural markdown blocks (prose ↔ table/code/heading).
/// outer_vpad / block breathing room.
const MD_BLOCK_V_GAP: usize = 1;
/// Shared content column: user text (after `› `), tools (`◆ `), assistant prose.
/// Keeps scrollback on two visual columns — gutter | content — .
const CONTENT_GUTTER: usize = 2;
/// Left pad for structural blocks so tables/code align under tool diamonds (`◆ `).
const MD_BLOCK_H_PAD: usize = CONTENT_GUTTER;
/// Inner horizontal pad inside each table cell .
const TABLE_CELL_H_PAD: usize = 1;
/// Width of an interior vertical border glyph (`│`).
const TABLE_BORDER_W: usize = 1;

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
                loading_elapsed_ms,
                subagent_activity,
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
                // sticky prompt: `› text…` left, clock right-aligned.
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
                    // "Recap" label + short summary (hard-cap 3 visual lines).
                    let summary = msg.content.trim();
                    let label = "Recap ";
                    let label_w = display_width(label);
                    let body_width = chat_width.saturating_sub(label_w).max(16);
                    let mut wraps = wrap_text(summary, body_width);
                    wraps.truncate(navi_core::RECAP_MAX_LINES.max(1));
                    if wraps.is_empty() {
                        wraps.push(String::new());
                    }
                    for (line_i, line) in wraps.into_iter().enumerate() {
                        if line_i == 0 {
                            rendered_lines.push(Line::from(vec![
                                Span::styled(
                                    label.to_string(),
                                    Style::default().fg(signal()).add_modifier(Modifier::BOLD),
                                ),
                                Span::styled(line, Style::default().fg(muted())),
                            ]));
                        } else {
                            // Indent continuation under the label.
                            rendered_lines.push(Line::from(vec![
                                Span::raw(" ".repeat(label_w)),
                                Span::styled(line, Style::default().fg(muted())),
                            ]));
                        }
                        line_sources.push(ChatLineSource::Message(index));
                    }
                    index += 1;
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
                let assistant_lines =
                    render_markdown_lines(&msg.content, chat_width, text(), text(), false);
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
    push_running_tools(
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

/// user prompt row:
/// ```text
/// › message text that wraps…                         4:25 AM
/// continued on next line without the clock
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
    let content_width = width.saturating_sub(prefix_w).saturating_sub(time_w).max(8);

    let display = text.trim();
    if display.is_empty() {
        return Vec::new();
    }
    let wrapped = wrap_text(display, content_width);
    let mut lines = Vec::new();

    for (i, line) in wrapped.into_iter().enumerate() {
        let is_first = i == 0;
        let mut spans = vec![Span::styled(
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
        )];
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

    lines
}

/// Format wall clock  `4:25 AM`. Empty if unknown.
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

/// In-flight tools (bash, read, subagent, …) with a pulsing diamond until they settle.
///
/// Previously only subagents were shown while running; bash/etc. were swallowed as
/// empty `tool:` placeholders, so the scrollback looked frozen mid-turn.
fn push_running_tools(
    rendered_lines: &mut Vec<Line<'static>>,
    line_sources: &mut Vec<ChatLineSource>,
    running_tools: &HashMap<String, ToolInvocation>,
    subagent_activity: &HashMap<String, String>,
    chat_width: usize,
    loading_elapsed_ms: Option<u64>,
) {
    if running_tools.is_empty() {
        return;
    }

    let mut tools = running_tools.values().collect::<Vec<_>>();
    tools.sort_by(|left, right| left.id.cmp(&right.id));

    let elapsed = loading_elapsed_ms.unwrap_or_default();
    let spinner = super::status::running_diamond_prefix(elapsed);
    let spin_color = super::status::running_diamond_color(code_operator());
    let elapsed_label = crate::background::format_duration_ms(elapsed);

    for invocation in tools {
        push_block_gap(rendered_lines, line_sources);
        let is_subagent = invocation.tool_name == "subagent";
        let source = if is_subagent {
            ChatLineSource::Subagent(invocation.id.clone())
        } else {
            ChatLineSource::ToolResult(invocation.id.clone())
        };

        let label = if is_subagent {
            let task = subagent_task_label(invocation);
            format!("Subagent Task — {task}")
        } else {
            tool_running_text(invocation)
        };

        // `◆ Run cargo test · 3s` — diamond pulses via spinner frame + elapsed.
        let width = chat_width.max(12);
        let suffix = format!(" · {elapsed_label}");
        let label_budget = width
            .saturating_sub(display_width(spinner) + display_width(&suffix))
            .max(8);
        let truncated = truncate_chars(&label, label_budget);
        let (action, detail) = truncated
            .split_once(' ')
            .map(|(a, d)| (a.to_string(), d.to_string()))
            .unwrap_or_else(|| (truncated.clone(), String::new()));
        let action_color = tool_color(invocation.tool_name.as_str());

        let mut spans = vec![
            Span::styled(
                spinner.to_string(),
                Style::default().fg(spin_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                action,
                Style::default()
                    .fg(action_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        if !detail.is_empty() {
            spans.push(Span::styled(" ", Style::default().fg(ghost())));
            spans.push(Span::styled(detail, Style::default().fg(muted())));
        }
        spans.push(Span::styled(suffix, Style::default().fg(code_number())));

        rendered_lines.push(Line::from(spans));
        line_sources.push(source.clone());

        // Subagent live detail under the header.
        if is_subagent {
            let task = subagent_task_label(invocation);
            let detail = subagent_activity
                .get(&invocation.id)
                .cloned()
                .unwrap_or_else(|| subagent_detail_label(invocation, &task));
            if !detail.is_empty() {
                let detail_width = width.saturating_sub(4).max(8);
                rendered_lines.push(Line::from(vec![
                    Span::styled("  ↳ ", Style::default().fg(ghost())),
                    Span::styled(
                        truncate_chars(&detail, detail_width),
                        Style::default().fg(muted()),
                    ),
                ]));
                line_sources.push(source);
            }
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
    loading_elapsed_ms: Option<u64>,
    subagent_activity: &HashMap<String, String>,
) -> Vec<(Line<'static>, ChatLineSource)> {
    use super::tool_policy::{tool_auto_expand, tool_body_visible};

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

    // Always show the one-line header (tool card title).
    lines.push((
        render_compact_tool_line_with_width(invocation, result, chat_width, loading_elapsed_ms),
        source.clone(),
    ));

    // Background-spawned subagents keep publishing activity after ToolCompleted.
    // Surface the latest status under the card so progress is still visible.
    if invocation.tool_name == "subagent"
        && tool_result_still_running(result)
        && let Some(detail) = subagent_activity.get(&result.invocation_id)
    {
        let detail = detail.trim();
        if !detail.is_empty() {
            let detail_width = chat_width.saturating_sub(4).max(8);
            lines.push((
                Line::from(vec![
                    Span::styled("  ↳ ".to_string(), Style::default().fg(ghost())),
                    Span::styled(
                        truncate_chars(detail, detail_width),
                        Style::default().fg(muted()),
                    ),
                ]),
                source.clone(),
            ));
        }
    }

    if show_body {
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

fn tool_result_still_running(result: &ToolResult) -> bool {
    result
        .output
        .get("status")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("running") || s.eq_ignore_ascii_case("pending"))
}

fn render_compact_tool_line_with_width(
    invocation: &ToolInvocation,
    result: &ToolResult,
    chat_width: usize,
    loading_elapsed_ms: Option<u64>,
) -> Line<'static> {
    // Background bash (and similar) can land a ToolCompleted with status=running
    // while the process is still going — keep the diamond pulsing.
    let still_running = tool_result_still_running(result);
    let elapsed = loading_elapsed_ms
        .or_else(|| result.output.get("elapsed_ms").and_then(|v| v.as_u64()))
        .unwrap_or(0);

    let (prefix, status_color) = if still_running {
        (
            super::status::running_diamond_prefix(elapsed),
            super::status::running_diamond_color(code_operator()),
        )
    } else {
        (
            super::status::settled_diamond_prefix(result.ok),
            super::status::settled_diamond_color(result.ok, Color::Green, red()),
        )
    };

    let text_width = chat_width.saturating_sub(display_width(prefix)).max(12);
    let label = truncate_chars(&tool_compact_text(invocation, result), text_width);
    let (action, detail) = label.split_once(' ').unwrap_or((&label, ""));
    let action_color = if still_running {
        tool_color(invocation.tool_name.as_str())
    } else if result.ok {
        tool_color(invocation.tool_name.as_str())
    } else {
        red()
    };
    let mut spans = vec![
        Span::styled(
            prefix.to_string(),
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
            if still_running || result.ok {
                muted()
            } else {
                text()
            },
        ));
    }
    Line::from(spans)
}

fn tool_color(tool_name: &str) -> Color {
    match tool_name {
        "read_file" | "view_file" | "grep" | "fs_browser" => code_type(),
        "write_file" | "apply_patch" | "edit" | "multiedit" | "write" => code_const(),
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
            let previous_language = language.clone();
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
                // DSL fences get a language badge line (mermaid / latex / charts).
                if let Some(label) = dsl_language_label(&language) {
                    let is_first = take_lead_marker(&mut lead_marker, show_marker);
                    let mut spans = marker_spans_at(show_marker, marker_color, is_first);
                    spans.push(Span::styled(
                        format!("▸ {label}"),
                        Style::default().fg(accent()).add_modifier(Modifier::BOLD),
                    ));
                    lines.push(Line::from(spans));
                } else {
                    let is_first = take_lead_marker(&mut lead_marker, show_marker);
                    lines.push(code_panel_padding_line_at(
                        show_marker,
                        marker_color,
                        is_first,
                    ));
                }
            } else {
                code_highlighter = None;
                // Closing fence: rail padding only for normal code (not DSLs).
                if !is_dsl_language(&previous_language) {
                    let is_first = take_lead_marker(&mut lead_marker, show_marker);
                    lines.push(code_panel_padding_line_at(
                        show_marker,
                        marker_color,
                        is_first,
                    ));
                }
            }
            last_block = MdBlockKind::Code;
            index += 1;
            continue;
        }

        if in_code {
            let spans = if is_diff_language(&language) {
                diff_line_spans(raw_line)
            } else if is_dsl_language(&language) {
                dsl_line_spans(raw_line, &language)
            } else if language.is_empty() {
                terminal_output_spans(raw_line)
            } else if let Some(ref mut hl) = code_highlighter {
                hl.highlight_line(raw_line)
            } else {
                highlight_code_line(raw_line, &language)
            };
            let marker_width = if show_marker { 2 } else { CONTENT_GUTTER };
            let panel_prefix_width = if is_dsl_language(&language) {
                0
            } else {
                code_panel_prefix_width()
            };
            let content_width = max_width
                .saturating_sub(marker_width + panel_prefix_width + 1)
                .max(1);
            let wrapped = wrap_spans_to_width(&spans, content_width);
            let wrapped = if wrapped.is_empty() {
                vec![Vec::new()]
            } else {
                wrapped
            };
            // Diff body tint (add/remove wash). Numbers keep fg-only color and
            // must not inherit this bg — the paint starts after the gutter.
            let line_tint = spans.iter().find_map(|s| s.style.bg);
            let is_diff = is_diff_language(&language);
            for content_spans in wrapped {
                let is_first = take_lead_marker(&mut lead_marker, show_marker);
                let mut line_spans = marker_spans_at(show_marker, marker_color, is_first);
                if !is_dsl_language(&language) {
                    line_spans.extend(code_panel_prefix_spans());
                }
                for span in content_spans {
                    line_spans.push(span);
                }
                if let Some(bg) = line_tint {
                    if is_diff {
                        // Leave number gutter / left pad unpainted; only fill
                        // spans that already carry the tint (code body).
                        // Trailing pad to viewport is handled by pad_code_block_bg.
                        lines.push(Line::from(line_spans));
                    } else {
                        for span in &mut line_spans {
                            if span.style.bg.is_none() {
                                span.style = span.style.bg(bg);
                            }
                        }
                        lines.push(Line::from(line_spans));
                    }
                } else {
                    lines.push(Line::from(line_spans));
                }
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
    if trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || ordered_list_marker(trimmed).is_some()
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
        // Real markdown headings — diamonds are reserved for NAVI tool ops only.
        // Terminals cannot change point size; hierarchy uses weight + color + underline cue.
        let title = &trimmed[heading + 1..];
        let (fg, mods) = match heading {
            1 => (signal(), Modifier::BOLD),
            2 => (accent(), Modifier::BOLD),
            3 => (code_type(), Modifier::BOLD),
            _ => (crate::theme::text(), Modifier::BOLD),
        };
        // Visual "size": H1/H2 get a leading weight mark + bold; deeper levels are quieter.
        if heading == 1 {
            spans.push(Span::styled(
                "# ".to_string(),
                Style::default().fg(fg).add_modifier(mods),
            ));
        } else if heading == 2 {
            spans.push(Span::styled(
                "## ".to_string(),
                Style::default().fg(fg).add_modifier(mods),
            ));
        } else {
            spans.push(Span::styled(
                format!("{} ", "#".repeat(heading)),
                Style::default().fg(muted()).add_modifier(Modifier::BOLD),
            ));
        }
        spans.extend(inline_text_spans(title, fg).into_iter().map(|mut span| {
            span.style = span.style.fg(fg).add_modifier(mods);
            span
        }));
        return Some(spans);
    }

    if let Some(rest) = trimmed.strip_prefix("> ") {
        // Blockquote: vertical bar, not a diamond.
        spans.push(Span::styled("│ ".to_string(), Style::default().fg(ghost())));
        spans.extend(inline_text_spans(rest, muted()).into_iter().map(|mut s| {
            s.style = s.style.add_modifier(Modifier::ITALIC);
            s
        }));
        return Some(spans);
    }

    if trimmed.starts_with('|') && trimmed.ends_with('|') {
        spans.extend(table_row_spans(&table_cells(trimmed), &[]));
        return Some(spans);
    }

    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        // Standard list bullet — not a status diamond.
        spans.push(Span::styled(
            "• ".to_string(),
            Style::default().fg(accent()),
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
    const MIN_COLUMN_WIDTH: usize = 4;

    let rows = table_rows
        .iter()
        .map(|row| table_cells(row))
        .collect::<Vec<_>>();
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    if column_count == 0 {
        return Vec::new();
    }
    // Content width before cell pad; pad is added to the column budget later.
    let mut content_widths = vec![0; column_count];
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            content_widths[index] = content_widths[index].max(rendered_inline_width(cell));
        }
    }

    // Align tables under tool diamonds: marker (`◆ `) or MD_BLOCK_H_PAD spaces.
    // full box needs outer left/right borders (`│` × (cols+1)).
    let gutter_width = if show_marker { 2 } else { MD_BLOCK_H_PAD };
    let available_width = max_width.saturating_sub(gutter_width).min(MAX_TABLE_WIDTH);
    let border_budget = column_count
        .saturating_add(1)
        .saturating_mul(TABLE_BORDER_W);
    let pad_budget = column_count.saturating_mul(TABLE_CELL_H_PAD.saturating_mul(2));
    let columns_width = available_width
        .saturating_sub(border_budget)
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
        .unwrap_or_else(|| vec![MIN_COLUMN_WIDTH; column_count]);

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

    // Table: full box frame on a panel surface.
    // ┌──────┬──────┐
    // │ head │ head │
    // ├──────┼──────┤
    // │ cell │ cell │
    // └──────┴──────┘
    let mut lines = Vec::new();
    let is_first = take_lead_marker(lead_marker, show_marker);
    lines.push(table_border_line(
        &content_widths,
        TableBorderKind::Top,
        show_marker,
        marker_color,
        is_first,
    ));

    for (row_index, cells) in rows.iter().enumerate() {
        lines.extend(wrapped_table_row_lines_at(
            cells,
            &content_widths,
            row_index == 0,
            show_marker,
            marker_color,
            lead_marker,
        ));
        if row_index == 0 && rows.len() > 1 {
            let is_first = take_lead_marker(lead_marker, show_marker);
            lines.push(table_border_line(
                &content_widths,
                TableBorderKind::HeaderRule,
                show_marker,
                marker_color,
                is_first,
            ));
        }
    }

    let is_first = take_lead_marker(lead_marker, show_marker);
    lines.push(table_border_line(
        &content_widths,
        TableBorderKind::Bottom,
        show_marker,
        marker_color,
        is_first,
    ));
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

/// Box-drawing row kinds for tables.
#[derive(Clone, Copy)]
enum TableBorderKind {
    Top,
    HeaderRule,
    Bottom,
}

fn table_border_style() -> Style {
    // Tables sit on the chat background — no elevated panel fill.
    Style::default().fg(ghost())
}

fn table_border_line(
    widths: &[usize],
    kind: TableBorderKind,
    show_marker: bool,
    marker_color: Color,
    is_first_line: bool,
) -> Line<'static> {
    let (left, mid, right, fill) = match kind {
        TableBorderKind::Top => ('┌', '┬', '┐', '─'),
        TableBorderKind::HeaderRule => ('├', '┼', '┤', '─'),
        TableBorderKind::Bottom => ('└', '┴', '┘', '─'),
    };
    let cell_inner = TABLE_CELL_H_PAD.saturating_mul(2);
    let border = table_border_style();
    let mut spans = marker_spans_at(show_marker, marker_color, is_first_line);
    // Paint gutter with transparent/bg so the box sits on chat surface.
    spans.push(Span::styled(left.to_string(), border));
    for (index, width) in widths.iter().copied().enumerate() {
        if index > 0 {
            spans.push(Span::styled(mid.to_string(), border));
        }
        spans.push(Span::styled(
            fill.to_string().repeat(width + cell_inner),
            border,
        ));
    }
    spans.push(Span::styled(right.to_string(), border));
    Line::from(spans)
}

fn wrapped_table_row_lines_at(
    cells: &[String],
    widths: &[usize],
    header: bool,
    show_marker: bool,
    marker_color: Color,
    lead_marker: &mut bool,
) -> Vec<Line<'static>> {
    // Header uses muted bold (table headers); body uses default text.
    // Inline `code` keeps syntax colors via inline_text_spans. No panel fill.
    let color = if header { muted() } else { text() };
    let border = table_border_style();
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
    let row_height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1).max(1);

    (0..row_height)
        .map(|line_index| {
            let is_first = take_lead_marker(lead_marker, show_marker);
            let mut spans = marker_spans_at(show_marker, marker_color, is_first);
            spans.push(Span::styled("│".to_string(), border));
            for (column_index, width) in widths.iter().copied().enumerate() {
                if column_index > 0 {
                    spans.push(Span::styled("│".to_string(), border));
                }
                spans.push(Span::styled(cell_pad.clone(), Style::default().fg(color)));
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
                spans.push(Span::styled(cell_pad.clone(), Style::default().fg(color)));
            }
            spans.push(Span::styled("│".to_string(), border));
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
    for span in &mut spans {
        if bold {
            span.style = span.style.add_modifier(Modifier::BOLD);
        }
    }
    wrap_inline_spans_to_width(&spans, width)
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
    // Narrow/wide fallback: bordered card with key/value rows.
    let Some(headers) = rows.first() else {
        return Vec::new();
    };
    let gutter_width = if show_marker { 2 } else { MD_BLOCK_H_PAD };
    let content_width = max_width.saturating_sub(gutter_width).max(16);
    // Inner width between left/right `│`.
    let inner_width = content_width.saturating_sub(2).max(12);
    let label_width = headers
        .iter()
        .map(|header| rendered_inline_width(header))
        .max()
        .unwrap_or(0)
        .min(inner_width.saturating_sub(6));
    let value_width = inner_width
        .saturating_sub(label_width)
        .saturating_sub(3) // ": " + spacing
        .max(6);
    let border = table_border_style();

    let mut lines = Vec::new();
    let hbar = |lead: &mut bool, left: char, right: char| -> Line<'static> {
        let is_first = take_lead_marker(lead, show_marker);
        let mut spans = marker_spans_at(show_marker, marker_color, is_first);
        spans.push(Span::styled(
            format!("{left}{}{right}", "─".repeat(inner_width)),
            border,
        ));
        Line::from(spans)
    };

    lines.push(hbar(lead_marker, '┌', '┐'));

    for (row_index, row) in rows.iter().enumerate().skip(1) {
        if row_index > 1 {
            lines.push(hbar(lead_marker, '├', '┤'));
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
                spans.push(Span::styled("│".to_string(), border));

                let mut body = String::new();
                body.push(' ');
                if line_index == 0 {
                    body.push_str(&format!("{label:<label_width$} "));
                } else {
                    body.push_str(&" ".repeat(label_width + 1));
                }
                body.push_str(&value);
                // Pad to inner_width then close border.
                let body_w = display_width(&body);
                if body_w < inner_width {
                    body.push_str(&" ".repeat(inner_width - body_w));
                } else if body_w > inner_width {
                    // Hard-trim by display columns.
                    body = truncate_display(&body, inner_width);
                }

                // Split label (bold/muted) from value (text) for nicer look.
                if line_index == 0 {
                    let label_part = format!(" {label:<label_width$} ");
                    let label_w = display_width(&label_part);
                    spans.push(Span::styled(
                        label_part,
                        Style::default().fg(muted()).add_modifier(Modifier::BOLD),
                    ));
                    let val_spans = inline_text_spans(&value, text());
                    let val_w: usize = val_spans.iter().map(|s| display_width(&s.content)).sum();
                    spans.extend(val_spans);
                    let fill = inner_width.saturating_sub(label_w + val_w);
                    if fill > 0 {
                        spans.push(Span::styled(" ".repeat(fill), Style::default().fg(text())));
                    }
                } else {
                    let indent = format!(" {}", " ".repeat(label_width + 1));
                    spans.push(Span::styled(indent.clone(), Style::default().fg(text())));
                    let val_spans = inline_text_spans(&value, text());
                    let val_w: usize = val_spans.iter().map(|s| display_width(&s.content)).sum();
                    spans.extend(val_spans);
                    let fill = inner_width.saturating_sub(display_width(&indent) + val_w);
                    if fill > 0 {
                        spans.push(Span::styled(" ".repeat(fill), Style::default().fg(text())));
                    }
                }
                let _ = body;
                spans.push(Span::styled("│".to_string(), border));
                lines.push(Line::from(spans));
            }
        }
    }

    lines.push(hbar(lead_marker, '└', '┘'));
    lines
}

fn truncate_display(text: &str, max_cols: usize) -> String {
    let mut out = String::new();
    let mut cols = 0usize;
    for ch in text.chars() {
        let w = display_width(&ch.to_string()).max(1);
        if cols + w > max_cols {
            break;
        }
        out.push(ch);
        cols += w;
    }
    out
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
    // No solid code-panel wash — syntax colors only.
    semantic_plain_spans(raw_line, text())
}

fn is_diff_language(language: &str) -> bool {
    matches!(language, "diff" | "patch")
}

fn diff_line_spans(raw_line: &str) -> Vec<Span<'static>> {
    // Hunk separators / meta headers (path chrome, … between hunks).
    if raw_line.starts_with("@@") || raw_line.trim() == "…" || raw_line.trim() == "..." {
        return vec![Span::styled(
            raw_line.to_string(),
            Style::default()
                .fg(code_const())
                .bg(diff_hunk_bg())
                .add_modifier(Modifier::BOLD),
        )];
    }
    if raw_line.starts_with("diff ")
        || raw_line.starts_with("index ")
        || raw_line.starts_with("*** ")
        || raw_line.starts_with("+++")
        || raw_line.starts_with("---")
    {
        return vec![Span::styled(
            raw_line.to_string(),
            Style::default()
                .fg(code_func())
                .bg(diff_meta_bg())
                .add_modifier(Modifier::BOLD),
        )];
    }

    // Claude Code–style: full-row red/green wash (including line-number gutter),
    // add/remove fg on body text, no raw +/- glyphs when numbers are present.
    let (bg, number_color, content_color) = if raw_line.starts_with('+') {
        (Some(diff_add_bg()), diff_add_fg(), diff_add_fg())
    } else if raw_line.starts_with('-') {
        (Some(diff_remove_bg()), diff_remove_fg(), diff_remove_fg())
    } else {
        // Context lines: no panel fill — only gutter number color.
        (None, ghost(), text())
    };

    if let Some((marker, rest)) = diff_marker_and_rest(raw_line) {
        // Numbered form (`+  39|content`): line number + body share the full-row wash.
        if let Some((num, after)) = split_diff_line_number(rest) {
            let mut spans = Vec::with_capacity(4);
            let mut num_style = Style::default()
                .fg(number_color)
                .add_modifier(Modifier::BOLD);
            if let Some(bg) = bg {
                num_style = num_style.bg(bg);
            }
            spans.push(Span::styled(format!("{num:>4} "), num_style));
            if after.is_empty() {
                // Keep a zero-width content holder so the row still tints when padded.
                if let Some(bg) = bg {
                    spans.push(Span::styled(" ", Style::default().bg(bg)));
                }
            } else {
                spans.extend(highlight_diff_code(after, content_color, bg));
            }
            return spans;
        }

        // Unnumbered legacy lines: +/- marker is colored fg only; body gets the wash.
        let mut spans = vec![Span::styled(
            marker.to_string(),
            Style::default()
                .fg(number_color)
                .add_modifier(Modifier::BOLD),
        )];
        spans.extend(highlight_diff_code(rest, content_color, bg));
        return spans;
    }

    semantic_plain_spans(raw_line, content_color)
        .into_iter()
        .map(|mut span| {
            if let Some(bg) = bg {
                span.style = span.style.bg(bg);
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

/// Split a fixed 4-wide line-number gutter produced by `normalize_diff_for_display`.
///
/// Numbered form after the +/-/space marker: `{num:>4}|{content}`
/// e.g. rest = `"  39|- Default registry: …"`.
fn split_diff_line_number(rest: &str) -> Option<(&str, &str)> {
    // Need at least 4-wide field + `|`.
    if rest.len() < 5 {
        return None;
    }
    let field = rest.get(..4)?;
    if !field.chars().all(|ch| ch.is_ascii_digit() || ch == ' ') {
        return None;
    }
    let digits = field.trim();
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    if rest.as_bytes().get(4) != Some(&b'|') {
        return None;
    }
    Some((digits, rest.get(5..).unwrap_or("")))
}

fn highlight_diff_code(rest: &str, fallback: Color, bg: Option<Color>) -> Vec<Span<'static>> {
    let highlighted = highlight_code_line(rest, "rust");
    let mut spans = if highlighted.len() == 1 && highlighted[0].style.fg == Some(text()) {
        semantic_plain_spans(rest, fallback)
    } else {
        highlighted
    };
    for span in &mut spans {
        if let Some(bg) = bg {
            span.style = span.style.bg(bg);
        }
        if span.style.fg.is_none() {
            span.style = span.style.fg(fallback);
        }
    }
    spans
}

fn is_dsl_language(language: &str) -> bool {
    dsl_language_label(language).is_some()
}

fn dsl_language_label(language: &str) -> Option<&'static str> {
    match language.to_ascii_lowercase().as_str() {
        "mermaid" => Some("mermaid"),
        "latex" | "tex" | "math" | "katex" => Some("math / latex"),
        "chart" | "vega" | "vegalite" | "vega-lite" | "plotly" | "graphviz" | "dot" => {
            Some("chart")
        }
        _ => None,
    }
}

fn dsl_line_spans(raw_line: &str, language: &str) -> Vec<Span<'static>> {
    let lang = language.to_ascii_lowercase();
    if matches!(lang.as_str(), "latex" | "tex" | "math" | "katex") {
        let rendered = latex_to_unicode(raw_line);
        return vec![Span::styled(
            rendered,
            Style::default()
                .fg(code_const())
                .add_modifier(Modifier::ITALIC),
        )];
    }
    if lang == "mermaid" {
        // Soft keyword highlight for common mermaid tokens.
        return mermaid_line_spans(raw_line);
    }
    // Generic chart / graph DSL: muted mono body.
    semantic_plain_spans(raw_line, muted())
}

fn mermaid_line_spans(raw_line: &str) -> Vec<Span<'static>> {
    let keywords = [
        "graph",
        "flowchart",
        "sequenceDiagram",
        "classDiagram",
        "stateDiagram",
        "erDiagram",
        "gantt",
        "pie",
        "subgraph",
        "end",
        "participant",
        "Note",
        "loop",
        "alt",
        "else",
        "opt",
        "par",
        "and",
        "critical",
        "break",
        "rect",
        "activate",
        "deactivate",
        "direction",
        "TB",
        "TD",
        "BT",
        "RL",
        "LR",
    ];
    let mut spans = Vec::new();
    let mut rest = raw_line;
    while !rest.is_empty() {
        let leading = rest.chars().take_while(|c| c.is_whitespace()).count();
        if leading > 0 {
            spans.push(Span::styled(
                rest[..leading].to_string(),
                Style::default().fg(text()),
            ));
            rest = &rest[leading..];
            continue;
        }
        let token_len = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
            .map(|c| c.len_utf8())
            .sum::<usize>();
        if token_len == 0 {
            let ch = rest.chars().next().unwrap();
            let w = ch.len_utf8();
            spans.push(Span::styled(
                rest[..w].to_string(),
                Style::default().fg(code_punct()),
            ));
            rest = &rest[w..];
            continue;
        }
        let token = &rest[..token_len];
        let is_kw = keywords.iter().any(|k| k.eq_ignore_ascii_case(token));
        spans.push(Span::styled(
            token.to_string(),
            Style::default()
                .fg(if is_kw { code_keyword() } else { text() })
                .add_modifier(if is_kw {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ));
        rest = &rest[token_len..];
    }
    spans
}

/// Lightweight LaTeX → Unicode for common math tokens in the TUI.
fn latex_to_unicode(input: &str) -> String {
    let mut out = input.to_string();
    // Strip display/inline math delimiters for cleaner reading.
    for delim in ["$$", "$", "\\[", "\\]", "\\(", "\\)"] {
        out = out.replace(delim, "");
    }
    let replacements = [
        ("\\alpha", "α"),
        ("\\beta", "β"),
        ("\\gamma", "γ"),
        ("\\delta", "δ"),
        ("\\epsilon", "ε"),
        ("\\theta", "θ"),
        ("\\lambda", "λ"),
        ("\\mu", "μ"),
        ("\\pi", "π"),
        ("\\sigma", "σ"),
        ("\\phi", "φ"),
        ("\\omega", "ω"),
        ("\\times", "×"),
        ("\\cdot", "·"),
        ("\\pm", "±"),
        ("\\mp", "∓"),
        ("\\leq", "≤"),
        ("\\geq", "≥"),
        ("\\neq", "≠"),
        ("\\approx", "≈"),
        ("\\infty", "∞"),
        ("\\sum", "∑"),
        ("\\prod", "∏"),
        ("\\int", "∫"),
        ("\\sqrt", "√"),
        ("\\partial", "∂"),
        ("\\nabla", "∇"),
        ("\\rightarrow", "→"),
        ("\\leftarrow", "←"),
        ("\\Rightarrow", "⇒"),
        ("\\Leftarrow", "⇐"),
        ("\\leftrightarrow", "↔"),
        ("\\in", "∈"),
        ("\\notin", "∉"),
        ("\\subset", "⊂"),
        ("\\subseteq", "⊆"),
        ("\\cup", "∪"),
        ("\\cap", "∩"),
        ("\\forall", "∀"),
        ("\\exists", "∃"),
        ("\\emptyset", "∅"),
        ("\\mathbb{R}", "ℝ"),
        ("\\mathbb{N}", "ℕ"),
        ("\\mathbb{Z}", "ℤ"),
        ("\\mathbb{Q}", "ℚ"),
        ("\\mathbb{C}", "ℂ"),
        ("\\frac", "/"),
        ("\\left", ""),
        ("\\right", ""),
        ("\\{", "{"),
        ("\\}", "}"),
        ("\\\\", " "),
    ];
    for (from, to) in replacements {
        out = out.replace(from, to);
    }
    // Collapse common brace groups: {x} → x
    while let Some(start) = out.find('{') {
        if let Some(end) = out[start..].find('}') {
            let inner = out[start + 1..start + end].to_string();
            out.replace_range(start..start + end + 1, &inner);
        } else {
            break;
        }
    }
    out
}

/// Green wash for additions (Claude Code–like).
fn diff_add_bg() -> Color {
    Color::Rgb(20, 58, 48)
}

fn diff_add_fg() -> Color {
    Color::Rgb(62, 207, 142)
}

/// Red wash for removals.
fn diff_remove_bg() -> Color {
    Color::Rgb(72, 32, 42)
}

fn diff_remove_fg() -> Color {
    Color::Rgb(240, 113, 120)
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
    // Fence padding: gutter rail only — no solid code-panel background.
    let mut spans = marker_spans_at(show_marker, marker_color, is_first_line);
    spans.extend(code_panel_prefix_spans());
    Line::from(spans)
}

fn code_panel_prefix_spans() -> Vec<Span<'static>> {
    vec![
        Span::styled("│", Style::default().fg(ghost())),
        Span::styled("  ", Style::default().fg(text())),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ChatImage, ChatMessage, ChatRole};
    use serde_json::json;

    #[test]
    fn headings_use_markdown_hashes_not_tool_diamonds() {
        let lines =
            render_markdown_lines("# Title\n## Section\n### Detail", 80, text(), text(), false);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("# Title") || joined.contains("Title"));
        assert!(joined.contains("##") || joined.contains("Section"));
        assert!(
            !joined.contains('◆') && !joined.contains('◇'),
            "got:\n{joined}"
        );
    }

    #[test]
    fn latex_and_mermaid_fences_render_as_dsl_blocks() {
        let md =
            "```latex\n\\alpha + \\beta = \\gamma\n```\n\n```mermaid\ngraph TD\n  A-->B\n```\n";
        let lines = render_markdown_lines(md, 80, text(), text(), false);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("math") || joined.contains("latex") || joined.contains("α"),
            "expected math dsl, got:\n{joined}"
        );
        assert!(
            joined.contains("mermaid") || joined.contains("graph"),
            "expected mermaid dsl, got:\n{joined}"
        );
        assert!(
            joined.contains("α")
                || joined.contains("beta")
                || joined.contains("γ")
                || joined.contains("α + β"),
            "expected latex unicode, got:\n{joined}"
        );
    }

    #[test]
    fn split_diff_line_number_parses_fixed_gutter() {
        assert_eq!(
            split_diff_line_number("  39|- Default registry"),
            Some(("39", "- Default registry"))
        );
        assert_eq!(
            split_diff_line_number("   1|fn main() {}"),
            Some(("1", "fn main() {}"))
        );
        assert_eq!(split_diff_line_number("1234|x"), Some(("1234", "x")));
        // Unnumbered / ambiguous content must not match.
        assert_eq!(split_diff_line_number("old line"), None);
        assert_eq!(split_diff_line_number("  39 bottles"), None);
        assert_eq!(split_diff_line_number("  39 old"), None);
    }

    #[test]
    fn numbered_diff_lines_render_gutter_without_plus_minus_glyph() {
        let md = "```diff\n-  39|old line\n+  39|new line\n  40|context\n```\n";
        let lines = render_markdown_lines(md, 80, muted(), text(), false);
        let joined: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        let body = joined.join("\n");
        assert!(
            body.contains("39") && body.contains("old line") && body.contains("new line"),
            "expected numbered diff body, got:\n{body}"
        );
        // Color rail hides the raw +/- sign when a gutter is present.
        assert!(
            !body.contains("-  39") && !body.contains("+  39"),
            "raw +/- gutter chrome should be hidden, got:\n{body}"
        );
        assert!(body.contains("40") && body.contains("context"));
    }

    #[test]
    fn numbered_diff_gutter_shares_full_row_wash() {
        // Claude Code–style: line numbers sit inside the same red/green row wash.
        let md = "```diff\n-  12|removed\n+  12|added\n```\n";
        let lines = render_markdown_lines(md, 80, text(), text(), false);
        let mut saw_num = false;
        let mut saw_body_tint = false;
        let mut saw_num_wash = false;
        for line in &lines {
            for span in &line.spans {
                let text = span.content.as_ref().trim();
                if text == "12"
                    || (text.ends_with("12")
                        && text.chars().all(|c| c.is_ascii_digit() || c == ' '))
                {
                    saw_num = true;
                    assert!(
                        span.style.fg == Some(diff_add_fg())
                            || span.style.fg == Some(diff_remove_fg())
                            || span.style.fg == Some(ghost()),
                        "number should be add/remove/context color, got {:?}",
                        span.style.fg
                    );
                    if span.style.bg == Some(diff_add_bg())
                        || span.style.bg == Some(diff_remove_bg())
                    {
                        saw_num_wash = true;
                    }
                }
                if span.content.as_ref().contains("removed")
                    || span.content.as_ref().contains("added")
                {
                    if span.style.bg == Some(diff_add_bg())
                        || span.style.bg == Some(diff_remove_bg())
                    {
                        saw_body_tint = true;
                    }
                }
            }
        }
        assert!(saw_num, "expected numbered gutter");
        assert!(saw_num_wash, "expected full-row wash on line-number gutter");
        assert!(saw_body_tint, "expected body line wash on content");
    }

    #[test]
    fn running_bash_renders_pulsing_header_with_elapsed() {
        let mut running = HashMap::new();
        running.insert(
            "call-1".into(),
            ToolInvocation {
                id: "call-1".into(),
                tool_name: "bash".into(),
                input: json!({ "command": "cargo test -p navi-core" }),
            },
        );

        let output = build_chat_render_for_messages(
            &[],
            80,
            false,
            false,
            0,
            &HashSet::new(),
            &HashSet::new(),
            &running,
            &HashMap::new(),
            &mut HashMap::new(),
            Some(3_200), // 10 pulse frames
        );

        let joined: String = output
            .lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");
        assert!(
            joined.contains("Run") && joined.contains("cargo test"),
            "expected running bash summary, got: {joined}"
        );
        assert!(
            joined.contains("◆") || joined.contains("◇"),
            "expected pulse diamond, got: {joined}"
        );
        assert!(
            joined.contains("3s") || joined.contains("3200"),
            "expected elapsed: {joined}"
        );
    }

    #[test]
    fn background_running_result_uses_running_diamond() {
        let inv = ToolInvocation {
            id: "bg-1".into(),
            tool_name: "bash".into(),
            input: json!({ "command": "sleep 30", "background": true }),
        };
        let result = ToolResult {
            invocation_id: "bg-1".into(),
            ok: true,
            output: json!({
                "background": true,
                "status": "running",
                "elapsed_ms": 1500,
                "stdout": "…",
            }),
        };
        let line = render_compact_tool_line_with_width(&inv, &result, 80, Some(1500));
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            text.starts_with('◆') || text.starts_with('◇'),
            "background running should pulse, got: {text}"
        );
        assert!(text.contains("Run") || text.contains("sleep"), "{text}");
    }

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
            format!(
                "{}{}",
                " ".repeat(CONTENT_GUTTER),
                "─".repeat(80 - CONTENT_GUTTER)
            )
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
        // Full box: top/header/bottom junctions.
        assert!(
            lines.iter().any(|line| {
                line.spans
                    .iter()
                    .any(|span| span.content.contains('┌') || span.content.contains('┬'))
            }),
            "expected top box border"
        );
        assert!(
            lines
                .iter()
                .any(|line| { line.spans.iter().any(|span| span.content.contains('┼')) }),
            "expected header rule with ┼"
        );
        assert!(
            lines.iter().any(|line| {
                line.spans
                    .iter()
                    .any(|span| span.content.contains('└') || span.content.contains('┴'))
            }),
            "expected bottom box border"
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
        // Gutter pad then box — never a bare quote-bar at column 0.
        assert!(!first.starts_with('│'));
        assert!(!first.starts_with('┃'));
        assert!(
            first.contains('┌'),
            "Table should open with top border, got {first:?}"
        );
    }

    #[test]
    fn table_has_full_box_frame() {
        let markdown =
            "| Área | Fix |\n| --- | --- |\n| Chat | running tools |\n| Cache | pulse frame |";
        let lines = render_markdown_lines(markdown, 80, text(), text(), false);
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        let joined = rendered.join("\n");
        assert!(joined.contains('┌') && joined.contains('┐'), "{joined}");
        assert!(joined.contains('├') && joined.contains('┤'), "{joined}");
        assert!(joined.contains('└') && joined.contains('┘'), "{joined}");
        assert!(
            joined.contains("Área") && joined.contains("Chat"),
            "{joined}"
        );
        // Column separators present on body rows.
        assert!(
            rendered.iter().filter(|l| l.contains('│')).count() >= 2,
            "{joined}"
        );
    }
}

fn code_panel_prefix_width() -> usize {
    3
}

/// Left gutter for chat content blocks.
///
/// - Marked (thinking): diamond on the first line, plain indent after.
/// - Unmarked (assistant/tool body): always `CONTENT_GUTTER` spaces so prose,
/// tables, and code share one content column with user `› ` and tool `◆ `.
/// Never draws a vertical bar / corner stroke (quote-bar).
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
