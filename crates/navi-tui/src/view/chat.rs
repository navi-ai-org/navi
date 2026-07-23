use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use ratatui::layout::{Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use navi_sdk::SubagentTranscriptKind;

use crate::TuiApp;
use crate::render::clear_modal_area;
use crate::render::markdown::build_chat_render_for_messages;
use crate::render::text::display_width;
use crate::state::{ChatLineSource, ChatView, Mode};
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};

use super::welcome::welcome_text;

/// Jump scrollback to the latest message (follow the tail).
/// Jump chat to the live end — Shift+G / Ctrl+Down.
pub(crate) fn jump_to_latest(app: &mut TuiApp) {
    app.scroll_offset = 0;
    // Drop the absolute viewport lock so the next frame follows the live end.
    {
        let mut cache = app.chat_render_cache.borrow_mut();
        cache.locked_viewport_top = None;
        cache.locked_scroll_offset = 0;
    }
    // Clear block selection so composer focus returns to the live end.
    crate::chat_blocks::clear_selected_block(app);
    app.selection = None;
}

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
    let start = resolve_chat_viewport_start(app, visible_height);
    let (mut visible_lines, visible_sources) = {
        let cache = app.chat_render_cache.borrow();
        let total_lines = cache.lines.len();
        let end = (start + visible_height).min(total_lines);
        let source_end = end.min(cache.line_sources.len());
        (
            cache.lines[start..end].to_vec(),
            cache.line_sources[start.min(source_end)..source_end].to_vec(),
        )
    };

    let rails = style_interactive_lines(
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
                ChatLineSource::Message(index) => {
                    // Higher-priority hits for `[Image N]` chips enable hover preview.
                    if let Some(line) = visible_lines.get(offset) {
                        crate::view::image_preview::register_chat_image_hits(
                            app, line, line_area, *index,
                        );
                    }
                    // Every message block is selectable (user + assistant).
                    Some(HitAction::ChatMessage(*index))
                }
                ChatLineSource::ToolResult(id) => Some(HitAction::ToolResult(id.clone())),
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

    // Paint hover/selection rails in the left margin — never inside the line
    // text, so markers like `◆ Run` do not shift when the pointer enters.
    paint_block_rails(frame, inner, &rails);

    // Floating “↓ Latest” when scrolled away from the live end.
    render_jump_to_latest_button(frame, app, inner);
}

/// Floating pill above the composer when the user has scrolled up in history.
/// Click (or Ctrl+↓ / Shift+G) jumps to the last message.
fn render_jump_to_latest_button(frame: &mut Frame<'_>, app: &mut TuiApp, chat_area: Rect) {
    if app.scroll_offset == 0 || chat_area.width < 12 || chat_area.height < 3 {
        return;
    }
    // Hide under modals; Normal / Subagent scrollback only.
    if !matches!(app.mode, Mode::Normal) {
        return;
    }

    // Show the keyboard shortcut beside the label so the affordance is discoverable.
    let label = if chat_area.width >= 28 {
        " ↓ Latest  ctrl+↓ "
    } else if chat_area.width >= 20 {
        " ↓ Latest ^↓ "
    } else {
        " ↓ Latest "
    };
    let label_w = display_width(label) as u16;
    let width = (label_w.saturating_add(2))
        .min(chat_area.width.saturating_sub(2))
        .max(8);
    let height = 3u16.min(chat_area.height);
    // Bottom-center of the chat pane (above the composer gap).
    let x = chat_area.x + chat_area.width.saturating_sub(width) / 2;
    let y = chat_area
        .y
        .saturating_add(chat_area.height.saturating_sub(height).saturating_sub(1));
    let rect = Rect::new(x, y, width, height);

    clear_modal_area(frame, rect);
    frame.render_widget(
        Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(accent()).bg(panel()))
            .style(Style::default().bg(panel())),
        rect,
    );
    let inner = rect.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    if inner.width > 0 && inner.height > 0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                label.to_string(),
                Style::default()
                    .fg(accent())
                    .bg(panel())
                    .add_modifier(Modifier::BOLD),
            )))
            .style(Style::default().bg(panel())),
            inner,
        );
    }

    // High z so it wins over chat line hits.
    app.register_hit(rect, 80, "jump to latest", HitAction::ScrollToBottom);
}

fn highlight_selection_columns(
    line: &Line<'static>,
    start_col: usize,
    end_col: usize,
) -> Line<'static> {
    if start_col >= end_col {
        return line.clone();
    }
    // Theme selection colors — DarkGray alone is invisible on many dark themes.
    let sel_bg = selection_bg();
    let sel_fg = selection_fg();
    let mut spans = Vec::new();
    let mut current_col = 0usize;
    for span in &line.spans {
        for ch in span.content.chars() {
            let width = display_width(&ch.to_string()).max(1);
            let next_col = current_col.saturating_add(width);
            let selected = next_col > start_col && current_col < end_col;
            let style = if selected {
                // Force readable contrast on the selection bar.
                span.style
                    .fg(sel_fg)
                    .bg(sel_bg)
                    .add_modifier(Modifier::BOLD)
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

/// Apply hover/selection line chrome (bg cleanup only) and return rails to
/// paint in the left margin. Rails are never inserted into the line text —
/// that used to shift `◆ Run` / `› prompt` one column on every hover.
fn style_interactive_lines(
    lines: &mut [Line<'static>],
    sources: &[ChatLineSource],
    app: &TuiApp,
    _width: usize,
) -> Vec<(usize, BlockRailTone)> {
    let mut rails = Vec::new();
    for (offset, (line, source)) in lines.iter_mut().zip(sources.iter()).enumerate() {
        let Some((hovered, block_selected, _action_selected, _soft_card)) =
            interactive_state(app, source)
        else {
            continue;
        };

        // Recap-style left rail only — no solid fill / selection_bg wash.
        // Selected wins over hover so the active block stays accent-bright.
        if block_selected {
            prepare_block_line_chrome(line);
            rails.push((offset, BlockRailTone::Selected));
        } else if hovered {
            prepare_block_line_chrome(line);
            rails.push((offset, BlockRailTone::Hovered));
        }
    }
    rails
}

/// Visual weight for the chat block rail (selection vs pointer hover).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockRailTone {
    /// Active block selection — accent rail for clear click feedback.
    Selected,
    /// Pointer over a block that is not selected — muted rail affordance.
    Hovered,
}

/// Returns (hovered, block_selected, action_selected, soft_card).
/// soft_card = true for tools that only show card chrome when interactive.
fn interactive_state(app: &TuiApp, source: &ChatLineSource) -> Option<(bool, bool, bool, bool)> {
    if matches!(source, ChatLineSource::None) {
        return None;
    }
    let block_selected = app
        .selected_chat_source
        .as_ref()
        .is_some_and(|selected| crate::chat_blocks::chat_sources_match(selected, source));
    let action_selected = match source {
        ChatLineSource::Message(index) => app.message_action_target == Some(*index),
        ChatLineSource::ToolResult(id) => {
            app.expanded_tool_results.contains(id) && !app.collapsed_tool_results.contains(id)
        }
        ChatLineSource::ToolGroup(ids) => {
            !ids.is_empty()
                && ids.iter().any(|id| {
                    app.expanded_tool_results.contains(id)
                        && !app.collapsed_tool_results.contains(id)
                })
        }
        ChatLineSource::Subagent(id) => matches!(
            &app.chat_view,
            crate::state::ChatView::Subagent { invocation_id } if invocation_id == id
        ),
        ChatLineSource::None => false,
    };
    let soft_card = matches!(
        source,
        ChatLineSource::ToolResult(_) | ChatLineSource::ToolGroup(_) | ChatLineSource::Subagent(_)
    );
    let hovered = app
        .hovered_chat_source
        .as_ref()
        .is_some_and(|hovered| crate::chat_blocks::chat_sources_match(hovered, source));
    Some((hovered, block_selected, action_selected, soft_card))
}

fn block_rail_style(tone: BlockRailTone) -> Style {
    // Selected uses accent so click feedback is obvious; hover stays muted so
    // it reads as affordance without competing with the active selection.
    let mut style = match tone {
        BlockRailTone::Selected => Style::default().fg(accent()).bg(bg()),
        BlockRailTone::Hovered => Style::default().fg(muted()).bg(bg()),
    };
    if matches!(tone, BlockRailTone::Selected) {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

/// Drop solid panel/selection fills so the external rail is the only chrome.
/// Does **not** insert glyphs — that would reflow markers like `◆ Run`.
fn prepare_block_line_chrome(line: &mut Line<'static>) {
    let chat_bg = bg();
    for span in line.spans.iter_mut() {
        if crate::render::markdown::is_image_tag(&span.content) {
            span.style = Style::default().bg(code_const()).fg(Color::Black);
            continue;
        }
        if span.style.bg == Some(code_block_bg()) {
            continue;
        }
        span.style = span.style.bg(chat_bg);
    }
}

/// Paint Recap-style `│` rails in the left margin of the chat pane.
///
/// `content` is the text rect (`area.inner` with horizontal margin ≥ 1). The
/// rail sits one cell to the left of the text so hover never shifts content.
fn paint_block_rails(frame: &mut Frame<'_>, content: Rect, rails: &[(usize, BlockRailTone)]) {
    if rails.is_empty() || content.x == 0 {
        return;
    }
    let rail_x = content.x - 1;
    let buf = frame.buffer_mut();
    let area = buf.area;
    for &(row_offset, tone) in rails {
        let y = content.y.saturating_add(row_offset as u16);
        if y >= content.bottom() || y >= area.bottom() || rail_x >= area.right() {
            continue;
        }
        let cell = &mut buf[(rail_x, y)];
        cell.set_symbol("│");
        cell.set_style(block_rail_style(tone));
    }
}

/// Extend any intentional line background (diff add/remove, etc.) to the full
/// viewport width so color does not stop at the end of the text string.
fn pad_code_block_bg(lines: &mut [Line<'static>], width: usize) {
    for line in lines.iter_mut() {
        // Prefer the rightmost non-default span bg (diff tint, not chat default).
        let line_bg = line.spans.iter().rev().find_map(|span| {
            let bg = span.style.bg?;
            if bg == crate::theme::bg() || bg == Color::Reset {
                None
            } else {
                Some(bg)
            }
        });
        let Some(bg) = line_bg else {
            continue;
        };
        let used: usize = line.spans.iter().map(|s| display_width(&s.content)).sum();
        if used < width {
            line.spans.push(Span::styled(
                " ".repeat(width - used),
                Style::default().bg(bg),
            ));
        }
    }
}

fn streaming_tail_index(app: &TuiApp) -> Option<usize> {
    if !(app.is_loading
        || !app.running_tools.is_empty()
        || app.background_commands.iter().any(|c| c.is_running()))
    {
        return None;
    }
    let last = app.messages.last()?;
    if matches!(last.status.as_deref(), Some("receiving") | Some("thinking")) {
        Some(app.messages.len().saturating_sub(1))
    } else {
        None
    }
}

fn history_prefix_signature(app: &TuiApp, history_len: usize) -> u64 {
    // Length-based fingerprint: finalized history is append-only during a stream
    // tail update, so we avoid re-hashing multi-KB content every animation frame.
    let mut hasher = DefaultHasher::new();
    app.full_tool_view.hash(&mut hasher);
    app.show_thinking.hash(&mut hasher);
    app.chat_view.hash(&mut hasher);
    app.theme_id.config_value().hash(&mut hasher);
    app.compact_tool_visible_limit.hash(&mut hasher);
    history_len.hash(&mut hasher);
    for msg in app.messages.iter().take(history_len) {
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
        msg.is_recap.hash(&mut hasher);
        msg.sent_at
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .hash(&mut hasher);
        if let Some(result) = &msg.tool_result {
            result.invocation_id.hash(&mut hasher);
            result.ok.hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn remap_tail_sources(
    sources: Vec<crate::state::ChatLineSource>,
    message_offset: usize,
) -> Vec<crate::state::ChatLineSource> {
    sources
        .into_iter()
        .map(|source| match source {
            crate::state::ChatLineSource::Message(idx) => {
                crate::state::ChatLineSource::Message(idx + message_offset)
            }
            other => other,
        })
        .collect()
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
        // Preserve bottom-relative offset across rebuilds whenever we already
        // have lines at the same width. Do not gate on `signature_hash != 0`:
        // several paths zero the hash to force a rebuild, and that used to
        // disable anchoring so streaming text yanked the viewport upward.
        (
            cache.lines.len(),
            !cache.lines.is_empty()
                && cache.width == chat_width
                && cache.full_tool_view == app.full_tool_view
                && cache.show_thinking == app.show_thinking
                && cache.compact_tool_visible_limit == app.compact_tool_visible_limit,
            cache.width != chat_width && !cache.lines.is_empty(),
        )
    };
    if width_changed {
        let mut cache = app.chat_render_cache.borrow_mut();
        cache.tool_render_cache.clear();
        cache.history_lines.clear();
        cache.history_line_sources.clear();
        cache.history_message_count = 0;
        cache.history_signature = 0;
        // Line indices are meaningless after a reflow — re-derive from offset.
        cache.locked_viewport_top = None;
    }

    // Incremental path: reuse finalized history markdown while streaming tail updates.
    if let Some(tail_idx) = streaming_tail_index(app) {
        let history_sig = history_prefix_signature(app, tail_idx);
        let can_reuse_history = {
            let cache = app.chat_render_cache.borrow();
            cache.width == chat_width
                && cache.full_tool_view == app.full_tool_view
                && cache.show_thinking == app.show_thinking
                && cache.compact_tool_visible_limit == app.compact_tool_visible_limit
                && cache.expanded_tool_signature == expanded_signature
                && cache.history_message_count == tail_idx
                && cache.history_signature == history_sig
                && !cache.history_lines.is_empty()
        };

        if can_reuse_history || tail_idx > 0 {
            let history_render = if can_reuse_history {
                None
            } else {
                Some(build_chat_render_for_messages(
                    &app.messages[..tail_idx],
                    chat_width,
                    app.full_tool_view,
                    app.show_thinking,
                    app.compact_tool_visible_limit,
                    &app.expanded_tool_results,
                    &app.collapsed_tool_results,
                    &app.running_tools,
                    &app.subagent_activity,
                    &mut app.chat_render_cache.borrow_mut().tool_render_cache,
                    app.loading_start
                        .map(|start| start.elapsed().as_millis() as u64),
                ))
            };

            let tail_render = build_chat_render_for_messages(
                &app.messages[tail_idx..],
                chat_width,
                app.full_tool_view,
                app.show_thinking,
                app.compact_tool_visible_limit,
                &app.expanded_tool_results,
                &app.collapsed_tool_results,
                &app.running_tools,
                &app.subagent_activity,
                &mut app.chat_render_cache.borrow_mut().tool_render_cache,
                app.loading_start
                    .map(|start| start.elapsed().as_millis() as u64),
            );
            let tail_sources = remap_tail_sources(tail_render.sources, tail_idx);

            let mut cache = app.chat_render_cache.borrow_mut();
            if let Some(history) = history_render {
                cache.history_lines = history.lines;
                cache.history_line_sources = history.sources;
                cache.history_message_count = tail_idx;
                cache.history_signature = history_sig;
            }

            let mut lines = cache.history_lines.clone();
            let mut sources = cache.history_line_sources.clone();
            if !lines.is_empty() && !tail_render.lines.is_empty() {
                lines.push(Line::from(""));
                sources.push(ChatLineSource::None);
            }
            lines.extend(tail_render.lines);
            sources.extend(tail_sources);

            if can_preserve_manual_scroll {
                app.scroll_offset =
                    anchored_scroll_offset(app.scroll_offset, previous_line_count, lines.len());
            }

            cache.width = chat_width;
            cache.full_tool_view = app.full_tool_view;
            cache.show_thinking = app.show_thinking;
            cache.compact_tool_visible_limit = app.compact_tool_visible_limit;
            cache.expanded_tool_signature = expanded_signature;
            cache.signature_hash = signature_hash;
            cache.lines = lines;
            cache.line_sources = sources;
            return;
        }
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
    // Idle/full rebuild: treat entire transcript as history prefix.
    cache.history_message_count = app.messages.len();
    cache.history_signature = history_prefix_signature(app, app.messages.len());
    cache.history_lines = rendered.lines.clone();
    cache.history_line_sources = rendered.sources.clone();
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

/// Absolute top line currently shown in the chat pane (read-only).
///
/// Prefers the render lock when it still matches `scroll_offset` so mouse hit
/// tests align with what was painted. Falls back to bottom-relative math.
pub(crate) fn chat_viewport_start(app: &TuiApp, visible_height: usize) -> usize {
    let cache = app.chat_render_cache.borrow();
    let total_lines = cache.lines.len();
    let max_start = total_lines.saturating_sub(visible_height);
    if app.scroll_offset == 0 {
        return max_start;
    }
    if let Some(top) = cache.locked_viewport_top
        && cache.locked_scroll_offset == app.scroll_offset
    {
        return top.min(max_start);
    }
    let effective_scroll = app.scroll_offset.min(max_start);
    max_start.saturating_sub(effective_scroll)
}

/// Resolve the absolute top line of the chat viewport for the next paint.
///
/// `scroll_offset` is distance from the live end (0 = stick to bottom). While
/// the user is scrolled up we lock the absolute top line across frames so
/// streaming appends and composer height changes do not shove the text they
/// are reading/copying.
fn resolve_chat_viewport_start(app: &mut TuiApp, visible_height: usize) -> usize {
    let total_lines = app.chat_render_cache.borrow().lines.len();
    let max_start = total_lines.saturating_sub(visible_height);

    if app.scroll_offset == 0 {
        let mut cache = app.chat_render_cache.borrow_mut();
        cache.locked_viewport_top = None;
        cache.locked_scroll_offset = 0;
        return max_start;
    }

    let start = chat_viewport_start(app, visible_height);

    // Keep scroll_offset in sync with the locked top so jump-to-latest, keys,
    // and the "↓ Latest" affordance stay correct after content/height changes.
    app.scroll_offset = max_start.saturating_sub(start);
    {
        let mut cache = app.chat_render_cache.borrow_mut();
        cache.locked_viewport_top = Some(start);
        cache.locked_scroll_offset = app.scroll_offset;
    }
    start
}

fn chat_render_signature(app: &TuiApp) -> u64 {
    let mut hasher = DefaultHasher::new();
    app.full_tool_view.hash(&mut hasher);
    app.show_thinking.hash(&mut hasher);
    app.chat_view.hash(&mut hasher);
    app.theme_id.config_value().hash(&mut hasher);
    app.compact_tool_visible_limit.hash(&mut hasher);
    let activity_animating = app.is_loading
        || !app.running_tools.is_empty()
        || !app.subagent_activity.is_empty()
        || app.background_commands.iter().any(|c| c.is_running());
    // Invalidate once per pulse frame so ◆/◇ advances while tools/commands run.
    if activity_animating {
        let elapsed_ms = app
            .loading_start
            .map(|start| start.elapsed().as_millis() as u64)
            .unwrap_or_else(|| app.tick().saturating_mul(80));
        crate::render::status::running_pulse_frame(elapsed_ms).hash(&mut hasher);
        // Also track wall clock seconds for elapsed labels (· 3s).
        (elapsed_ms / 1000).hash(&mut hasher);
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

    // While streaming/loading: hash finalized history by count + lengths only,
    // and fully hash the streaming tail. Idle/finalized: full content hash.
    let message_count = app.messages.len();
    message_count.hash(&mut hasher);
    let streaming_tail = activity_animating
        && app
            .messages
            .last()
            .is_some_and(|m| matches!(m.status.as_deref(), Some("receiving") | Some("thinking")));
    for (i, msg) in app.messages.iter().enumerate() {
        let is_tail = streaming_tail && i + 1 == message_count;
        msg.role.hash(&mut hasher);
        msg.images.len().hash(&mut hasher);
        msg.image_labels.hash(&mut hasher);
        msg.status.hash(&mut hasher);
        msg.usage_label.hash(&mut hasher);
        msg.elapsed_ms.hash(&mut hasher);
        msg.model_label.hash(&mut hasher);
        msg.provider_label.hash(&mut hasher);
        msg.is_compact_summary.hash(&mut hasher);
        msg.is_recap.hash(&mut hasher);
        msg.sent_at
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .hash(&mut hasher);
        if let Some(result) = &msg.tool_result {
            result.ok.hash(&mut hasher);
        }
        if is_tail || !streaming_tail {
            // Full content for the live tail, or for every message when idle.
            msg.content.hash(&mut hasher);
            msg.thinking_content.hash(&mut hasher);
        } else {
            // Stable finalized prefix: length-only keeps signature cheap.
            msg.content.len().hash(&mut hasher);
            msg.thinking_content.len().hash(&mut hasher);
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
    let mut open = app
        .expanded_tool_results
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    open.sort();
    let mut closed = app
        .collapsed_tool_results
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    closed.sort();
    format!(
        "full={} open={} closed={}",
        app.full_tool_view,
        open.join(","),
        closed.join(",")
    )
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
        &app.collapsed_tool_results,
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

    use super::{anchored_scroll_offset, ensure_chat_cache, resolve_chat_viewport_start};

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
    fn jump_to_latest_clears_scroll_and_selection() {
        use crate::state::ChatLineSource;
        let mut app = crate::tests::test_app("");
        app.scroll_offset = 42;
        app.chat_render_cache.borrow_mut().locked_viewport_top = Some(10);
        app.chat_render_cache.borrow_mut().locked_scroll_offset = 42;
        app.selected_chat_source = Some(ChatLineSource::Message(0));
        super::jump_to_latest(&mut app);
        assert_eq!(app.scroll_offset, 0);
        assert!(app.selected_chat_source.is_none());
        assert!(app.selection.is_none());
        assert!(app.chat_render_cache.borrow().locked_viewport_top.is_none());
    }

    #[test]
    fn anchored_scroll_keeps_tail_at_zero() {
        assert_eq!(anchored_scroll_offset(0, 100, 120), 0);
    }

    #[test]
    fn viewport_stays_put_while_streaming_when_scrolled_up() {
        use crate::state::{ChatMessage, ChatRole};

        let mut app = crate::tests::test_app("");
        for i in 0..40 {
            app.messages.push(ChatMessage::new(
                ChatRole::User,
                format!("history line {i} — padding text so wrap makes several rows"),
            ));
        }
        app.is_loading = true;
        app.loading_start = Some(std::time::Instant::now());
        app.messages.push(ChatMessage {
            status: Some("receiving".to_string()),
            ..ChatMessage::new(ChatRole::Assistant, "start".to_string())
        });

        ensure_chat_cache(&mut app, 40);
        let total_before = app.chat_render_cache.borrow().lines.len();
        assert!(
            total_before > 20,
            "expected tall transcript, got {total_before}"
        );

        app.scroll_offset = 12;
        let visible = 10usize;
        let start_before = resolve_chat_viewport_start(&mut app, visible);
        assert!(start_before > 0);
        assert_eq!(app.scroll_offset, 12);

        let top_text = {
            let cache = app.chat_render_cache.borrow();
            line_text(&cache.lines[start_before])
        };

        if let Some(last) = app.messages.last_mut() {
            last.content.push_str(&format!(
                "\n\n{}",
                "streaming paragraph that wraps into many lines. ".repeat(80)
            ));
        }
        ensure_chat_cache(&mut app, 40);
        let total_after = app.chat_render_cache.borrow().lines.len();
        assert!(
            total_after > total_before + 10,
            "expected streaming to grow transcript ({total_before} → {total_after})"
        );

        let start_after = resolve_chat_viewport_start(&mut app, visible);
        assert_eq!(
            start_after, start_before,
            "viewport top must stay locked while the model streams below"
        );
        let top_text_after = {
            let cache = app.chat_render_cache.borrow();
            line_text(&cache.lines[start_after])
        };
        assert_eq!(
            top_text_after, top_text,
            "same content must remain under the top of the viewport"
        );
        assert!(
            app.scroll_offset >= 12,
            "scroll_offset should grow with appended lines, got {}",
            app.scroll_offset
        );
    }

    #[test]
    fn viewport_stays_put_when_visible_height_changes() {
        use crate::state::{ChatMessage, ChatRole};

        let mut app = crate::tests::test_app("");
        for i in 0..30 {
            app.messages.push(ChatMessage::new(
                ChatRole::Assistant,
                format!("block {i} with enough text to occupy a full line of the transcript"),
            ));
        }
        ensure_chat_cache(&mut app, 48);
        app.scroll_offset = 8;
        let start = resolve_chat_viewport_start(&mut app, 12);
        let start_taller = resolve_chat_viewport_start(&mut app, 16);
        assert_eq!(
            start_taller, start,
            "composer height change must not shove scrolled history"
        );
    }

    #[test]
    fn follow_tail_when_scroll_offset_is_zero() {
        use crate::state::{ChatMessage, ChatRole};

        let mut app = crate::tests::test_app("");
        for i in 0..20 {
            app.messages.push(ChatMessage::new(
                ChatRole::Assistant,
                format!("tail follow {i}"),
            ));
        }
        ensure_chat_cache(&mut app, 40);
        app.scroll_offset = 0;
        let visible = 8usize;
        let total = app.chat_render_cache.borrow().lines.len();
        let start = resolve_chat_viewport_start(&mut app, visible);
        assert_eq!(start, total.saturating_sub(visible));
        assert!(app.chat_render_cache.borrow().locked_viewport_top.is_none());
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
        // Sticky user bar: single content line (`› text…` [+ optional clock]).
        assert_eq!(cache.lines.len(), 1);
        assert!(
            rendered.starts_with('›') || rendered.starts_with("› "),
            "expected › user prefix, got {rendered:?}"
        );
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

    #[test]
    fn selected_block_uses_left_rail_not_selection_bg() {
        use crate::state::ChatLineSource;
        use crate::theme::{ThemeId, accent, selection_bg, with_palette};
        use ratatui::style::{Color, Modifier};

        with_palette(&ThemeId::Lain.palette(), || {
            let mut app = crate::tests::test_app("");
            app.messages.push(ChatMessage::new(
                ChatRole::Assistant,
                "hello selected".to_string(),
            ));
            app.selected_chat_source = Some(ChatLineSource::Message(0));

            let original = "  hello selected";
            let mut lines = vec![ratatui::prelude::Line::from(vec![
                ratatui::prelude::Span::styled(
                    original,
                    ratatui::style::Style::default().fg(crate::theme::text()),
                ),
            ])];
            let sources = vec![ChatLineSource::Message(0)];
            let rails = super::style_interactive_lines(&mut lines, &sources, &app, 40);

            // Rail is scheduled for margin paint — not inserted into the line.
            assert_eq!(rails, vec![(0, super::BlockRailTone::Selected)]);
            assert_eq!(
                lines[0].spans.first().map(|s| s.content.as_ref()),
                Some(original),
                "selection must not reflow line text"
            );
            let style = super::block_rail_style(super::BlockRailTone::Selected);
            assert_eq!(
                style.fg,
                Some(accent()),
                "selected rail should use accent for clear click feedback"
            );
            assert!(
                style.add_modifier.contains(Modifier::BOLD),
                "selected rail should be bold"
            );
            let bad = selection_bg();
            for span in &lines[0].spans {
                assert_ne!(
                    span.style.bg,
                    Some(bad),
                    "selection must not paint selection_bg on {:?}",
                    span.content.as_ref()
                );
                // No light lavender fill — chat bg or code block only.
                if let Some(Color::Rgb(r, g, b)) = span.style.bg {
                    // Lain selection_bg is (236, 232, 255) — reject near-white lavenders.
                    assert!(
                        !(r > 200 && g > 200 && b > 200),
                        "unexpected light bg on selected span {:?}",
                        span.content.as_ref()
                    );
                }
            }
        });
    }

    #[test]
    fn hovered_block_uses_muted_rail_without_selection_bg() {
        use crate::state::ChatLineSource;
        use crate::theme::{ThemeId, muted, selection_bg, with_palette};
        use ratatui::style::Modifier;

        with_palette(&ThemeId::Lain.palette(), || {
            let mut app = crate::tests::test_app("");
            app.messages.push(ChatMessage::new(
                ChatRole::Assistant,
                "hello hover".to_string(),
            ));
            app.hovered_chat_source = Some(ChatLineSource::Message(0));

            let original = "  hello hover";
            let mut lines = vec![ratatui::prelude::Line::from(vec![
                ratatui::prelude::Span::styled(
                    original,
                    ratatui::style::Style::default().fg(crate::theme::text()),
                ),
            ])];
            let sources = vec![ChatLineSource::Message(0)];
            let rails = super::style_interactive_lines(&mut lines, &sources, &app, 40);

            assert_eq!(rails, vec![(0, super::BlockRailTone::Hovered)]);
            assert_eq!(
                lines[0].spans.first().map(|s| s.content.as_ref()),
                Some(original),
                "hover must not reflow line text"
            );
            let style = super::block_rail_style(super::BlockRailTone::Hovered);
            assert_eq!(
                style.fg,
                Some(muted()),
                "hover rail should stay muted so it does not compete with selection"
            );
            assert!(
                !style.add_modifier.contains(Modifier::BOLD),
                "hover rail should not be bold"
            );
            let bad = selection_bg();
            for span in &lines[0].spans {
                assert_ne!(
                    span.style.bg,
                    Some(bad),
                    "hover must not paint selection_bg on {:?}",
                    span.content.as_ref()
                );
            }
        });
    }

    #[test]
    fn selected_rail_wins_over_hover_on_same_block() {
        use crate::state::ChatLineSource;
        use crate::theme::{ThemeId, with_palette};

        with_palette(&ThemeId::Lain.palette(), || {
            let mut app = crate::tests::test_app("");
            app.messages
                .push(ChatMessage::new(ChatRole::User, "both states".to_string()));
            app.selected_chat_source = Some(ChatLineSource::Message(0));
            app.hovered_chat_source = Some(ChatLineSource::Message(0));

            let original = "  both states";
            let mut lines = vec![ratatui::prelude::Line::from(vec![
                ratatui::prelude::Span::styled(
                    original,
                    ratatui::style::Style::default().fg(crate::theme::text()),
                ),
            ])];
            let sources = vec![ChatLineSource::Message(0)];
            let rails = super::style_interactive_lines(&mut lines, &sources, &app, 40);

            assert_eq!(
                rails,
                vec![(0, super::BlockRailTone::Selected)],
                "selection must win over hover"
            );
            assert_eq!(
                lines[0].spans.first().map(|s| s.content.as_ref()),
                Some(original),
                "combined hover+select must not reflow text"
            );
        });
    }

    #[test]
    fn hover_rail_does_not_shift_tool_marker_lines() {
        use crate::state::ChatLineSource;
        use crate::theme::{ThemeId, with_palette};

        with_palette(&ThemeId::Lain.palette(), || {
            let mut app = crate::tests::test_app("");
            app.hovered_chat_source = Some(ChatLineSource::ToolResult("t1".into()));

            // Tool lines start with `◆ ` — the old in-line rail inserted `│`
            // and shoved "Run" one column to the right.
            let original_prefix = "◆ ";
            let original_action = "Run";
            let mut lines = vec![ratatui::prelude::Line::from(vec![
                ratatui::prelude::Span::styled(
                    original_prefix,
                    ratatui::style::Style::default().fg(crate::theme::accent()),
                ),
                ratatui::prelude::Span::styled(
                    original_action,
                    ratatui::style::Style::default().fg(crate::theme::text()),
                ),
            ])];
            let sources = vec![ChatLineSource::ToolResult("t1".into())];
            let rails = super::style_interactive_lines(&mut lines, &sources, &app, 40);

            assert_eq!(rails, vec![(0, super::BlockRailTone::Hovered)]);
            assert_eq!(lines[0].spans.len(), 2);
            assert_eq!(lines[0].spans[0].content.as_ref(), original_prefix);
            assert_eq!(lines[0].spans[1].content.as_ref(), original_action);
            let joined: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(joined, "◆ Run");
            assert!(
                !joined.contains('│'),
                "rail must not be embedded in tool text: {joined}"
            );
        });
    }
}
