use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::Style;
use ratatui::widgets::{Paragraph, Wrap};

use navi_sdk::BackgroundTaskStatus;

use crate::TuiApp;
use crate::background::{bg_status_label, format_bg_elapsed};
use crate::render::{clear_modal_area, modal_block};
use crate::theme::*;

pub(super) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let title = "Background Tasks";
    let block = modal_block(title);
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    if app.background_commands.is_empty() {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "No background commands.",
                Style::default().fg(muted()),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Use bash with background=true to run long-lived commands.",
                Style::default().fg(muted()),
            )),
        ];
        frame.render_widget(
            Paragraph::new(lines)
                .style(Style::default().fg(text()).bg(modal_bg()))
                .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(6), Constraint::Length(1)])
        .split(inner);

    // Build task list
    let mut lines = Vec::new();
    for (i, cmd) in app.background_commands.iter().enumerate() {
        let selected = i == app.bg_command_selected;
        let indicator = if cmd.is_running() { "●" } else { " " };
        let indicator_color = match cmd.status {
            BackgroundTaskStatus::Running => accent(),
            BackgroundTaskStatus::Completed => accent(),
            BackgroundTaskStatus::Failed | BackgroundTaskStatus::TimedOut => red(),
            BackgroundTaskStatus::Cancelled => muted(),
        };

        let status_str = bg_status_label(cmd);
        let elapsed_str = format_bg_elapsed(cmd);
        let command_display = truncate_command(&cmd.command, 30);

        let line = Line::from(vec![
            Span::styled(
                format!(" {indicator} "),
                Style::default().fg(indicator_color),
            ),
            Span::styled(
                format!("{:<8}", cmd.task_id),
                Style::default().fg(if selected { accent() } else { text() }),
            ),
            Span::styled(
                format!("{command_display:<32}"),
                Style::default().fg(if selected { text() } else { muted() }),
            ),
            Span::styled(
                format!("{status_str:<12}"),
                Style::default().fg(indicator_color),
            ),
            Span::styled(format!("{elapsed_str:>8}"), Style::default().fg(muted())),
        ]);
        lines.push(line);

        // Show expanded detail for selected task
        if selected && (!cmd.stdout.is_empty() || !cmd.stderr.is_empty() || cmd.error.is_some()) {
            if let Some(err) = &cmd.error {
                lines.push(Line::from(Span::styled(
                    format!("  error: {err}"),
                    Style::default().fg(red()),
                )));
            }
            if !cmd.stdout.is_empty() {
                let preview = first_n_lines(&cmd.stdout, 3);
                for line in preview.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("  │ {line}"),
                        Style::default().fg(ghost()),
                    )));
                }
                if cmd.stdout.lines().count() > 3 {
                    lines.push(Line::from(Span::styled(
                        format!("  │ ... ({} lines total)", cmd.stdout.lines().count()),
                        Style::default().fg(ghost()),
                    )));
                }
            }
            if !cmd.stderr.is_empty() {
                let preview = first_n_lines(&cmd.stderr, 2);
                for line in preview.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("  │ {line}"),
                        Style::default().fg(red()),
                    )));
                }
            }
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(text()).bg(modal_bg()))
            .wrap(Wrap { trim: false }),
        rows[0],
    );

    // Footer
    let has_running = app.background_commands.iter().any(|c| c.is_running());
    let footer_text = if has_running {
        "↑↓ navigate · c cancel · esc close"
    } else {
        "↑↓ navigate · esc close"
    };
    frame.render_widget(
        Paragraph::new(footer_text).style(Style::default().fg(muted()).bg(modal_bg())),
        rows[1],
    );
}

fn truncate_command(cmd: &str, max_len: usize) -> String {
    let cmd = cmd.replace('\n', " ");
    if cmd.len() <= max_len {
        cmd
    } else {
        format!("{}…", &cmd[..max_len.saturating_sub(1)])
    }
}

fn first_n_lines(text: &str, n: usize) -> String {
    text.lines().take(n).collect::<Vec<_>>().join("\n")
}
