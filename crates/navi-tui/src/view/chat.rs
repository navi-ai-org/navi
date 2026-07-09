use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use ratatui::layout::{Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Paragraph, Wrap};

use navi_sdk::SubagentTranscriptKind;

use crate::TuiApp;
use crate::render::markdown::build_chat_render_for_messages;
use crate::render::text::display_width;
use crate::state::{ChatLineSource, ChatRole, ChatView, Mode};
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};

use super::welcome::welcome_text;

pub(crate) fn render_chat_area(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    app.chat_render_cache.borrow_mut().chat_rect = Some(inner);

    if let ChatView::Subagent { invocation_id } = app.chat_view.clone() {
        render_subagent_chat_area(frame, app, inner, &invocation_id);
        return;
    }

    if app.messages.is_empty() && !app.is_loading {
        let welcome = welcome_text(app, inner.width as usize, inner.height as usize);
        frame.render_widget(
            Paragraph::new(welcome)
                .style(Style::default().bg(bg()))
                .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }

    let chat_width = inner.width as usize;
    ensure_chat_cache(app, chat_width);
    let visible_height = inner.height as usize;
    {
        let cache = app.chat_render_cache.borrow();
        let max_scroll = cache.lines.len().saturating_sub(visible_height);
        app.scroll_offset = app.scroll_offset.min(max_scroll);
    }
    let (start, mut visible_lines, visible_sources) = {
        let cache = app.chat_render_cache.borrow();
        let rendered_lines = &cache.lines;
        let total_lines = rendered_lines.len();
        let max_scroll = total_lines.saturating_sub(visible_height);
        let effective_scroll = app.scroll_offset.min(max_scroll);
        let start = total_lines
            .saturating_sub(visible_height)
            .saturating_sub(effective_scroll);
        let end = (start + visible_height).min(total_lines);
        let source_end = end.min(cache.line_sources.len());
        (
            start,
            rendered_lines[start..end].to_vec(),
            cache.line_sources[start.min(source_end)..source_end].to_vec(),
        )
    };

    style_interactive_lines(
        &mut visible_lines,
        &visible_sources,
        app,
        inner.width as usize,
    );
    pad_code_block_bg(&mut visible_lines, inner.width as usize);

    if let Some(selection) = &app.selection {
        let sel_start = selection.start.min(selection.end);
        let sel_end = selection.start.max(selection.end);

        for (idx, line) in visible_lines.iter_mut().enumerate() {
            let global_idx = start + idx;
            if global_idx >= sel_start.0 && global_idx <= sel_end.0 {
                let start_col = if global_idx == sel_start.0 {
                    sel_start.1
                } else {
                    0
                };
                let end_col = if global_idx == sel_end.0 {
                    sel_end.1
                } else {
                    usize::MAX
                };

                *line = highlight_selection_columns(line, start_col, end_col);
            }
        }
    }

    if app.mode == Mode::Normal {
        for (offset, source) in visible_sources.iter().enumerate() {
            let line_area = line_rect(inner, offset);
            let action = match source {
                ChatLineSource::Message(index)
                    if app
                        .messages
                        .get(*index)
                        .is_some_and(|message| message.role == ChatRole::User) =>
                {
                    // Higher-priority hits for `[Image N]` chips enable hover preview.
                    if let Some(line) = visible_lines.get(offset) {
                        crate::view::image_preview::register_chat_image_hits(
                            app, line, line_area, *index,
                        );
                    }
                    Some(HitAction::ChatMessage(*index))
                }
                ChatLineSource::ToolResult(id) if !app.full_tool_view => {
                    Some(HitAction::ToolResult(id.clone()))
                }
                ChatLineSource::ToolGroup(ids) if !ids.is_empty() => {
                    Some(HitAction::ToolGroup(ids.clone()))
                }
                ChatLineSource::Subagent(id) => Some(HitAction::Subagent(id.clone())),
                _ => None,
            };
            if let Some(action) = action {
                app.register_hit(line_area, 5, "chat", action);
            }
        }
    }

    frame.render_widget(
        Paragraph::new(Text::from(visible_lines))
            .style(Style::default().bg(bg()))
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn highlight_selection_columns(
    line: &Line<'static>,
    start_col: usize,
    end_col: usize,
) -> Line<'static> {
    if start_col >= end_col {
        return line.clone();
    }
    let mut spans = Vec::new();
    let mut current_col = 0usize;
    for span in &line.spans {
        for ch in span.content.chars() {
            let width = display_width(&ch.to_string()).max(1);
            let next_col = current_col.saturating_add(width);
            let selected = next_col > start_col && current_col < end_col;
            let style = if selected {
                span.style.bg(Color::DarkGray)
            } else {
                span.style
            };
            push_char_span(&mut spans, ch, style);
            current_col = next_col;
        }
    }
    Line::from(spans)
}

fn push_char_span(spans: &mut Vec<Span<'static>>, ch: char, style: Style) {
    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        last.content.to_mut().push(ch);
        return;
    }
    spans.push(Span::styled(ch.to_string(), style));
}

fn render_subagent_chat_area(
    frame: &mut Frame<'_>,
    app: &mut TuiApp,
    inner: Rect,
    invocation_id: &str,
) {
    let footer_height = 1;
    let body_height = inner.height.saturating_sub(footer_height);
    let body = Rect::new(inner.x, inner.y, inner.width, body_height);
    let footer = Rect::new(
        inner.x,
        inner.y.saturating_add(body_height),
        inner.width,
        footer_height,
    );
    let lines = build_subagent_lines(app, invocation_id, inner.width as usize);
    let visible_height = body.height as usize;
    let max_scroll = lines.len().saturating_sub(visible_height);
    app.scroll_offset = app.scroll_offset.min(max_scroll);
    let start = lines
        .len()
        .saturating_sub(visible_height)
        .saturating_sub(app.scroll_offset);
    let end = (start + visible_height).min(lines.len());

    frame.render_widget(
        Paragraph::new(Text::from(lines[start..end].to_vec()))
            .style(Style::default().bg(bg()))
            .wrap(Wrap { trim: false }),
        body,
    );
    frame.render_widget(
        Paragraph::new(Line::from(subagent_footer_spans(
            app,
            invocation_id,
            inner.width as usize,
        )))
        .style(Style::default().bg(panel())),
        footer,
    );
}

fn build_subagent_lines(app: &TuiApp, invocation_id: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let index = app
        .subagent_order
        .iter()
        .position(|id| id == invocation_id)
        .map(|idx| idx + 1)
        .unwrap_or(1);
    let total = app.subagent_order.len().max(1);
    let title = app
        .subagent_transcripts
        .get(invocation_id)
        .map(|transcript| transcript.title.as_str())
        .or_else(|| {
            app.tool_invocations
                .get(invocation_id)
                .and_then(|invocation| {
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
                })
        })
        .unwrap_or("Subagent");

    lines.push(Line::from(vec![
        Span::styled(
            " Subagent ",
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("({index} of {total}) "),
            Style::default().fg(muted()),
        ),
        Span::styled(
            truncate_display(title, width.saturating_sub(20).max(8)),
            Style::default().fg(text()).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(""));

    let Some(transcript) = app.subagent_transcripts.get(invocation_id) else {
        lines.push(Line::from(Span::styled(
            "Waiting for subagent events...",
            Style::default().fg(muted()).add_modifier(Modifier::ITALIC),
        )));
        return lines;
    };

    if transcript.items.is_empty() {
        lines.push(Line::from(Span::styled(
            "Waiting for subagent events...",
            Style::default().fg(muted()).add_modifier(Modifier::ITALIC),
        )));
        return lines;
    }

    for item in &transcript.items {
        let (marker, color) = match item.kind {
            SubagentTranscriptKind::ToolRequested => ("→", code_type()),
            SubagentTranscriptKind::ToolCompleted => {
                if item.ok == Some(false) {
                    ("✗", red())
                } else {
                    ("✓", code_operator())
                }
            }
            SubagentTranscriptKind::Text => {
                if item.ok == Some(false) {
                    ("✗", red())
                } else {
                    ("●", accent())
                }
            }
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker} "), Style::default().fg(color)),
            Span::styled(
                truncate_display(&item.title, width.saturating_sub(4).max(8)),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ]));
        if let Some(detail) = &item.detail
            && !detail.trim().is_empty()
        {
            lines.push(Line::from(vec![
                Span::styled("  ↳ ", Style::default().fg(ghost())),
                Span::styled(
                    truncate_display(detail, width.saturating_sub(5).max(8)),
                    Style::default().fg(muted()),
                ),
            ]));
        }
    }

    lines
}

fn subagent_footer_spans(app: &TuiApp, invocation_id: &str, width: usize) -> Vec<Span<'static>> {
    let index = app
        .subagent_order
        .iter()
        .position(|id| id == invocation_id)
        .map(|idx| idx + 1)
        .unwrap_or(1);
    let total = app.subagent_order.len().max(1);
    let left = format!("  Subagent ({index} of {total})");
    let right = "Parent up   Prev left   Next right";
    let gap = width.saturating_sub(display_width(&left) + display_width(right));
    vec![
        Span::styled(left, Style::default().fg(text()).bg(panel())),
        Span::styled(" ".repeat(gap), Style::default().fg(muted()).bg(panel())),
        Span::styled("Parent ", Style::default().fg(text()).bg(panel())),
        Span::styled("up   ", Style::default().fg(muted()).bg(panel())),
        Span::styled("Prev ", Style::default().fg(text()).bg(panel())),
        Span::styled("left   ", Style::default().fg(muted()).bg(panel())),
        Span::styled("Next ", Style::default().fg(text()).bg(panel())),
        Span::styled("right", Style::default().fg(muted()).bg(panel())),
    ]
}

fn truncate_display(value: &str, max_width: usize) -> String {
    if display_width(value) <= max_width {
        return value.to_string();
    }
    if max_width <= 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in value.chars() {
        let next = ch.len_utf8().min(2);
        if used.saturating_add(next).saturating_add(1) > max_width {
            break;
        }
        used = used.saturating_add(next);
        out.push(ch);
    }
    out.push('…');
    out
}

fn style_interactive_lines(
    lines: &mut [Line<'static>],
    sources: &[ChatLineSource],
    app: &TuiApp,
    width: usize,
) {
    for (line, source) in lines.iter_mut().zip(sources.iter()) {
        let Some((hovered, selected)) = interactive_state(app, source) else {
            continue;
        };
        if matches!(
            source,
            ChatLineSource::ToolResult(_)
                | ChatLineSource::ToolGroup(_)
                | ChatLineSource::Subagent(_)
        ) && !hovered
            && !selected
        {
            continue;
        }
        let bg = if selected {
            interactive_bg()
        } else if hovered {
            interactive_hover_bg()
        } else {
            interactive_bg()
        };
        apply_card_bg(line, width, bg, hovered || selected);
    }
}

fn interactive_state(app: &TuiApp, source: &ChatLineSource) -> Option<(bool, bool)> {
    let selected = match source {
        ChatLineSource::Message(index) => {
            if !app
                .messages
                .get(*index)
                .is_some_and(|message| message.role == ChatRole::User)
            {
                return None;
            }
            app.message_action_target == Some(*index)
        }
        ChatLineSource::ToolResult(_) | ChatLineSource::ToolGroup(_) if app.full_tool_view => {
            return None;
        }
        ChatLineSource::ToolResult(id) => app.expanded_tool_results.contains(id),
        ChatLineSource::ToolGroup(ids) => {
            !ids.is_empty() && ids.iter().any(|id| app.expanded_tool_results.contains(id))
        }
        ChatLineSource::Subagent(id) => matches!(
            &app.chat_view,
            crate::state::ChatView::Subagent { invocation_id } if invocation_id == id
        ),
        ChatLineSource::None => return None,
    };
    let hovered = app
        .hovered_chat_source
        .as_ref()
        .is_some_and(|hovered| chat_sources_match(hovered, source));
    Some((hovered, selected))
}

fn chat_sources_match(a: &ChatLineSource, b: &ChatLineSource) -> bool {
    match (a, b) {
        (ChatLineSource::Message(left), ChatLineSource::Message(right)) => left == right,
        (ChatLineSource::ToolResult(left), ChatLineSource::ToolResult(right)) => left == right,
        (ChatLineSource::ToolGroup(left), ChatLineSource::ToolGroup(right)) => left == right,
        (ChatLineSource::Subagent(left), ChatLineSource::Subagent(right)) => left == right,
        _ => false,
    }
}

fn apply_card_bg(line: &mut Line<'static>, width: usize, bg: Color, emphasize: bool) {
    let mut used = 0usize;
    for (index, span) in line.spans.iter_mut().enumerate() {
        used = used.saturating_add(display_width(&span.content));
        // Keep composer-style image chips highlighted on hover/select.
        if crate::render::markdown::is_image_tag(&span.content) {
            span.style = Style::default().bg(code_const()).fg(Color::Black);
            continue;
        }
        span.style = span.style.bg(bg);
        if emphasize && index == 0 {
            span.style = span.style.fg(signal()).add_modifier(Modifier::BOLD);
        }
    }
    if used < width {
        line.spans.push(Span::styled(
            " ".repeat(width - used),
            Style::default().fg(text()).bg(bg),
        ));
    }
}

fn pad_code_block_bg(lines: &mut [Line<'static>], width: usize) {
    let bg = code_block_bg();
    for line in lines.iter_mut() {
        let is_code = line.spans.iter().any(|span| span.style.bg == Some(bg));
        if !is_code {
            continue;
        }
        let used: usize = line.spans.iter().map(|s| display_width(&s.content)).sum();
        if used < width {
            line.spans.push(Span::styled(
                " ".repeat(width - used),
                Style::default().bg(bg),
            ));
        }
    }
}

fn ensure_chat_cache(app: &mut TuiApp, chat_width: usize) {
    let signature_hash = chat_render_signature(app);
    let expanded_signature = expanded_tool_signature(app);
    {
        let cache = app.chat_render_cache.borrow();
        if cache.width == chat_width
            && cache.full_tool_view == app.full_tool_view
            && cache.show_thinking == app.show_thinking
            && cache.compact_tool_visible_limit == app.compact_tool_visible_limit
            && cache.expanded_tool_signature == expanded_signature
            && cache.signature_hash == signature_hash
        {
            return;
        }
    }

    let (previous_line_count, can_preserve_manual_scroll, width_changed) = {
        let cache = app.chat_render_cache.borrow();
        (
            cache.lines.len(),
            cache.signature_hash != 0
                && cache.width == chat_width
                && cache.full_tool_view == app.full_tool_view
                && cache.show_thinking == app.show_thinking
                && cache.compact_tool_visible_limit == app.compact_tool_visible_limit,
            cache.width != chat_width && cache.signature_hash != 0,
        )
    };
    if width_changed {
        app.chat_render_cache.borrow_mut().tool_render_cache.clear();
    }
    let rendered = build_chat_render(app, chat_width);
    if can_preserve_manual_scroll {
        app.scroll_offset =
            anchored_scroll_offset(app.scroll_offset, previous_line_count, rendered.lines.len());
    }

    let mut cache = app.chat_render_cache.borrow_mut();
    cache.width = chat_width;
    cache.full_tool_view = app.full_tool_view;
    cache.show_thinking = app.show_thinking;
    cache.compact_tool_visible_limit = app.compact_tool_visible_limit;
    cache.expanded_tool_signature = expanded_signature;
    cache.signature_hash = signature_hash;
    cache.lines = rendered.lines;
    cache.line_sources = rendered.sources;
}

fn anchored_scroll_offset(
    scroll_offset: usize,
    previous_line_count: usize,
    next_line_count: usize,
) -> usize {
    if scroll_offset == 0 {
        return 0;
    }
    if next_line_count >= previous_line_count {
        scroll_offset.saturating_add(next_line_count - previous_line_count)
    } else {
        scroll_offset.saturating_sub(previous_line_count - next_line_count)
    }
}

fn chat_render_signature(app: &TuiApp) -> u64 {
    let mut hasher = DefaultHasher::new();
    app.full_tool_view.hash(&mut hasher);
    app.show_thinking.hash(&mut hasher);
    app.chat_view.hash(&mut hasher);
    app.theme_id.config_value().hash(&mut hasher);
    app.compact_tool_visible_limit.hash(&mut hasher);
    if app.is_loading
        || !app.running_tools.is_empty()
        || app.background_commands.iter().any(|c| c.is_running())
    {
        (app.tick() % 8).hash(&mut hasher);
        app.loading_start
            .map(|start| start.elapsed().as_secs())
            .hash(&mut hasher);
    }
    let mut running_tools = app.running_tools.values().collect::<Vec<_>>();
    running_tools.sort_by(|left, right| left.id.cmp(&right.id));
    for invocation in running_tools {
        invocation.id.hash(&mut hasher);
        invocation.tool_name.hash(&mut hasher);
        invocation.input.to_string().hash(&mut hasher);
    }
    let mut subagent_activity = app.subagent_activity.iter().collect::<Vec<_>>();
    subagent_activity.sort_by(|left, right| left.0.cmp(right.0));
    for (invocation_id, message) in subagent_activity {
        invocation_id.hash(&mut hasher);
        message.hash(&mut hasher);
    }
    for msg in &app.messages {
        msg.role.hash(&mut hasher);
        msg.content.hash(&mut hasher);
        msg.images.len().hash(&mut hasher);
        msg.image_labels.hash(&mut hasher);
        msg.thinking_content.hash(&mut hasher);
        msg.status.hash(&mut hasher);
        msg.usage_label.hash(&mut hasher);
        msg.elapsed_ms.hash(&mut hasher);
        msg.model_label.hash(&mut hasher);
        msg.provider_label.hash(&mut hasher);
        msg.is_compact_summary.hash(&mut hasher);
        if let Some(result) = &msg.tool_result {
            result.ok.hash(&mut hasher);
        }
    }
    // Include background command state so chat re-renders when they update
    for cmd in &app.background_commands {
        cmd.task_id.hash(&mut hasher);
        cmd.status.hash(&mut hasher);
        cmd.elapsed_ms.hash(&mut hasher);
        cmd.stdout.len().hash(&mut hasher);
        cmd.stderr.len().hash(&mut hasher);
    }
    hasher.finish()
}

fn expanded_tool_signature(app: &TuiApp) -> String {
    let mut ids = app.expanded_tool_results.iter().collect::<Vec<_>>();
    ids.sort();
    ids.into_iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
pub(super) fn build_chat_lines(app: &mut TuiApp, chat_width: usize) -> Vec<Line<'static>> {
    build_chat_render(app, chat_width).lines
}

fn build_chat_render(
    app: &mut TuiApp,
    chat_width: usize,
) -> crate::render::markdown::ChatRenderOutput {
    build_chat_render_for_messages(
        &app.messages,
        chat_width,
        app.full_tool_view,
        app.show_thinking,
        app.compact_tool_visible_limit,
        &app.expanded_tool_results,
        &app.running_tools,
        &app.subagent_activity,
        &mut app.chat_render_cache.borrow_mut().tool_render_cache,
        app.loading_start
            .map(|start| start.elapsed().as_millis() as u64),
    )
}

#[cfg(test)]
mod tests {
    use crate::state::{ChatMessage, ChatRole};

    use super::{anchored_scroll_offset, ensure_chat_cache};

    fn line_text(line: &ratatui::prelude::Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn anchored_scroll_tracks_added_lines_when_scrolled_up() {
        assert_eq!(anchored_scroll_offset(10, 100, 105), 15);
    }

    #[test]
    fn anchored_scroll_tracks_removed_lines_when_scrolled_up() {
        assert_eq!(anchored_scroll_offset(10, 100, 94), 4);
    }

    #[test]
    fn anchored_scroll_keeps_tail_at_zero() {
        assert_eq!(anchored_scroll_offset(0, 100, 120), 0);
    }

    #[test]
    fn user_text_message_rendered_block_contains_text() {
        let mut app = crate::tests::test_app("");
        app.messages.push(ChatMessage::new(
            ChatRole::User,
            "ykdl tui ja esta funcional.".to_string(),
        ));

        ensure_chat_cache(&mut app, 80);
        let cache = app.chat_render_cache.borrow();
        let rendered = cache
            .lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("ykdl tui ja esta funcional."));
        assert_eq!(cache.lines.len(), 3);
    }

    #[test]
    fn chat_cache_invalidates_when_same_length_message_content_changes() {
        let mut app = crate::tests::test_app("");
        app.messages
            .push(ChatMessage::new(ChatRole::User, "abc".to_string()));

        ensure_chat_cache(&mut app, 80);
        app.messages[0].content = "xyz".to_string();
        ensure_chat_cache(&mut app, 80);

        let rendered = app
            .chat_render_cache
            .borrow()
            .lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("xyz"));
        assert!(!rendered.contains("abc"));
    }
}
