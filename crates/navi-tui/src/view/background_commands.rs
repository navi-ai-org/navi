use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Paragraph, Wrap};

use navi_sdk::{BackgroundCommandSnapshot, BackgroundTaskStatus};

use crate::TuiApp;
use crate::background::{bg_status_label, format_bg_elapsed};
use crate::render::{clear_modal_area, modal_block};
use crate::theme::*;
use crate::ui::interaction::{HitAction, ScrollTarget, line_rect};
use crate::ui::list::render_scrollbar;

pub(crate) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    frame.render_widget(modal_block("Background Tasks"), area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    if app.background_commands.is_empty() {
        render_empty(frame, inner);
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(6),
            Constraint::Length(1),
        ])
        .split(inner);

    let running = app
        .background_commands
        .iter()
        .filter(|cmd| cmd.is_running())
        .count();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{} tasks", app.background_commands.len()),
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::default().fg(muted())),
            Span::styled(
                format!("{running} running"),
                Style::default().fg(if running > 0 { accent() } else { muted() }),
            ),
            Span::styled("  ", Style::default().fg(muted())),
            Span::styled("enter/click opens output", Style::default().fg(muted())),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[0],
    );

    let header = Line::from(vec![
        Span::styled("Status   ", Style::default().fg(ghost())),
        Span::styled("Task       ", Style::default().fg(ghost())),
        Span::styled("Elapsed   ", Style::default().fg(ghost())),
        Span::styled("Command", Style::default().fg(ghost())),
    ]);
    frame.render_widget(
        Paragraph::new(header).style(Style::default().bg(modal_bg())),
        line_rect(rows[1], 0),
    );

    let list_area = Rect::new(
        rows[1].x,
        rows[1].y.saturating_add(1),
        rows[1].width,
        rows[1].height.saturating_sub(1),
    );
    let visible = list_area.height as usize;
    let start = app
        .bg_command_scroll
        .min(app.background_commands.len().saturating_sub(visible));
    let end = (start + visible).min(app.background_commands.len());

    for (visual_row, index) in (start..end).enumerate() {
        let Some(cmd) = app.background_commands.get(index) else {
            continue;
        };
        let selected = index == app.bg_command_selected;
        let row_area = line_rect(list_area, visual_row);
        let line = task_row(cmd, selected, row_area.width as usize);
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(if selected {
                selection_bg()
            } else {
                modal_bg()
            })),
            row_area,
        );
        app.register_hit(
            row_area,
            35,
            format!("background task {}", cmd.task_id),
            HitAction::BackgroundCommand(index),
        );
    }

    render_scrollbar(
        frame,
        app,
        list_area,
        app.background_commands.len(),
        start,
        ScrollTarget::BackgroundCommands,
    );

    let has_running = app.background_commands.iter().any(|c| c.is_running());
    let footer = if has_running {
        "enter open  c cancel  r refresh  up/down select  esc close"
    } else {
        "enter open  r refresh  up/down select  esc close"
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(muted()).bg(modal_bg())),
        rows[2],
    );
}

pub(crate) fn render_output(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    frame.render_widget(modal_block("Background Task Output"), area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let Some(cmd) = app.background_commands.get(app.bg_command_selected) else {
        render_empty(frame, inner);
        return;
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(inner);

    render_output_header(frame, cmd, rows[0]);

    let output_width = rows[1].width.saturating_sub(1) as usize;
    let lines = output_lines(cmd, output_width);
    let visible = rows[1].height as usize;
    let max_scroll = lines.len().saturating_sub(visible);
    let offset = if app.bg_command_output_follow {
        max_scroll
    } else {
        app.bg_command_output_scroll.min(max_scroll)
    };
    let visible_lines = lines
        .iter()
        .skip(offset)
        .take(visible)
        .cloned()
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(visible_lines))
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(modal_bg())),
        rows[1],
    );
    render_scrollbar(
        frame,
        app,
        rows[1],
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
        format!("{follow}  up/down scroll  f/end tail  c cancel  r refresh  esc back")
    } else {
        format!("{follow}  up/down scroll  f/end tail  r refresh  esc back")
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(muted()).bg(modal_bg())),
        rows[2],
    );
}

fn render_empty(frame: &mut Frame<'_>, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "No background commands.",
            Style::default().fg(muted()),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Run bash with background=true to keep a long command alive.",
            Style::default().fg(muted()),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(text()).bg(modal_bg()))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn task_row(cmd: &BackgroundCommandSnapshot, selected: bool, width: usize) -> Line<'static> {
    let status_color = status_color(cmd.status);
    let elapsed = format_bg_elapsed(cmd);
    let fixed_width = 31usize;
    let command_width = width.saturating_sub(fixed_width).max(12);
    let command = truncate_chars(&cmd.command.replace('\n', " "), command_width);
    Line::from(vec![
        Span::styled(
            format!("{:<9}", bg_status_label(cmd)),
            Style::default().fg(status_color),
        ),
        Span::styled(
            format!("{:<11}", truncate_chars(&cmd.task_id, 10)),
            Style::default()
                .fg(if selected { accent() } else { text() })
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
        Span::styled(format!("{elapsed:<10}"), Style::default().fg(muted())),
        Span::styled(
            command,
            Style::default().fg(if selected { text() } else { muted() }),
        ),
    ])
}

fn render_output_header(frame: &mut Frame<'_>, cmd: &BackgroundCommandSnapshot, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);
    let status = bg_status_label(cmd);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Task ", Style::default().fg(muted())),
            Span::styled(
                cmd.task_id.clone(),
                Style::default().fg(accent()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::default().fg(muted())),
            Span::styled(status, Style::default().fg(status_color(cmd.status))),
            Span::styled("  ", Style::default().fg(muted())),
            Span::styled(format_bg_elapsed(cmd), Style::default().fg(muted())),
            exit_code_span(cmd),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Command ", Style::default().fg(muted())),
            Span::styled(
                truncate_chars(
                    &cmd.command.replace('\n', " "),
                    (rows[1].width as usize).saturating_sub(8),
                ),
                Style::default().fg(text()),
            ),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[1],
    );
    let description = cmd.description.as_deref().unwrap_or("");
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Desc ", Style::default().fg(muted())),
            Span::styled(
                if description.is_empty() {
                    "none".to_string()
                } else {
                    truncate_chars(description, (rows[2].width as usize).saturating_sub(5))
                },
                Style::default().fg(if description.is_empty() {
                    ghost()
                } else {
                    muted()
                }),
            ),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[2],
    );
}

fn exit_code_span(cmd: &BackgroundCommandSnapshot) -> Span<'static> {
    if let Some(code) = cmd.exit_code {
        Span::styled(format!("  exit {code}"), Style::default().fg(muted()))
    } else {
        Span::raw("")
    }
}

fn output_lines(cmd: &BackgroundCommandSnapshot, width: usize) -> Vec<Line<'static>> {
    let width = width.max(12);
    let mut lines = Vec::new();
    if let Some(error) = &cmd.error {
        lines.push(Line::from(Span::styled(
            "error",
            Style::default().fg(red()),
        )));
        push_wrapped(&mut lines, error, width, red());
        lines.push(Line::from(""));
    }
    push_stream(
        &mut lines,
        "stdout",
        &cmd.stdout,
        cmd.stdout_truncated,
        width,
        text(),
    );
    if !cmd.stdout.is_empty() && !cmd.stderr.is_empty() {
        lines.push(Line::from(""));
    }
    push_stream(
        &mut lines,
        "stderr",
        &cmd.stderr,
        cmd.stderr_truncated,
        width,
        red(),
    );
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Waiting for output...",
            Style::default().fg(muted()),
        )));
    }
    lines
}

fn push_stream(
    lines: &mut Vec<Line<'static>>,
    label: &'static str,
    content: &str,
    truncated: bool,
    width: usize,
    color: ratatui::style::Color,
) {
    if content.is_empty() {
        return;
    }
    lines.push(Line::from(Span::styled(
        label,
        Style::default().fg(accent()).add_modifier(Modifier::BOLD),
    )));
    push_wrapped(lines, content, width, color);
    if truncated {
        lines.push(Line::from(Span::styled(
            "[output truncated]",
            Style::default().fg(ghost()),
        )));
    }
}

fn push_wrapped(
    lines: &mut Vec<Line<'static>>,
    content: &str,
    width: usize,
    color: ratatui::style::Color,
) {
    for raw in content.lines() {
        if raw.is_empty() {
            lines.push(Line::from(""));
            continue;
        }
        for wrapped in wrap_line(raw, width) {
            lines.push(Line::from(Span::styled(
                wrapped,
                Style::default().fg(color),
            )));
        }
    }
}

fn wrap_line(line: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut wrapped = Vec::new();
    let mut current = String::new();
    for ch in line.chars() {
        if current.chars().count() >= width {
            wrapped.push(current);
            current = String::new();
        }
        current.push(ch);
    }
    if !current.is_empty() {
        wrapped.push(current);
    }
    wrapped
}

fn truncate_chars(value: &str, max_len: usize) -> String {
    if value.chars().count() <= max_len {
        return value.to_string();
    }
    let keep = max_len.saturating_sub(3);
    format!("{}...", value.chars().take(keep).collect::<String>())
}

fn status_color(status: BackgroundTaskStatus) -> ratatui::style::Color {
    match status {
        BackgroundTaskStatus::Running => accent(),
        BackgroundTaskStatus::Completed => signal(),
        BackgroundTaskStatus::Failed | BackgroundTaskStatus::TimedOut => red(),
        BackgroundTaskStatus::Cancelled => muted(),
    }
}
