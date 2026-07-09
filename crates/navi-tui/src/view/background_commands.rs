//! Background tasks modal — clean cards + readable output viewer.
//!
//! Output modal layout:
//! ```text
//! ╭─ Background Task Output ─────────────────────────────╮
//! │  ◆ Done · 17s · exit 0                               │
//! │  $ echo "…"                                          │
//! │  description · bg_1                                  │
//! │  ─────────────────────────────────────────────────── │
//! │  stdout                                              │
//! │  line one                                            │
//! │  line two                                            │
//! │                                                      │
//! │  following tail · ↑↓ scroll · esc back               │
//! ╰──────────────────────────────────────────────────────╯
//! ```

use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use navi_sdk::{BackgroundCommandSnapshot, BackgroundTaskStatus};

use crate::TuiApp;
use crate::background::{bg_status_label, format_bg_elapsed};
use crate::render::layout::opaque_fill;
use crate::render::status::{DIAMOND, DIAMOND_HOLLOW, running_diamond};
use crate::render::text::display_width;
use crate::render::clear_modal_area;
use crate::theme::*;
use crate::ui::interaction::{HitAction, ScrollTarget, line_rect};
use crate::ui::list::render_scrollbar;

/// Visual lines consumed by each task card (status + command + detail).
const CARD_LINES: usize = 3;
/// Gap line between cards.
const CARD_GAP: usize = 1;
const CARD_STRIDE: usize = CARD_LINES + CARD_GAP;

/// Solid panel color — never Reset, so chat never bleeds through modals.
fn solid_surface() -> Color {
    let c = modal_bg();
    if matches!(c, Color::Reset) {
        Color::Black
    } else {
        c
    }
}

fn surface_style() -> Style {
    Style::default().fg(text()).bg(solid_surface())
}

pub(crate) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    opaque_fill(frame, area, surface_style());

    let total = app.background_commands.len();
    let title = if total == 0 {
        "Background Tasks".to_string()
    } else {
        format!("Background Tasks  ·  {total}")
    };
    frame.render_widget(
        Block::new()
            .title(Line::from(Span::styled(
                format!(" {title} "),
                Style::default()
                    .fg(text())
                    .bg(solid_surface())
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(accent()).bg(solid_surface()))
            .style(surface_style()),
        area,
    );

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    opaque_fill(frame, inner, surface_style());

    if app.background_commands.is_empty() {
        render_empty(frame, inner);
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(6),
            Constraint::Length(1),
        ])
        .split(inner);

    let running = app
        .background_commands
        .iter()
        .filter(|cmd| cmd.is_running())
        .count();
    let done = total.saturating_sub(running);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{running}"),
                Style::default()
                    .fg(if running > 0 { accent() } else { muted() })
                    .bg(solid_surface())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" running", Style::default().fg(muted()).bg(solid_surface())),
            Span::styled("  ·  ", Style::default().fg(ghost()).bg(solid_surface())),
            Span::styled(
                format!("{done}"),
                Style::default()
                    .fg(if done > 0 { signal() } else { muted() })
                    .bg(solid_surface())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" done", Style::default().fg(muted()).bg(solid_surface())),
            Span::styled(
                "    ▸ open  ·  ✕ cancel",
                Style::default().fg(ghost()).bg(solid_surface()),
            ),
        ]))
        .style(surface_style()),
        rows[0],
    );

    let list_area = rows[1];
    opaque_fill(frame, list_area, surface_style());
    let visible_cards = (list_area.height as usize / CARD_STRIDE).max(1);
    let start = app
        .bg_command_scroll
        .min(app.background_commands.len().saturating_sub(visible_cards));
    let end = (start + visible_cards).min(app.background_commands.len());

    let mut y_off = 0usize;
    for index in start..end {
        let Some(cmd) = app.background_commands.get(index) else {
            continue;
        };
        let selected = index == app.bg_command_selected;
        let card_top = list_area.y.saturating_add(y_off as u16);
        if card_top >= list_area.y.saturating_add(list_area.height) {
            break;
        }
        let remaining = list_area
            .y
            .saturating_add(list_area.height)
            .saturating_sub(card_top) as usize;
        let draw_lines = CARD_LINES.min(remaining);
        if draw_lines == 0 {
            break;
        }

        let card_area = Rect::new(list_area.x, card_top, list_area.width, draw_lines as u16);
        render_task_card(frame, app, cmd, index, selected, card_area);
        y_off = y_off.saturating_add(CARD_STRIDE);
    }

    render_scrollbar(
        frame,
        app,
        list_area,
        app.background_commands.len().saturating_mul(CARD_STRIDE),
        start.saturating_mul(CARD_STRIDE),
        ScrollTarget::BackgroundCommands,
    );

    let footer = if running > 0 {
        "enter open   c/✕ cancel   r refresh   ↑↓ select   esc close"
    } else {
        "enter open   r refresh   ↑↓ select   esc close"
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(muted()).bg(solid_surface())),
        rows[2],
    );
}

fn render_task_card(
    frame: &mut Frame<'_>,
    app: &TuiApp,
    cmd: &BackgroundCommandSnapshot,
    index: usize,
    selected: bool,
    area: Rect,
) {
    // Selected cards use the theme selection pair (fg+bg) so light highlight
    // bars never leave light text on a light background.
    let (fg, bg) = if selected {
        let bg = selection_bg();
        let fg = selection_fg();
        (
            if matches!(fg, Color::Reset) {
                Color::Black
            } else {
                fg
            },
            if matches!(bg, Color::Reset) {
                solid_surface()
            } else {
                bg
            },
        )
    } else {
        (text(), solid_surface())
    };
    let width = area.width as usize;
    let lines = card_lines(cmd, selected, width, app.tick(), fg, bg);

    opaque_fill(frame, area, Style::default().fg(fg).bg(bg));
    for (row, line) in lines.into_iter().enumerate() {
        if row as u16 >= area.height {
            break;
        }
        let row_area = line_rect(area, row);
        frame.render_widget(
            Paragraph::new(line).style(Style::default().fg(fg).bg(bg)),
            row_area,
        );
    }

    app.register_hit(
        area,
        30,
        format!("background task {}", cmd.task_id),
        HitAction::BackgroundCommandOpen(index),
    );

    if area.width > 0 && area.height > 0 {
        let chevron = Rect::new(area.x, area.y, 2.min(area.width), 1);
        app.register_hit(
            chevron,
            45,
            format!("open background task {}", cmd.task_id),
            HitAction::BackgroundCommandOpen(index),
        );
    }

    if cmd.is_running() && area.width >= 3 && area.height > 0 {
        let cancel = Rect::new(
            area.x.saturating_add(area.width.saturating_sub(2)),
            area.y,
            2,
            1,
        );
        app.register_hit(
            cancel,
            50,
            format!("cancel background task {}", cmd.task_id),
            HitAction::BackgroundCommandCancel(index),
        );
    }
}

fn card_lines(
    cmd: &BackgroundCommandSnapshot,
    selected: bool,
    width: usize,
    tick: u64,
    fg: Color,
    bg: Color,
) -> Vec<Line<'static>> {
    // On selected rows force all ink through `fg` (theme selection_fg) so a light
    // selection bar never hosts light-on-light text.
    let status_color = if selected {
        fg
    } else {
        status_color(cmd.status)
    };
    let title_color = fg;
    let muted_color = if selected { fg } else { muted() };
    let dim = if selected { fg } else { ghost() };
    let base = Style::default().fg(fg).bg(bg);

    let chevron = if selected { "▾" } else { "▸" };
    let glyph = status_glyph(cmd, tick);
    let status = human_status(cmd);
    let elapsed = format_bg_elapsed(cmd);

    let cancel = if cmd.is_running() { "✕" } else { "" };
    let left1 = format!("{chevron}  {glyph} {status} · {elapsed}");
    let pad1 = width
        .saturating_sub(display_width(&left1))
        .saturating_sub(display_width(cancel));
    let mut line1 = vec![
        Span::styled(
            format!("{chevron} "),
            base.fg(if selected { fg } else { ghost() })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{glyph} "),
            base.fg(status_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{status} · {elapsed}"),
            base.fg(status_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ".repeat(pad1), base.fg(dim)),
    ];
    if cmd.is_running() {
        line1.push(Span::styled(
            "✕",
            base.fg(if selected { fg } else { red() })
                .add_modifier(Modifier::BOLD),
        ));
    }

    let cmd_text = truncate_display(&cmd.command.replace('\n', " "), width.saturating_sub(3));
    let line2 = Line::from(vec![
        Span::styled("   ", base.fg(dim)),
        Span::styled(
            cmd_text,
            base.fg(title_color).add_modifier(if selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
        ),
    ]);

    let desc = cmd
        .description
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("");
    let detail = if desc.is_empty() {
        cmd.task_id.clone()
    } else {
        format!("{desc}  ·  {}", cmd.task_id)
    };
    let line3 = Line::from(vec![
        Span::styled("   ", base.fg(dim)),
        Span::styled(
            truncate_display(&detail, width.saturating_sub(3)),
            base.fg(muted_color),
        ),
    ]);

    vec![Line::from(line1), line2, line3]
}

fn status_glyph(cmd: &BackgroundCommandSnapshot, tick: u64) -> &'static str {
    match cmd.status {
        BackgroundTaskStatus::Running => running_diamond(tick.saturating_mul(80)),
        BackgroundTaskStatus::Completed => "✓",
        BackgroundTaskStatus::Failed | BackgroundTaskStatus::TimedOut => "✗",
        BackgroundTaskStatus::Cancelled => DIAMOND_HOLLOW,
    }
}

fn human_status(cmd: &BackgroundCommandSnapshot) -> &'static str {
    bg_status_label(cmd)
}

/// Output viewer — fully opaque, structured header + log body + footer.
pub(crate) fn render_output(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    // Wipe every cell first so chat never peeks through transparent themes.
    frame.render_widget(Clear, area);
    opaque_fill(frame, area, surface_style());

    let block = Block::new()
        .title(Line::from(Span::styled(
            " Background Task Output ",
            Style::default()
                .fg(text())
                .bg(solid_surface())
                .add_modifier(Modifier::BOLD),
        )))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent()).bg(solid_surface()))
        .style(surface_style());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    opaque_fill(frame, inner, surface_style());

    let Some(cmd) = app.background_commands.get(app.bg_command_selected) else {
        render_empty(frame, inner);
        return;
    };

    // header (4) + body + footer (1)
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(6),
            Constraint::Length(1),
        ])
        .split(inner);

    render_output_header(frame, cmd, rows[0], app.tick());

    // Horizontal rule under header
    // (drawn as last line of header region)

    let body = rows[1];
    opaque_fill(frame, body, surface_style());
    let output_width = body.width.saturating_sub(2) as usize; // room for scrollbar
    let lines = output_lines(cmd, output_width.max(8));
    let visible = body.height as usize;
    let max_scroll = lines.len().saturating_sub(visible);
    let offset = if app.bg_command_output_follow {
        max_scroll
    } else {
        app.bg_command_output_scroll.min(max_scroll)
    };
    let visible_lines: Vec<Line<'static>> = lines
        .iter()
        .skip(offset)
        .take(visible)
        .cloned()
        .collect();

    // Pre-wrapped lines — do NOT enable Paragraph wrap (double-wrap mess).
    frame.render_widget(
        Paragraph::new(Text::from(visible_lines)).style(surface_style()),
        body,
    );
    render_scrollbar(
        frame,
        app,
        body,
        lines.len(),
        offset,
        ScrollTarget::BackgroundCommandOutput,
    );

    let follow = if app.bg_command_output_follow {
        "following tail"
    } else {
        "scroll locked"
    };
    let footer = if cmd.is_running() {
        format!("{follow}  ·  ↑↓ scroll  ·  f tail  ·  c cancel  ·  r refresh  ·  esc back")
    } else {
        format!("{follow}  ·  ↑↓ scroll  ·  f tail  ·  r refresh  ·  esc back")
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(muted()).bg(solid_surface())),
        rows[2],
    );

    if cmd.is_running() && rows[0].width >= 3 {
        let cancel = Rect::new(
            rows[0].x.saturating_add(rows[0].width.saturating_sub(2)),
            rows[0].y,
            2,
            1,
        );
        app.register_hit(
            cancel,
            50,
            format!("cancel background task {}", cmd.task_id),
            HitAction::BackgroundCommandCancel(app.bg_command_selected),
        );
    }
}

fn render_empty(frame: &mut Frame<'_>, area: Rect) {
    opaque_fill(frame, area, surface_style());
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "No background tasks.",
            Style::default().fg(muted()).bg(solid_surface()),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Run a command with background=true to keep it alive here.",
            Style::default().fg(muted()).bg(solid_surface()),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "esc close",
            Style::default().fg(ghost()).bg(solid_surface()),
        )),
    ];
    frame.render_widget(Paragraph::new(lines).style(surface_style()), area);
}

fn render_output_header(
    frame: &mut Frame<'_>,
    cmd: &BackgroundCommandSnapshot,
    area: Rect,
    tick: u64,
) {
    opaque_fill(frame, area, surface_style());
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // status
            Constraint::Length(1), // command
            Constraint::Length(1), // meta
            Constraint::Length(1), // separator
        ])
        .split(area);

    let glyph = status_glyph(cmd, tick);
    let status = human_status(cmd);
    let status_color = status_color(cmd.status);
    let cancel_hint = if cmd.is_running() { "  ✕" } else { "" };

    // Status row
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{glyph} "),
                Style::default()
                    .fg(status_color)
                    .bg(solid_surface())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                status,
                Style::default()
                    .fg(status_color)
                    .bg(solid_surface())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(ghost()).bg(solid_surface())),
            Span::styled(
                format_bg_elapsed(cmd),
                Style::default().fg(muted()).bg(solid_surface()),
            ),
            exit_code_span(cmd),
            Span::styled(
                cancel_hint,
                Style::default()
                    .fg(red())
                    .bg(solid_surface())
                    .add_modifier(Modifier::BOLD),
            ),
        ]))
        .style(surface_style()),
        rows[0],
    );

    // Command row — always $ prefix for scanability
    let cmd_display = format!(
        "$ {}",
        truncate_display(
            &cmd.command.replace('\n', " "),
            rows[1].width.saturating_sub(2) as usize
        )
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            cmd_display,
            Style::default()
                .fg(text())
                .bg(solid_surface())
                .add_modifier(Modifier::BOLD),
        )))
        .style(surface_style()),
        rows[1],
    );

    // Meta row
    let description = cmd.description.as_deref().unwrap_or("").trim();
    let meta = if description.is_empty() {
        format!("id {}", cmd.task_id)
    } else {
        format!(
            "{}  ·  id {}",
            truncate_display(description, rows[2].width.saturating_sub(12) as usize),
            cmd.task_id
        )
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            meta,
            Style::default().fg(muted()).bg(solid_surface()),
        )))
        .style(surface_style()),
        rows[2],
    );

    // Separator
    let rule = "─".repeat(rows[3].width as usize);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            rule,
            Style::default().fg(ghost()).bg(solid_surface()),
        )))
        .style(surface_style()),
        rows[3],
    );
}

fn exit_code_span(cmd: &BackgroundCommandSnapshot) -> Span<'static> {
    match cmd.exit_code {
        Some(0) => Span::styled(
            "  ·  exit 0".to_string(),
            Style::default().fg(signal()).bg(solid_surface()),
        ),
        Some(code) => Span::styled(
            format!("  ·  exit {code}"),
            Style::default().fg(red()).bg(solid_surface()),
        ),
        None => Span::styled("", Style::default().bg(solid_surface())),
    }
}

fn output_lines(cmd: &BackgroundCommandSnapshot, width: usize) -> Vec<Line<'static>> {
    let width = width.max(12);
    let mut lines = Vec::new();

    if let Some(error) = &cmd.error {
        lines.push(section_header("error", red()));
        push_wrapped(&mut lines, error, width, red());
        lines.push(blank_line());
    }

    let has_stdout = !cmd.stdout.is_empty();
    let has_stderr = !cmd.stderr.is_empty();

    if has_stdout {
        lines.push(section_header("stdout", accent()));
        push_wrapped(&mut lines, &cmd.stdout, width, text());
        if cmd.stdout_truncated {
            lines.push(Line::from(Span::styled(
                "  [truncated]",
                Style::default().fg(ghost()).bg(solid_surface()),
            )));
        }
    }

    if has_stdout && has_stderr {
        lines.push(blank_line());
    }

    if has_stderr {
        lines.push(section_header("stderr", red()));
        push_wrapped(&mut lines, &cmd.stderr, width, red());
        if cmd.stderr_truncated {
            lines.push(Line::from(Span::styled(
                "  [truncated]",
                Style::default().fg(ghost()).bg(solid_surface()),
            )));
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Waiting for output…",
            Style::default().fg(muted()).bg(solid_surface()),
        )));
    }
    lines
}

fn section_header(label: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{DIAMOND} "),
            Style::default()
                .fg(color)
                .bg(solid_surface())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            label.to_string(),
            Style::default()
                .fg(color)
                .bg(solid_surface())
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn blank_line() -> Line<'static> {
    Line::from(Span::styled(
        "",
        Style::default().bg(solid_surface()),
    ))
}

fn push_wrapped(
    lines: &mut Vec<Line<'static>>,
    content: &str,
    width: usize,
    color: Color,
) {
    // Indent body under section headers for clear hierarchy.
    let indent = "  ";
    let body_width = width.saturating_sub(display_width(indent)).max(8);
    for raw in content.lines() {
        if raw.is_empty() {
            lines.push(blank_line());
            continue;
        }
        for wrapped in wrap_line(raw, body_width) {
            lines.push(Line::from(vec![
                Span::styled(indent, Style::default().bg(solid_surface())),
                Span::styled(
                    wrapped,
                    Style::default().fg(color).bg(solid_surface()),
                ),
            ]));
        }
    }
}

fn wrap_line(line: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut wrapped = Vec::new();
    let mut current = String::new();
    for ch in line.chars() {
        if display_width(&current) >= width {
            wrapped.push(std::mem::take(&mut current));
        }
        current.push(ch);
    }
    if !current.is_empty() {
        wrapped.push(current);
    }
    if wrapped.is_empty() {
        wrapped.push(String::new());
    }
    wrapped
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
        let w = display_width(&ch.to_string());
        if used.saturating_add(w).saturating_add(1) > max_width {
            break;
        }
        used = used.saturating_add(w);
        out.push(ch);
    }
    out.push('…');
    out
}

fn status_color(status: BackgroundTaskStatus) -> Color {
    match status {
        BackgroundTaskStatus::Running => accent(),
        BackgroundTaskStatus::Completed => signal(),
        BackgroundTaskStatus::Failed | BackgroundTaskStatus::TimedOut => red(),
        BackgroundTaskStatus::Cancelled => muted(),
    }
}
