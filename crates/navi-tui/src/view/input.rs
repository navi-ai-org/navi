use ratatui::layout::{Alignment, Margin, Position, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::TuiApp;
use crate::input::{
    COMPOSER_COLLAPSED_LINES, COMPOSER_MAX_VISIBLE_LINES, input_visual_line_count,
    input_visual_line_ranges, selected_input_range,
};
use crate::render::text::display_width;
use crate::state::ChatRole;
use crate::theme::*;
use crate::ui::floor_char_boundary;
use crate::ui::interaction::HitAction;
use navi_core::PermissionMode;

/// prompt prefix (`› `) before draft text.
const PROMPT: &str = "› ";
const PROMPT_WIDTH: usize = 2;
/// Horizontal inset inside the rounded border (left/right padding).
const INNER_PAD: u16 = 1;
/// Row below the box for model · permission (not inside the draft).
const META_ROW: u16 = 1;
/// Lerp factor per tick toward target height (soft resize).
const COMPOSER_ANIM_LERP: f32 = 0.42;
/// Snap when within this many rows of the target.
const COMPOSER_ANIM_EPS: f32 = 0.08;

/// `collapse_unfocused`: composer is expanded only while the prompt has focus
/// (Normal mode, no scrollback block selected, main chat view).
pub(crate) fn composer_is_focused(app: &TuiApp) -> bool {
    app.mode == crate::state::Mode::Normal
        && app.selected_chat_source.is_none()
        && matches!(app.chat_view, crate::state::ChatView::Parent)
}

/// Desired content-line count (inside the rounded box, excluding borders).
pub(crate) fn composer_target_content_lines(app: &TuiApp, input_width: usize) -> usize {
    if !composer_is_focused(app) {
        return COMPOSER_COLLAPSED_LINES;
    }
    let wrap_width = input_width.saturating_sub(6).max(8);
    input_visual_line_count(&app.input, wrap_width).clamp(1, COMPOSER_MAX_VISIBLE_LINES)
}

/// Advance animated height toward the target. Returns true if still animating.
pub(crate) fn advance_composer_animation(app: &mut TuiApp, input_width: usize) -> bool {
    let target = composer_target_content_lines(app, input_width) as f32;
    let cur = app.composer_anim_lines;
    if (cur - target).abs() <= COMPOSER_ANIM_EPS {
        if cur != target {
            app.composer_anim_lines = target;
        }
        return false;
    }
    // Exponential ease toward target.
    app.composer_anim_lines = cur + (target - cur) * COMPOSER_ANIM_LERP;
    true
}

pub(crate) fn render_input(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let outer = area.inner(Margin {
        horizontal: 1,
        vertical: 0,
    });
    // Transparent — same as chat bg. No elevated panel fill.
    let surface = bg();

    // Reserve bottom row for meta (outside the draft box).
    let box_height = outer.height.saturating_sub(META_ROW).max(1);
    let box_area = Rect::new(outer.x, outer.y, outer.width, box_height);
    let meta_area = Rect::new(
        outer.x,
        outer.y.saturating_add(box_height),
        outer.width,
        if outer.height > box_height {
            META_ROW
        } else {
            0
        },
    );

    // Thin rounded border only — no fill behind the draft.
    let block = Block::new()
        .borders(Borders::ALL)
        .border_set(ratatui::symbols::border::ROUNDED)
        .border_style(Style::default().fg(ghost()).bg(surface))
        .style(Style::default().fg(text()).bg(surface));

    let bordered = block.inner(box_area);
    frame.render_widget(block, box_area);

    if bordered.width == 0 || bordered.height == 0 {
        return;
    }

    let content = bordered.inner(Margin {
        horizontal: INNER_PAD.min(bordered.width.saturating_sub(1) / 2),
        vertical: 0,
    });
    if content.width == 0 || content.height == 0 {
        return;
    }

    // Draft only inside the box — no model/permission chrome mixed into the text.
    let wrap_width = (content.width as usize).saturating_sub(PROMPT_WIDTH).max(1);

    app.input_wrap_width = wrap_width;
    let focused = composer_is_focused(app);
    let (raw_lines, cursor_line, cursor_column) = input_lines(app, wrap_width);
    let ranges = crate::input::input_visual_line_ranges(&app.input, wrap_width);
    let content_h = content.height as usize;

    // Collapse: when unfocused, only show a single summary line (first line
    // of the draft, truncated with … if more content exists).
    let (visible_raw, visible_start, collapsed_summary) = if focused {
        let (vis, start) = visible_input_lines(raw_lines, content_h, cursor_line);
        (vis, start, None)
    } else {
        let total = raw_lines.len().max(1);
        let first = raw_lines.first().cloned().unwrap_or_else(|| Line::from(""));
        let more = total > 1 || app.input.contains('\n');
        (vec![first], 0, if more { Some(total) } else { None })
    };

    // Paint `› ` on first draft line; indent continuations. No bg on spans.
    let mut painted: Vec<Line<'static>> = Vec::with_capacity(visible_raw.len());
    for (i, line) in visible_raw.into_iter().enumerate() {
        let global = visible_start + i;
        let is_first = global == 0;
        let mut spans = Vec::new();
        spans.push(Span::styled(
            if is_first {
                PROMPT.to_string()
            } else {
                " ".repeat(PROMPT_WIDTH)
            },
            Style::default()
                .fg(if is_first { user_accent() } else { ghost() })
                .add_modifier(if is_first {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ));
        spans.extend(line.spans);
        if is_first && collapsed_summary.is_some() {
            spans.push(Span::styled(
                " …",
                Style::default().fg(ghost()).add_modifier(Modifier::ITALIC),
            ));
        }
        painted.push(Line::from(spans));
    }
    if painted.is_empty() {
        painted.push(Line::from(vec![Span::styled(
            PROMPT.to_string(),
            Style::default()
                .fg(user_accent())
                .add_modifier(Modifier::BOLD),
        )]));
    }

    // Register hover hits for `[Image N]` chips before paint.
    if app.mode == crate::state::Mode::Normal && !app.pending_images.is_empty() {
        let input_owned = app.input.clone();
        for (visible_row, line_index) in (visible_start..visible_start + painted.len()).enumerate()
        {
            let Some((start, end)) = ranges.get(line_index).copied() else {
                continue;
            };
            if end > input_owned.len() || start > end {
                continue;
            }
            let line_text = input_owned[start..end].to_string();
            let line_area = Rect::new(
                content.x.saturating_add(PROMPT_WIDTH as u16),
                content.y.saturating_add(visible_row as u16),
                content.width.saturating_sub(PROMPT_WIDTH as u16),
                1,
            );
            crate::view::image_preview::register_pending_image_hits(
                app,
                &input_owned,
                start,
                &line_text,
                line_area,
            );
        }
    }

    frame.render_widget(
        Paragraph::new(Text::from(painted)).style(Style::default().fg(text()).bg(surface)),
        content,
    );

    // Cursor only when the composer has focus .
    if focused {
        let cursor_x = content.x.saturating_add(
            (PROMPT_WIDTH + cursor_column).min(content.width.saturating_sub(1) as usize) as u16,
        );
        let cursor_y = content.y.saturating_add(
            cursor_line
                .saturating_sub(visible_start)
                .min(content.height.saturating_sub(1) as usize) as u16,
        );
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }

    // Meta lives outside the draft box (right-aligned), transparent bg.
    if meta_area.height > 0 && meta_area.width > 0 {
        let built = composer_meta_right(app, meta_area.width as usize);
        frame.render_widget(
            Paragraph::new(built.line)
                .alignment(Alignment::Right)
                .style(Style::default().bg(surface)),
            meta_area,
        );

        // Context chip: hover reveals window %; click opens usage modal.
        if let Some((offset_cols, chip_w)) = built.context_range {
            let full_w = built.display_width;
            let start_x = meta_area
                .x
                .saturating_add(meta_area.width.saturating_sub(full_w as u16))
                .saturating_add(offset_cols as u16);
            let hit_w = (chip_w as u16).min(
                meta_area
                    .width
                    .saturating_sub(start_x.saturating_sub(meta_area.x)),
            );
            if hit_w > 0 {
                app.register_hit(
                    Rect::new(start_x, meta_area.y, hit_w, 1),
                    20,
                    "context usage",
                    HitAction::ContextUsage,
                );
            }
        }

        if !app.queued_user_messages.is_empty() {
            let queued_width = queued_footer_label(app).len() as u16;
            app.register_hit(
                Rect::new(
                    meta_area.x,
                    meta_area.y,
                    queued_width.min(meta_area.width),
                    1,
                ),
                4,
                "open message queue",
                HitAction::OpenMessageQueue,
            );
        }
        if app.available_update.is_some() {
            // Click anywhere on the meta strip that starts with the update chip.
            // Precise chip geometry is approximate; whole-strip fallback is fine.
            app.register_hit(
                meta_area,
                5,
                "open update available",
                HitAction::OpenUpdateAvailable,
            );
        }
        if !app.pending_questions.is_empty() {
            app.register_hit(
                meta_area,
                3,
                "reopen pending question",
                HitAction::ReopenQuestion,
            );
        }
    }
}

pub(crate) fn composer_height(app: &TuiApp, input_width: usize) -> u16 {
    // Use animated content-line count .
    let content_lines = app
        .composer_anim_lines
        .round()
        .clamp(1.0, COMPOSER_MAX_VISIBLE_LINES as f32) as u16;
    // top border + draft lines + bottom border + meta row below the box.
    let _ = input_width; // wrap width is considered when advancing animation.
    2 + content_lines + META_ROW
}

pub(crate) fn composer_hint_height(app: &TuiApp) -> u16 {
    // Goal line only — plan progress lives in the topbar above chat.
    if app.goal_state.is_some() { 1 } else { 0 }
}

pub(crate) fn composer_activity_height(app: &TuiApp) -> u16 {
    if composer_activity_line(app, 1).is_some() {
        3
    } else {
        0
    }
}

pub(crate) fn render_input_activity(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    if area.height == 0 {
        return;
    }
    frame.render_widget(Block::new().style(Style::default().bg(bg())), area);
    let line_y = if area.height >= 3 {
        area.y.saturating_add(1)
    } else {
        area.y
    };
    let activity_area = Rect::new(
        area.x.saturating_add(3),
        line_y,
        area.width.saturating_sub(4),
        1,
    );
    let Some(activity) = composer_activity_line(app, activity_area.width as usize) else {
        return;
    };
    frame.render_widget(
        Paragraph::new(activity).style(Style::default().bg(bg())),
        activity_area,
    );
}

pub(crate) fn render_input_hint(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    if area.height == 0 {
        return;
    }

    // Permanent shortcut hints were removed — they cluttered every frame.
    // Shortcuts live in Help (`?` / ctrl+.). Plan progress is the topbar.
    // Goal line (if any) still sits above the composer.
    if let Some(goal_line) = composer_goal_line(app, area.width as usize) {
        let goal_area = Rect::new(area.x, area.y, area.width, 1);
        frame.render_widget(
            Paragraph::new(goal_line).style(Style::default().bg(bg())),
            goal_area,
        );
    }
}

fn composer_goal_line(app: &TuiApp, width: usize) -> Option<Line<'static>> {
    let goal = app.goal_state.as_ref()?;
    let label = goal
        .short_description
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&goal.objective);
    let mut text_value = format!("goal {}", label.trim());
    if let Some(budget) = goal.token_budget {
        let percent = (goal.tokens_used as f64 / budget as f64 * 100.0).round() as i32;
        text_value.push_str(&format!(" ({percent}%)"));
    }
    let available = width.saturating_sub(4).max(1);
    Some(Line::from(vec![Span::styled(
        format!("  {}", fit_display_width(&text_value, available)),
        Style::default().fg(text()).bg(bg()),
    )]))
}

fn composer_activity_line(app: &TuiApp, width: usize) -> Option<Line<'static>> {
    if !app.is_loading || !app.provider_configured {
        return None;
    }
    let elapsed_ms = app
        .loading_start
        .map(|start| start.elapsed().as_millis() as u64)
        .unwrap_or(0);
    let (status, color) = composer_activity_status(app);
    let elapsed = format_activity_elapsed(elapsed_ms);
    let suffix = format!(" · {elapsed}");
    // diamond pulse while a turn is running — no corner trail.
    let diamond = crate::render::status::running_diamond(elapsed_ms);
    let status_width = width.saturating_sub(display_width(diamond) + display_width(&suffix) + 2);
    let status = fit_display_width(&status, status_width.max(1));

    Some(Line::from(vec![
        Span::styled(
            diamond,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::default().fg(ghost())),
        Span::styled(status, Style::default().fg(text())),
        Span::styled(suffix, Style::default().fg(code_number())),
    ]))
}

/// Activity label driven by real agent state (approvals, tools, stream status).
/// Avoids time-based phase claims that don't match what the model is doing.
fn composer_activity_status(app: &TuiApp) -> (String, ratatui::style::Color) {
    if !app.pending_approvals.is_empty() {
        let label = if app.pending_approvals.len() == 1 {
            let id = &app.pending_approvals[0].id;
            app.tool_invocations
                .get(id)
                .map(|invocation| tool_status_label(&invocation.tool_name))
                .unwrap_or_else(|| "tool".to_string())
        } else {
            format!("{} tools", app.pending_approvals.len())
        };
        return (format!("Waiting for approval: {label}"), code_const());
    }

    if !app.pending_questions.is_empty() {
        return ("Waiting for input".to_string(), code_const());
    }

    if !app.running_tools.is_empty() {
        return (running_tools_status(app), code_operator());
    }

    if let Some(status) = background_subagent_status(app) {
        return (status, code_operator());
    }

    let active = active_assistant_message(app);
    let status = active.and_then(|message| message.status.as_deref());
    match status {
        Some("receiving") => ("Streaming response".to_string(), accent()),
        Some("retrying") => ("Retrying".to_string(), code_operator()),
        Some(label) if label.starts_with("tool:") => {
            let tool = label.trim_start_matches("tool:").trim();
            if tool.is_empty() {
                ("Running tool".to_string(), code_operator())
            } else {
                (
                    format!("Running {}", tool_status_label(tool)),
                    code_operator(),
                )
            }
        }
        Some(label) if label.starts_with("approval:") => {
            ("Waiting for approval".to_string(), code_const())
        }
        Some(label) if label == "question" || label.starts_with("questions:") => {
            ("Waiting for input".to_string(), code_const())
        }
        _ => {
            // Prefer observed stream evidence over a generic spinner label.
            if active.is_some_and(|message| !message.thinking_content.is_empty()) {
                ("Thinking".to_string(), code_operator())
            } else if active.is_some_and(|message| !message.content.trim().is_empty()) {
                ("Streaming response".to_string(), accent())
            } else {
                // No tokens yet — honest wait, not false-specific "checking tools".
                ("Waiting for model".to_string(), code_operator())
            }
        }
    }
}

fn running_tools_status(app: &TuiApp) -> String {
    if app.running_tools.len() == 1 {
        let (id, invocation) = app
            .running_tools
            .iter()
            .next()
            .expect("running_tools non-empty");
        if let Some(activity) = app.subagent_activity.get(id) {
            let detail = activity.trim();
            if !detail.is_empty() {
                return format!("Subagent: {detail}");
            }
        }
        return format!("Running {}", tool_status_label(&invocation.tool_name));
    }

    format!("Running {} tools", app.running_tools.len())
}

fn background_subagent_status(app: &TuiApp) -> Option<String> {
    // Prefer the newest background subagent that still has live activity.
    for message in app.messages.iter().rev() {
        let Some(invocation) = message.tool_invocation.as_ref() else {
            continue;
        };
        if invocation.tool_name != "subagent" {
            continue;
        }
        let Some(result) = message.tool_result.as_ref() else {
            continue;
        };
        let still_running = result.output.get("background").and_then(|v| v.as_bool()) == Some(true)
            && result
                .output
                .get("status")
                .and_then(|v| v.as_str())
                .is_some_and(|status| {
                    status.eq_ignore_ascii_case("running") || status.eq_ignore_ascii_case("pending")
                });
        if !still_running {
            continue;
        }
        if let Some(activity) = app.subagent_activity.get(&invocation.id) {
            let detail = activity.trim();
            if !detail.is_empty() {
                return Some(format!("Subagent: {detail}"));
            }
        }
        return Some("Running subagent".to_string());
    }
    None
}

/// Latest in-flight model response (not a tool card / compact summary).
fn active_assistant_message(app: &TuiApp) -> Option<&crate::state::ChatMessage> {
    app.messages.iter().rev().find(|message| {
        message.role == ChatRole::Assistant
            && message.tool_invocation.is_none()
            && message.tool_result.is_none()
            && !message.is_compact_summary
    })
}

fn format_activity_elapsed(ms: u64) -> String {
    let seconds = ms / 1_000;
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m{}s", seconds / 60, seconds % 60)
    }
}

fn tool_status_label(tool_name: &str) -> String {
    tool_name.replace('_', " ")
}

fn visible_input_lines(
    lines: Vec<Line<'static>>,
    height: usize,
    cursor_line: usize,
) -> (Vec<Line<'static>>, usize) {
    let height = height.max(1);
    let mut start = cursor_line.saturating_add(1).saturating_sub(height);
    if start + height > lines.len() {
        start = lines.len().saturating_sub(height);
    }
    (lines.into_iter().skip(start).take(height).collect(), start)
}

fn input_lines(app: &TuiApp, width: usize) -> (Vec<Line<'static>>, usize, usize) {
    let width = width.max(1);
    let text_style = Style::default().fg(text());

    if app.input.is_empty() {
        return (vec![Line::from("")], 0, 0);
    }

    let cursor = app.input_cursor.min(app.input.len());
    let cursor = floor_char_boundary(&app.input, cursor);
    let selected = selected_input_range(app);
    let ranges = input_visual_line_ranges(&app.input, width);
    let mut cursor_line = ranges.len().saturating_sub(1);
    let mut cursor_column = 0;
    let mut lines = Vec::new();

    for (line_index, (start, end)) in ranges.iter().copied().enumerate() {
        if cursor >= start && cursor <= end {
            cursor_line = line_index;
            cursor_column = app.input[start..cursor].chars().count();
        }
        let line_text = &app.input[start..end];
        let mut spans = Vec::new();
        let mut current_idx = 0;

        while current_idx < line_text.len() {
            let rest = &line_text[current_idx..];
            let rest_bytes = rest.as_bytes();
            if rest_bytes.starts_with(b"[Image ") {
                let mut check_idx = 7;
                let mut has_digits = false;
                while check_idx < rest_bytes.len() && rest_bytes[check_idx].is_ascii_digit() {
                    has_digits = true;
                    check_idx += 1;
                }
                if has_digits && check_idx < rest_bytes.len() && rest_bytes[check_idx] == b']' {
                    let tag_end = current_idx + check_idx + 1;
                    let tag_text = &line_text[current_idx..tag_end];
                    let mut style = Style::default()
                        .bg(code_const())
                        .fg(ratatui::style::Color::Black);
                    if let Some((sel_start, sel_end)) = selected {
                        let tag_start_byte = start + current_idx;
                        let tag_end_byte = start + tag_end;
                        if tag_start_byte >= sel_start && tag_end_byte <= sel_end {
                            style = Style::default().fg(selection_fg()).bg(selection_bg());
                        }
                    }
                    spans.push(Span::styled(tag_text.to_string(), style));
                    current_idx = tag_end;
                    continue;
                }
            }

            if let Some(ch) = rest.chars().next() {
                let byte_idx = start + current_idx;
                let mut style = text_style;
                if selected
                    .is_some_and(|(sel_start, sel_end)| byte_idx >= sel_start && byte_idx < sel_end)
                {
                    style = style.fg(selection_fg()).bg(selection_bg());
                }
                spans.push(Span::styled(ch.to_string(), style));
                current_idx += ch.len_utf8();
            } else {
                break;
            }
        }
        lines.push(Line::from(spans));
    }
    (lines, cursor_line, cursor_column)
}

/// Built composer meta line + optional context-chip hit range (col offset, width).
struct ComposerMetaBuilt {
    line: Line<'static>,
    display_width: usize,
    /// `(start_col_in_line, chip_display_width)` for the context token meter.
    context_range: Option<(usize, usize)>,
}

/// Right-side composer chrome.
/// Kept compact so it can sit on the same row as the draft.
///
/// Context chip (Grok-style): default shows `3.2k / 200k`; hover reveals
/// `(12%)` with threshold coloring.
fn composer_meta_right(app: &TuiApp, width: usize) -> ComposerMetaBuilt {
    if !app.pending_questions.is_empty() {
        let line = Line::from(vec![
            Span::styled("Question pending", Style::default().fg(code_const())),
            Span::styled(" · ", Style::default().fg(ghost())),
            Span::styled(
                "ctrl+enter",
                Style::default().fg(signal()).add_modifier(Modifier::BOLD),
            ),
        ]);
        let display_width = spans_display_width(&line.spans);
        return ComposerMetaBuilt {
            line,
            display_width,
            context_range: None,
        };
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut context_range: Option<(usize, usize)> = None;

    if !app.queued_user_messages.is_empty() {
        spans.push(Span::styled(
            queued_footer_label(app),
            Style::default()
                .fg(code_const())
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(ghost())));
    }
    if let Some(info) = &app.available_update {
        spans.push(Span::styled(
            format!("⬆ v{}", info.latest_version),
            Style::default().fg(signal()).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(ghost())));
    }
    if app.bg_models_running > 0 {
        spans.push(Span::styled(
            format!("⚙ {} bg", app.bg_models_running),
            Style::default().fg(signal()).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(ghost())));
    }
    if !app.pending_images.is_empty() {
        let count = app.pending_images.len();
        spans.push(Span::styled(
            format!("{count} image{}", if count > 1 { "s" } else { "" }),
            Style::default().fg(signal()).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(ghost())));
    }

    let model = selected_model_label(app);
    let binary_effort =
        crate::state::ThinkingLevel::is_binary_for_model(app.models.get(app.selected_model));
    let thinking = app.thinking_level.display_label(binary_effort);
    let pending = app.input.len();
    let context = if app.hover_context_usage {
        app.compact_state.usage_label_with_percent(pending)
    } else {
        app.compact_state.usage_label_compact(pending)
    };
    let context_color = context_usage_color(app, pending);
    let permission = permission_mode_spans(app);
    let permission_w = spans_display_width(&permission);

    // Wide: `model (thinking) · [◆ credits] · context · permission`
    // Medium: `model · permission`
    // Narrow: permission only (or model if no permission room).
    if width >= 56 {
        let model_label = format!("{model} ({thinking})");
        spans.push(Span::styled(model_label, Style::default().fg(muted())));
        if let Some(hc) = hypercredit_footer_label(app) {
            spans.push(Span::styled(" · ", Style::default().fg(ghost())));
            spans.push(Span::styled(hc, Style::default().fg(signal())));
        }
        spans.push(Span::styled(" · ", Style::default().fg(ghost())));
        let start = spans_display_width(&spans);
        let chip_w = display_width(&context);
        spans.push(Span::styled(
            context,
            Style::default()
                .fg(context_color)
                .add_modifier(if app.hover_context_usage {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ));
        context_range = Some((start, chip_w));
        if permission_w > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(ghost())));
            spans.extend(permission);
        }
    } else if width >= 28 {
        spans.push(Span::styled(model, Style::default().fg(muted())));
        if permission_w > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(ghost())));
            spans.extend(permission);
        }
    } else if permission_w > 0 && permission_w <= width {
        spans.extend(permission);
    } else {
        spans.push(Span::styled(
            fit_display_width(&model, width.max(1)),
            Style::default().fg(muted()),
        ));
    }

    // Soft-trim if we still overflow — drop the hit range (layout changed).
    let total = spans_display_width(&spans);
    if total > width && width > 1 {
        let text = spans_to_text(&spans);
        let trimmed = fit_display_width(&text, width);
        let display_width = display_width(&trimmed);
        return ComposerMetaBuilt {
            line: Line::from(vec![Span::styled(trimmed, Style::default().fg(muted()))]),
            display_width,
            context_range: None,
        };
    }
    ComposerMetaBuilt {
        line: Line::from(spans),
        display_width: total,
        context_range,
    }
}

fn context_usage_color(app: &TuiApp, pending: usize) -> ratatui::style::Color {
    use navi_core::compact::CompactThreshold;
    match app.compact_state.threshold_level(pending) {
        CompactThreshold::Error | CompactThreshold::CircuitOpen => red(),
        CompactThreshold::Warning => signal(),
        CompactThreshold::Normal => {
            if app.hover_context_usage {
                accent()
            } else {
                ghost()
            }
        }
    }
}

fn queued_footer_label(app: &TuiApp) -> String {
    format!("{} queued", app.queued_user_messages.len())
}

/// Crush-style remaining Hypercredits chip for the composer meta strip.
fn hypercredit_footer_label(app: &TuiApp) -> Option<String> {
    let remaining = app.usage_state.remaining_credits?;
    let unit = app.usage_state.remaining_credit_unit.as_deref()?;
    if !unit.eq_ignore_ascii_case("hypercredits") {
        return None;
    }
    // Only show for Charm Hyper (or while remaining unit is hypercredits).
    let provider = app.loaded_config.config.model.provider.as_str();
    if navi_sdk::canonical_provider_id(provider) != "charm-hyper" {
        // Still show if we have an explicit hypercredits unit from a report.
    }
    Some(format!("◆ {}", navi_sdk::format_hypercredits(remaining)))
}

fn permission_mode_spans(app: &TuiApp) -> Vec<Span<'static>> {
    let mode = current_permission_mode(app);
    // YOLO is loud (red caps); other modes stay compact lowercase.
    let label = permission_mode_label(mode);
    let label_color = match mode {
        PermissionMode::Yolo => red(),
        PermissionMode::Restricted => code_const(),
        PermissionMode::AcceptEdits => signal(),
        PermissionMode::Auto => accent(),
    };
    let mut spans = vec![Span::styled(
        label,
        Style::default()
            .fg(label_color)
            .add_modifier(Modifier::BOLD),
    )];

    if app.dreaming {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            "dreaming…",
            Style::default().fg(signal()).add_modifier(Modifier::ITALIC),
        ));
    }

    spans
}

fn current_permission_mode(app: &TuiApp) -> PermissionMode {
    if app.yolo_mode {
        PermissionMode::Yolo
    } else {
        app.loaded_config.config.security.permission_mode
    }
}

fn permission_mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Restricted => "restricted",
        PermissionMode::AcceptEdits => "accept-edits",
        PermissionMode::Auto => "auto",
        PermissionMode::Yolo => "YOLO",
    }
}

fn spans_display_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum()
}

fn spans_to_text(spans: &[Span<'_>]) -> String {
    spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn fit_display_width(value: &str, width: usize) -> String {
    if display_width(value) <= width {
        return value.to_string();
    }
    if width <= 1 {
        return "…".to_string();
    }
    let mut result = String::new();
    let mut used = 0usize;
    for ch in value.chars() {
        let char_width = display_width(&ch.to_string());
        if used + char_width >= width {
            break;
        }
        result.push(ch);
        used += char_width;
    }
    result.push('…');
    result
}

fn selected_model_label(app: &TuiApp) -> String {
    let label = app
        .models
        .get(app.selected_model)
        .map(|model| model.name.as_str())
        .unwrap_or("model");
    let label = label.rsplit('/').next().unwrap_or(label);
    if label.chars().count() <= 24 {
        return label.to_string();
    }
    let mut shortened = label.chars().take(23).collect::<String>();
    shortened.push('…');
    shortened
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::time::Instant;

    use crate::state::ChatMessage;
    use crate::theme::{ThemeId, with_palette};

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn input_lines_wrap_long_text_and_keep_cursor_visible() {
        let mut app = crate::tests::test_app(&"a".repeat(30));
        app.input_cursor = app.input.len();

        let (lines, cursor_line, _) = input_lines(&app, 12);
        let (visible, _) = visible_input_lines(lines, 2, cursor_line);
        let text = visible.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert_eq!(visible.len(), 2);
        assert!(cursor_line >= 2);
        assert!(text.contains("aaa"));
        assert!(text.ends_with('a'));
    }

    #[test]
    fn input_lines_show_previous_line_after_trailing_newline() {
        let mut app = crate::tests::test_app("abc\n");
        app.input_cursor = app.input.len();

        let (lines, cursor_line, cursor_column) = input_lines(&app, 20);
        let (visible, _) = visible_input_lines(lines, 2, cursor_line);
        let text = visible.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert_eq!(visible.len(), 2);
        assert_eq!(cursor_line, 1);
        assert_eq!(cursor_column, 0);
        assert!(text.contains("abc"));
        assert!(line_text(visible.last().unwrap()).is_empty());
    }

    #[test]
    fn input_lines_handles_utf8_near_image_tag_probe_width() {
        let mut app = crate::tests::test_app("ainda nã");
        app.input_cursor = app.input.len();

        let (lines, _, cursor_column) = input_lines(&app, 20);

        assert_eq!(line_text(&lines[0]), "ainda nã");
        assert_eq!(cursor_column, 8);
    }

    #[test]
    fn input_lines_styles_image_tags_after_utf8_text() {
        let mut app = crate::tests::test_app("ação [Image 1]");
        app.input_cursor = app.input.len();

        let (lines, _, _) = input_lines(&app, 40);

        assert_eq!(line_text(&lines[0]), "ação [Image 1]");
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|span| span.content.as_ref() == "[Image 1]")
        );
    }

    #[test]
    fn selected_model_label_hides_provider_prefix() {
        let mut app = crate::tests::test_app("");
        let selected = app.selected_model;
        app.models[selected].name = "ai21/jamba-large-1.7".to_string();

        assert_eq!(selected_model_label(&app), "jamba-large-1.7");
    }

    #[test]
    fn composer_meta_uses_compact_metadata_on_narrow_viewports() {
        let app = crate::tests::test_app("");
        // right meta on medium width: `model · permission` (no provider/thinking).
        let built = composer_meta_right(&app, 34);
        let text = line_text(&built.line);

        assert!(text.contains("gpt-5.5"));
        assert!(!text.contains("OpenAI"));
        assert!(!text.contains("adaptive"));
        assert!(display_width(&text) <= 34);
    }

    #[test]
    fn composer_meta_right_wide_includes_thinking_and_context() {
        let app = crate::tests::test_app("");
        let built = composer_meta_right(&app, 72);
        let text = line_text(&built.line);
        assert!(text.contains("gpt-5.5"));
        assert!(text.contains('('));
        assert!(text.contains("0 /") || text.contains('/'));
        // Default: counts only — no percent until hover.
        assert!(
            !text.contains('%'),
            "percent should be hover-only, got {text}"
        );
        assert!(built.context_range.is_some());
    }

    #[test]
    fn composer_meta_reveals_percent_on_context_hover() {
        let mut app = crate::tests::test_app("");
        app.compact_state.last_input_tokens = Some(20_000);
        app.compact_state.context_window = 200_000;
        app.hover_context_usage = true;
        let built = composer_meta_right(&app, 80);
        let text = line_text(&built.line);
        assert!(
            text.contains('%'),
            "hover should reveal window percent: {text}"
        );
        assert!(text.contains("20k") || text.contains("20000") || text.contains('/'));
    }

    #[test]
    fn render_input_draws_prompt_and_rounded_border() {
        with_palette(&ThemeId::Lain.palette(), || {
            let mut app = crate::tests::test_app("hello");
            let mut terminal = Terminal::new(TestBackend::new(48, 6)).expect("terminal");
            terminal
                .draw(|frame| render_input(frame, &mut app, Rect::new(0, 0, 48, 5)))
                .expect("draw");
            let screen = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol().to_string())
                .collect::<String>();
            // Rounded corners + prompt glyph.
            assert!(
                screen.contains('╭') || screen.contains('┌'),
                "expected rounded/box border, got {screen:?}"
            );
            assert!(
                screen.contains('›') || screen.contains('>'),
                "expected › prompt, got {screen:?}"
            );
            assert!(screen.contains("hello"));
        });
    }

    #[test]
    fn composer_meta_shows_yolo_permission_in_red() {
        let mut app = crate::tests::test_app("");
        app.yolo_mode = true;

        let built = composer_meta_right(&app, 96);
        let yolo = built
            .line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "YOLO")
            .expect("YOLO permission label");

        assert_eq!(yolo.style.fg, Some(red()));
    }

    #[test]
    fn composer_goal_line_uses_short_description() {
        let mut app = crate::tests::test_app("");
        app.goal_state = Some(crate::state::GoalUiState {
            objective: "Implement a very long detailed objective".to_string(),
            short_description: Some("Fix modal layout".to_string()),
            ..Default::default()
        });

        let line = composer_goal_line(&app, 80).expect("goal line");
        let text = line_text(&line);

        assert!(text.contains("goal Fix modal layout"));
        assert!(!text.contains("very long detailed"));
    }

    #[test]
    fn queued_footer_label_registers_click_hit() {
        let mut app = crate::tests::test_app("");
        app.queued_user_messages
            .push_back(crate::state::QueuedUserMessage {
                text: "queued task".to_string(),
                images: Vec::new(),
            });
        // Height 5: border+draft+border+meta → meta on last row.
        let mut terminal = Terminal::new(TestBackend::new(72, 8)).expect("terminal");

        terminal
            .draw(|frame| render_input(frame, &mut app, Rect::new(0, 0, 72, 5)))
            .expect("draw");

        // Meta row is the last row of the input area (y = 0 + 5 - 1 = 4).
        let hit = app
            .hit_test(2, 4)
            .or_else(|| app.hit_test(4, 4))
            .expect("queued label hit on meta row");
        assert!(matches!(
            hit.action,
            crate::ui::interaction::HitAction::OpenMessageQueue
        ));
    }

    #[test]
    fn composer_activity_line_only_shows_while_loading() {
        let mut app = crate::tests::test_app("");
        assert!(composer_activity_line(&app, 80).is_none());

        app.is_loading = true;
        app.loading_start = Some(Instant::now());
        let line = composer_activity_line(&app, 80).expect("activity line");
        let text = line_text(&line);

        assert!(text.contains("Waiting for model"));
        assert!(text.contains("0s"));
    }

    #[test]
    fn composer_activity_line_uses_receiving_status() {
        let mut app = crate::tests::test_app("");
        app.is_loading = true;
        app.loading_start = Some(Instant::now());
        app.messages.push(ChatMessage {
            status: Some("receiving".to_string()),
            ..ChatMessage::new(ChatRole::Assistant, "Partial".to_string())
        });

        let line = composer_activity_line(&app, 80).expect("activity line");
        assert!(line_text(&line).contains("Streaming response"));
    }

    #[test]
    fn composer_activity_line_uses_thinking_content() {
        let mut app = crate::tests::test_app("");
        app.is_loading = true;
        app.loading_start = Some(Instant::now());
        app.messages.push(ChatMessage {
            status: Some("thinking".to_string()),
            thinking_content: "step by step".to_string(),
            ..ChatMessage::new(ChatRole::Assistant, String::new())
        });

        let line = composer_activity_line(&app, 80).expect("activity line");
        assert!(line_text(&line).contains("Thinking"));
    }

    #[test]
    fn composer_activity_line_uses_running_tool() {
        let mut app = crate::tests::test_app("");
        app.is_loading = true;
        app.loading_start = Some(Instant::now());
        app.running_tools.insert(
            "call-1".to_string(),
            navi_sdk::ToolInvocation {
                id: "call-1".to_string(),
                tool_name: "read_file".to_string(),
                input: serde_json::json!({}),
            },
        );

        let line = composer_activity_line(&app, 80).expect("activity line");
        assert!(line_text(&line).contains("Running read file"));
    }

    #[test]
    fn composer_collapses_when_scrollback_focused() {
        let mut app = crate::tests::test_app("line one\nline two\nline three");
        // Focused: multi-line target.
        assert!(composer_is_focused(&app));
        let focused_target = composer_target_content_lines(&app, 40);
        assert!(
            focused_target >= 2,
            "expected multi-line target, got {focused_target}"
        );

        // Select a chat block → unfocused → collapse to 1.
        app.messages
            .push(ChatMessage::new(ChatRole::User, "hi".into()));
        app.selected_chat_source = Some(crate::state::ChatLineSource::Message(0));
        assert!(!composer_is_focused(&app));
        assert_eq!(composer_target_content_lines(&app, 40), 1);
    }

    #[test]
    fn composer_animation_moves_toward_target() {
        let mut app = crate::tests::test_app("a\nb\nc\nd");
        app.composer_anim_lines = 1.0;
        assert!(advance_composer_animation(&mut app, 40));
        assert!(app.composer_anim_lines > 1.0);
    }

    #[test]
    fn render_input_uses_transparent_chat_background() {
        with_palette(&ThemeId::Lain.palette(), || {
            let mut app = crate::tests::test_app("");
            let mut terminal = Terminal::new(TestBackend::new(32, 6)).expect("terminal");

            terminal
                .draw(|frame| render_input(frame, &mut app, Rect::new(0, 0, 32, 5)))
                .expect("draw");

            let buffer = terminal.backend().buffer();
            // Interior cells use chat bg — no elevated panel fill.
            assert_eq!(buffer[(2, 1)].bg, bg());
        });
    }

    #[test]
    fn render_input_keeps_meta_outside_draft_box() {
        with_palette(&ThemeId::Lain.palette(), || {
            let mut app = crate::tests::test_app("hello world");
            let mut terminal = Terminal::new(TestBackend::new(56, 6)).expect("terminal");
            terminal
                .draw(|frame| render_input(frame, &mut app, Rect::new(0, 0, 56, 5)))
                .expect("draw");

            let buffer = terminal.backend().buffer();
            let row = |y: u16| {
                (0..56)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            };
            // area y=0..5 → box height 4 (rows 0–3), meta at y=4.
            let draft_row = row(1);
            let meta_row = row(4);

            assert!(
                draft_row.contains("hello"),
                "draft should be inside the box: {draft_row:?}"
            );
            // Model/permission must not sit on the draft line.
            assert!(
                !draft_row.contains("gpt")
                    && !draft_row.contains("yolo")
                    && !draft_row.contains("auto"),
                "meta chrome must not be inside draft: {draft_row:?}"
            );
            // Meta row may be empty if model label is long; at least draft isolation holds.
            let _ = meta_row;
        });
    }
}
