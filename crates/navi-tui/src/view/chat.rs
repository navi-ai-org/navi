use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use ratatui::layout::{Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::TuiApp;
use crate::render::markdown::{
    USER_IMAGE_ROW_HEIGHT, USER_IMAGES_PER_ROW, build_chat_render_for_messages,
};
use crate::render::text::display_width;
use crate::state::{ChatLineSource, ChatRole, Mode};
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};

use super::welcome::welcome_text;

pub(super) fn render_chat_area(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    app.chat_render_cache.borrow_mut().chat_rect = Some(inner);

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
                let start_char = if global_idx == sel_start.0 {
                    sel_start.1
                } else {
                    0
                };
                let end_char = if global_idx == sel_end.0 {
                    sel_end.1
                } else {
                    usize::MAX
                };

                let mut new_spans = Vec::new();
                let mut current_char = 0;
                for span in line.spans.iter() {
                    let span_len = display_width(&span.content);
                    let span_end = current_char + span_len;

                    if span_end <= start_char || current_char >= end_char {
                        new_spans.push(span.clone());
                    } else if current_char >= start_char && span_end <= end_char {
                        new_spans.push(Span::styled(
                            span.content.clone(),
                            span.style.bg(Color::DarkGray),
                        ));
                    } else {
                        let c1 = start_char.saturating_sub(current_char).min(span_len);
                        let c2 = end_char.saturating_sub(current_char).min(span_len);

                        let s: String = span.content.chars().collect();

                        if c1 > 0 {
                            let p1: String = s.chars().take(c1).collect();
                            new_spans.push(Span::styled(p1, span.style));
                        }
                        if c2 > c1 {
                            let p2: String = s.chars().skip(c1).take(c2 - c1).collect();
                            new_spans.push(Span::styled(p2, span.style.bg(Color::DarkGray)));
                        }
                        if span_len > c2 {
                            let p3: String = s.chars().skip(c2).collect();
                            new_spans.push(Span::styled(p3, span.style));
                        }
                    }
                    current_char = span_end;
                }
                *line = Line::from(new_spans);
            }
        }
    }

    frame.render_widget(
        Paragraph::new(Text::from(visible_lines))
            .style(Style::default().bg(bg()))
            .wrap(Wrap { trim: false }),
        inner,
    );

    render_chat_images(frame, app, inner, &visible_sources);

    if app.mode == Mode::Normal {
        for (offset, source) in visible_sources.into_iter().enumerate() {
            let action = match source {
                ChatLineSource::Message(index)
                | ChatLineSource::ImageRow {
                    message_index: index,
                    ..
                } if app
                    .messages
                    .get(index)
                    .is_some_and(|message| message.role == ChatRole::User) =>
                {
                    Some(HitAction::ChatMessage(index))
                }
                ChatLineSource::ToolResult(id) if !app.full_tool_view => {
                    Some(HitAction::ToolResult(id))
                }
                ChatLineSource::ToolGroup(ids) if !ids.is_empty() => {
                    Some(HitAction::ToolGroup(ids))
                }
                _ => None,
            };
            if let Some(action) = action {
                app.register_hit(line_rect(inner, offset), 5, "chat", action);
            }
        }
    }
}

fn render_chat_images(
    frame: &mut Frame<'_>,
    app: &mut TuiApp,
    inner: Rect,
    visible_sources: &[ChatLineSource],
) {
    let mut rendered_rows = Vec::new();
    for (offset, source) in visible_sources.iter().enumerate() {
        let ChatLineSource::ImageRow {
            message_index,
            start_index,
            count,
        } = *source
        else {
            continue;
        };
        let key = (message_index, start_index, count);
        if rendered_rows.contains(&key) {
            continue;
        }
        rendered_rows.push(key);

        let visible_height = visible_sources
            .iter()
            .skip(offset)
            .take(USER_IMAGE_ROW_HEIGHT)
            .take_while(|row_source| **row_source == *source)
            .count() as u16;
        if visible_height == 0 {
            continue;
        }

        let row_area = Rect::new(
            inner.x,
            inner.y.saturating_add(offset as u16),
            inner.width,
            visible_height,
        );
        render_image_row(frame, app, row_area, message_index, start_index, count);
    }
}

fn render_image_row(
    frame: &mut Frame<'_>,
    app: &mut TuiApp,
    row_area: Rect,
    message_index: usize,
    start_index: usize,
    count: usize,
) {
    let Some(message) = app.messages.get_mut(message_index) else {
        return;
    };
    if count == 0 || row_area.width <= 6 || row_area.height == 0 {
        return;
    }

    let count = count.min(USER_IMAGES_PER_ROW);
    let gap = 1;
    let available = row_area.width.saturating_sub(8);
    let thumb_width =
        18.min(available.saturating_sub((count.saturating_sub(1) as u16) * gap) / count as u16);
    let thumb_height = row_area.height.min(7);
    let total_width = thumb_width * count as u16 + gap * count.saturating_sub(1) as u16;
    let start_x = row_area.x.saturating_add(5).min(
        row_area
            .x
            .saturating_add(row_area.width.saturating_sub(total_width)),
    );
    let y = row_area.y + row_area.height.saturating_sub(thumb_height) / 2;

    for local_index in 0..count {
        let image_index = start_index + local_index;
        let Some(image) = message.images.get_mut(image_index) else {
            continue;
        };
        let image_area = Rect::new(
            start_x + local_index as u16 * (thumb_width + gap),
            y,
            thumb_width,
            thumb_height,
        );
        frame.render_widget(Block::new().style(Style::default().bg(panel())), image_area);
        if let Some(protocol) = image.protocol.as_mut() {
            frame.render_stateful_widget(ratatui_image::StatefulImage::new(), image_area, protocol);
        } else {
            frame.render_widget(
                Paragraph::new(image.label.clone())
                    .style(Style::default().fg(muted()).bg(panel()))
                    .wrap(Wrap { trim: true }),
                image_area,
            );
        }
    }
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
            ChatLineSource::ToolResult(_) | ChatLineSource::ToolGroup(_)
        ) && !hovered
            && !selected
        {
            continue;
        }
        let bg = if hovered || selected {
            interactive_hover_bg()
        } else {
            interactive_bg()
        };
        apply_card_bg(line, width, bg, hovered || selected);
    }
}

fn interactive_state(app: &TuiApp, source: &ChatLineSource) -> Option<(bool, bool)> {
    let selected = match source {
        ChatLineSource::Message(index)
        | ChatLineSource::ImageRow {
            message_index: index,
            ..
        } => {
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
        (
            ChatLineSource::ImageRow {
                message_index: left,
                ..
            },
            ChatLineSource::ImageRow {
                message_index: right,
                ..
            },
        ) => left == right,
        (
            ChatLineSource::Message(left),
            ChatLineSource::ImageRow {
                message_index: right,
                ..
            },
        )
        | (
            ChatLineSource::ImageRow {
                message_index: left,
                ..
            },
            ChatLineSource::Message(right),
        ) => left == right,
        (ChatLineSource::ToolResult(left), ChatLineSource::ToolResult(right)) => left == right,
        (ChatLineSource::ToolGroup(left), ChatLineSource::ToolGroup(right)) => left == right,
        _ => false,
    }
}

fn apply_card_bg(line: &mut Line<'static>, width: usize, bg: Color, emphasize: bool) {
    let mut used = 0usize;
    for (index, span) in line.spans.iter_mut().enumerate() {
        used = used.saturating_add(display_width(&span.content));
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
    for msg in &app.messages {
        msg.role.hash(&mut hasher);
        msg.content.len().hash(&mut hasher);
        msg.images.len().hash(&mut hasher);
        msg.image_labels.hash(&mut hasher);
        msg.thinking_content.len().hash(&mut hasher);
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
        &mut app.chat_render_cache.borrow_mut().tool_render_cache,
        app.loading_start
            .map(|start| start.elapsed().as_millis() as u64),
    )
}

#[cfg(test)]
mod tests {
    use super::anchored_scroll_offset;

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
}
