use ratatui::layout::{Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Color, Style};
use ratatui::text::Text;
use ratatui::widgets::{Paragraph, Wrap};

use crate::TuiApp;
use crate::render::build_chat_lines_for_messages;
use crate::state::ChatRole;
use crate::theme::*;

use super::welcome::welcome_text;

pub(super) fn render_chat_area(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    app.chat_render_cache.borrow_mut().chat_rect = Some(inner);

    if app.messages.is_empty() && !app.is_loading {
        let welcome = welcome_text(app, inner.width as usize);
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
    let cache = app.chat_render_cache.borrow();
    let rendered_lines = &cache.lines;

    let visible_height = inner.height as usize;
    let total_lines = rendered_lines.len();
    let max_scroll = total_lines.saturating_sub(visible_height);
    let effective_scroll = app.scroll_offset.min(max_scroll);
    let start = total_lines
        .saturating_sub(visible_height)
        .saturating_sub(effective_scroll);
    let end = (start + visible_height).min(total_lines);

    let mut visible_lines: Vec<Line<'static>> = rendered_lines[start..end].to_vec();

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
                    let span_len = span.content.chars().count();
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
}

fn ensure_chat_cache(app: &mut TuiApp, chat_width: usize) {
    let signature = chat_render_signature(app);
    {
        let cache = app.chat_render_cache.borrow();
        if cache.width == chat_width
            && cache.full_tool_view == app.full_tool_view
            && cache.show_thinking == app.show_thinking
            && cache.compact_tool_visible_limit == app.compact_tool_visible_limit
            && cache.signature == signature
        {
            return;
        }
    }

    let (previous_line_count, can_preserve_manual_scroll) = {
        let cache = app.chat_render_cache.borrow();
        (
            cache.lines.len(),
            !cache.signature.is_empty()
                && cache.width == chat_width
                && cache.full_tool_view == app.full_tool_view
                && cache.show_thinking == app.show_thinking
                && cache.compact_tool_visible_limit == app.compact_tool_visible_limit,
        )
    };
    let lines = build_chat_lines(app, chat_width);
    if can_preserve_manual_scroll {
        app.scroll_offset =
            anchored_scroll_offset(app.scroll_offset, previous_line_count, lines.len());
    }

    let mut cache = app.chat_render_cache.borrow_mut();
    cache.width = chat_width;
    cache.full_tool_view = app.full_tool_view;
    cache.show_thinking = app.show_thinking;
    cache.compact_tool_visible_limit = app.compact_tool_visible_limit;
    cache.signature = signature;
    cache.lines = lines;
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

fn chat_render_signature(app: &TuiApp) -> String {
    let mut signature = String::with_capacity(app.messages.len() * 48);
    signature.push_str(if app.full_tool_view {
        "full|"
    } else {
        "compact|"
    });
    signature.push_str(if app.show_thinking { "think|" } else { "hide|" });
    signature.push_str(app.theme_id.config_value());
    signature.push('|');
    signature.push_str(&app.compact_tool_visible_limit.to_string());
    signature.push('|');
    for msg in &app.messages {
        signature.push(match msg.role {
            ChatRole::User => 'u',
            ChatRole::Assistant => 'a',
        });
        signature.push(':');
        signature.push_str(&msg.content.len().to_string());
        signature.push(':');
        signature.push_str(&msg.thinking_content.len().to_string());
        signature.push(':');
        signature.push_str(msg.status.as_deref().unwrap_or_default());
        signature.push(':');
        signature.push_str(msg.usage_label.as_deref().unwrap_or_default());
        signature.push(':');
        signature.push_str(&msg.elapsed_ms.unwrap_or_default().to_string());
        signature.push(':');
        signature.push_str(msg.model_label.as_deref().unwrap_or_default());
        signature.push(':');
        signature.push_str(msg.provider_label.as_deref().unwrap_or_default());
        if msg.is_compact_summary {
            signature.push_str(":compact");
        }
        if let Some(result) = &msg.tool_result {
            signature.push(':');
            signature.push_str(if result.ok { "ok" } else { "err" });
        }
        signature.push('|');
    }
    signature
}

pub(super) fn build_chat_lines(app: &TuiApp, chat_width: usize) -> Vec<Line<'static>> {
    build_chat_lines_for_messages(
        app.messages.iter(),
        chat_width,
        app.full_tool_view,
        app.show_thinking,
        app.compact_tool_visible_limit,
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
