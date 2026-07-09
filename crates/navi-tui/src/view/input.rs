use ratatui::layout::{Alignment, Margin, Position, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, Paragraph};

use crate::TuiApp;
use crate::input::{
    COMPOSER_MAX_VISIBLE_LINES, input_visual_line_count, input_visual_line_ranges,
    selected_input_range,
};
use crate::providers::selected_provider_label;
use crate::render::text::display_width;
use crate::state::ChatRole;
use crate::theme::*;
use crate::ui::floor_char_boundary;
use crate::ui::interaction::HitAction;
use navi_core::PermissionMode;

const INPUT_TOP_PADDING_ROWS: u16 = 1;
const FOOTER_BOTTOM_PADDING_ROWS: u16 = 1;
const INPUT_TEXT_INSET_COLUMNS: u16 = 1;

pub(crate) fn render_input(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 0,
    });

    let block = Block::new()
        .borders(ratatui::widgets::Borders::LEFT)
        .border_set(ratatui::symbols::border::Set {
            vertical_left: "▌",
            ..ratatui::symbols::border::PLAIN
        })
        .border_style(Style::default().fg(accent()).bg(composer_panel_bg(app)))
        .style(Style::default().fg(text()).bg(composer_panel_bg(app)));

    let inner_block_area = block.inner(inner);
    frame.render_widget(block, inner);

    frame.render_widget(
        Block::new().style(Style::default().bg(composer_panel_bg(app))),
        inner_block_area,
    );

    let panel_area = inner_block_area.inner(Margin {
        horizontal: INPUT_TEXT_INSET_COLUMNS,
        vertical: 0,
    });
    let input_y = panel_area.y + INPUT_TOP_PADDING_ROWS.min(panel_area.height);
    let last_panel_y = panel_area.y + panel_area.height.saturating_sub(1);
    let desired_footer_y = panel_area.y
        + panel_area
            .height
            .saturating_sub(1 + FOOTER_BOTTOM_PADDING_ROWS);
    let footer_y = desired_footer_y
        .max(input_y.saturating_add(1))
        .min(last_panel_y);
    let footer_area = Rect::new(
        panel_area.x,
        footer_y,
        panel_area.width,
        1.min(panel_area.height),
    );
    let input_bottom = footer_area.y;
    let full_input_area = Rect::new(
        panel_area.x,
        input_y,
        panel_area.width.saturating_add(INPUT_TEXT_INSET_COLUMNS),
        input_bottom.saturating_sub(input_y).max(1),
    );
    let input_area = full_input_area;

    app.input_wrap_width = input_area.width as usize;
    let (lines, cursor_line, cursor_column) = input_lines(app, input_area.width as usize);
    let ranges = crate::input::input_visual_line_ranges(&app.input, input_area.width as usize);
    let (input_lines, visible_start) =
        visible_input_lines(lines, input_area.height as usize, cursor_line);

    // Register hover hits for `[Image N]` chips before paint so z-order is ready.
    if app.mode == crate::state::Mode::Normal && !app.pending_images.is_empty() {
        let input_owned = app.input.clone();
        for (visible_row, line_index) in
            (visible_start..visible_start + input_lines.len()).enumerate()
        {
            let Some((start, end)) = ranges.get(line_index).copied() else {
                continue;
            };
            if end > input_owned.len() || start > end {
                continue;
            }
            let line_text = input_owned[start..end].to_string();
            let line_area = Rect::new(
                input_area.x,
                input_area.y.saturating_add(visible_row as u16),
                input_area.width,
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
        Paragraph::new(Text::from(input_lines))
            .style(Style::default().fg(text()).bg(composer_panel_bg(app)))
            .block(Block::new()),
        input_area,
    );
    if app.mode == crate::state::Mode::Normal {
        let cursor_x = input_area
            .x
            .saturating_add(cursor_column.min(input_area.width.saturating_sub(1) as usize) as u16);
        let cursor_y = input_area.y.saturating_add(
            cursor_line
                .saturating_sub(visible_start)
                .min(input_area.height.saturating_sub(1) as usize) as u16,
        );
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }

    frame.render_widget(
        Paragraph::new(composer_footer_line(app, footer_area.width as usize))
            .style(Style::default().bg(composer_panel_bg(app))),
        footer_area,
    );

    if !app.queued_user_messages.is_empty() {
        let queued_width = queued_footer_label(app).len() as u16;
        app.register_hit(
            Rect::new(
                footer_area.x,
                footer_area.y,
                queued_width.min(footer_area.width),
                1,
            ),
            4,
            "open message queue",
            HitAction::OpenMessageQueue,
        );
    }

    if !app.pending_questions.is_empty() {
        app.register_hit(
            footer_area,
            3,
            "reopen pending question",
            HitAction::ReopenQuestion,
        );
    }
}

pub(crate) fn composer_height(app: &TuiApp, input_width: usize) -> u16 {
    let wrap_width = input_width.saturating_sub(6);
    let visible_lines =
        input_visual_line_count(&app.input, wrap_width).clamp(1, COMPOSER_MAX_VISIBLE_LINES) as u16;
    // top inset + text area (min 3 rows) + footer
    INPUT_TOP_PADDING_ROWS + visible_lines.max(3) + 1
}

pub(crate) fn composer_hint_height(app: &TuiApp) -> u16 {
    let hint = if show_composer_hint(app) { 1 } else { 0 };
    let goal = if app.goal_state.is_some() { 1 } else { 0 };
    hint + goal
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

    if let Some(goal_line) = composer_goal_line(app, area.width as usize) {
        let goal_area = Rect::new(area.x, area.y, area.width, 1);
        frame.render_widget(
            Paragraph::new(goal_line).style(Style::default().bg(bg())),
            goal_area,
        );
    }

    if show_composer_hint(app) {
        let hint_area = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(1),
            area.width,
            1,
        );
        frame.render_widget(
            Paragraph::new(composer_hint_line(hint_area.width as usize))
                .alignment(Alignment::Right)
                .style(Style::default().bg(bg())),
            hint_area,
        );
    }
}

fn show_composer_hint(_app: &TuiApp) -> bool {
    true
}

fn composer_hint_line(width: usize) -> Line<'static> {
    let style = Style::default().fg(ghost()).add_modifier(Modifier::ITALIC);
    let hint = if width >= 96 {
        "ctrl+p commands · ctrl+t background tasks · ctrl+b background agents · ctrl+v paste image"
    } else if width >= 62 {
        "ctrl+p commands · ctrl+m models · ctrl+enter send · ctrl+v image"
    } else {
        "ctrl+p commands · ctrl+enter send"
    };
    Line::from(vec![Span::styled(hint, style)])
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

pub(crate) fn composer_panel_bg(_app: &TuiApp) -> ratatui::style::Color {
    interactive_bg()
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
    // Grok-style diamond pulse while a turn is running — no corner trail.
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

fn composer_footer_line(app: &TuiApp, width: usize) -> Line<'static> {
    if !app.pending_questions.is_empty() {
        return footer_with_permission(
            vec![
                Span::styled("Question pending", Style::default().fg(code_const())),
                Span::styled(" · ", Style::default().fg(ghost())),
                Span::styled(
                    "ctrl+enter",
                    Style::default().fg(signal()).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" reopen", Style::default().fg(muted())),
            ],
            app,
            width,
        );
    }

    let mut spans: Vec<Span<'static>> = Vec::new();

    // Show queued message indicator.
    if !app.queued_user_messages.is_empty() {
        spans.push(Span::styled(
            queued_footer_label(app),
            Style::default()
                .fg(code_const())
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(ghost())));
    }

    // Show bg model running indicator
    if app.bg_models_running > 0 {
        spans.push(Span::styled(
            format!("⚙ {} bg", app.bg_models_running),
            Style::default().fg(signal()).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(ghost())));
    }

    // Show pending image indicator if any images are attached.
    if !app.pending_images.is_empty() {
        let count = app.pending_images.len();
        spans.push(Span::styled(
            format!("{} image{}", count, if count > 1 { "s" } else { "" }),
            Style::default().fg(signal()).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(ghost())));
    }

    let provider = selected_provider_label(app);
    let thinking = app.thinking_level.label();
    let model = selected_model_label(app);
    // Include composer draft in preflight estimate (Grok-style live meter).
    let context = app.compact_state.usage_label(app.input.len());

    if width < 48 {
        let available_model_width = width.saturating_sub(display_width(&context) + 3).max(1);
        return footer_with_permission(
            vec![
                Span::styled(
                    fit_display_width(&model, available_model_width),
                    Style::default().fg(code_type()),
                ),
                Span::styled(" · ", Style::default().fg(ghost())),
                Span::styled(
                    context,
                    Style::default()
                        .fg(code_number())
                        .add_modifier(Modifier::BOLD),
                ),
            ],
            app,
            width,
        );
    }

    spans.push(Span::styled(model, Style::default().fg(code_type())));
    spans.push(Span::styled(" ", Style::default().fg(ghost())));
    spans.push(Span::styled(
        provider.to_string(),
        Style::default().fg(signal()).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(" · ", Style::default().fg(ghost())));
    spans.push(Span::styled(
        thinking.to_string(),
        Style::default()
            .fg(code_const())
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(" · ", Style::default().fg(ghost())));
    spans.push(Span::styled(
        context,
        Style::default()
            .fg(code_number())
            .add_modifier(Modifier::BOLD),
    ));
    // Per-turn usage from the last UsageReported event (Grok updates this every turn).
    if let Some(turn) = app.usage_state.last_turn_label.as_ref() {
        spans.push(Span::styled(" · ", Style::default().fg(ghost())));
        spans.push(Span::styled(
            turn.clone(),
            Style::default().fg(muted()),
        ));
    }

    footer_with_permission(spans, app, width)
}

fn queued_footer_label(app: &TuiApp) -> String {
    format!("{} queued", app.queued_user_messages.len())
}

fn footer_with_permission(
    mut left: Vec<Span<'static>>,
    app: &TuiApp,
    width: usize,
) -> Line<'static> {
    let permission = permission_mode_spans(app);
    let permission_width = spans_display_width(&permission);
    let left_width = spans_display_width(&left);

    if permission_width == 0 || width < permission_width + 8 {
        return Line::from(left);
    }

    if left_width + permission_width < width {
        left.push(Span::styled(
            " ".repeat(width - left_width - permission_width),
            Style::default().fg(ghost()),
        ));
        left.extend(permission);
        return Line::from(left);
    }

    let allowed_left_width = width.saturating_sub(permission_width + 1);
    let left_text = spans_to_text(&left);
    Line::from(
        vec![
            Span::styled(
                fit_display_width(&left_text, allowed_left_width),
                Style::default().fg(muted()),
            ),
            Span::styled(" ", Style::default().fg(ghost())),
        ]
        .into_iter()
        .chain(permission)
        .collect::<Vec<_>>(),
    )
}

fn permission_mode_spans(app: &TuiApp) -> Vec<Span<'static>> {
    let mode = current_permission_mode(app);
    let label = permission_mode_label(mode);
    let label_color = if mode == PermissionMode::Yolo {
        red()
    } else {
        signal()
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
        PermissionMode::Yolo => "yolo",
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
    fn composer_footer_uses_compact_metadata_on_narrow_viewports() {
        let app = crate::tests::test_app("");
        let line = composer_footer_line(&app, 34);
        let text = line_text(&line);

        assert!(text.contains("gpt-5.5"));
        assert!(text.contains("0 /"));
        assert!(!text.contains("OpenAI"));
        assert!(!text.contains("adaptive"));
        assert!(display_width(&text) <= 34);
    }

    #[test]
    fn composer_footer_shows_yolo_permission_in_red() {
        let mut app = crate::tests::test_app("");
        app.yolo_mode = true;

        let line = composer_footer_line(&app, 96);
        let yolo = line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "yolo")
            .expect("yolo permission label");

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
        let mut terminal = Terminal::new(TestBackend::new(72, 8)).expect("terminal");

        terminal
            .draw(|frame| render_input(frame, &mut app, Rect::new(0, 0, 72, 6)))
            .expect("draw");

        let hit = app.hit_test(4, 4).expect("queued label hit");
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
    fn render_input_left_padding_uses_panel_background() {
        with_palette(&ThemeId::Lain.palette(), || {
            let mut app = crate::tests::test_app("");
            let mut terminal = Terminal::new(TestBackend::new(32, 6)).expect("terminal");

            terminal
                .draw(|frame| render_input(frame, &mut app, Rect::new(0, 0, 32, 6)))
                .expect("draw");

            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[(2, 1)].bg, composer_panel_bg(&app));
        });
    }
}
