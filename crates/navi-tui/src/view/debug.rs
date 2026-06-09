use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::Style;
use ratatui::widgets::{Clear, Paragraph, Wrap};

use crate::TuiApp;
use crate::providers::{current_provider_credential_status, selected_provider_label};
use crate::render::modal_block;
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};

pub(super) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Debug");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(12), Constraint::Length(1)])
        .split(inner);

    let active_state = if app.has_stream_task() {
        "streaming"
    } else if app.has_tool_task() {
        "tool"
    } else if app.is_loading {
        "loading"
    } else {
        "idle"
    };
    let provider = selected_provider_label(app);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Log file: ", Style::default().fg(muted())),
            Span::styled(
                app.log_path().display().to_string(),
                Style::default().fg(text()),
            ),
        ]),
        Line::from(vec![
            Span::styled("Session:  ", Style::default().fg(muted())),
            Span::styled(
                app.session_id.as_str().to_string(),
                Style::default().fg(text()),
            ),
        ]),
        Line::from(vec![
            Span::styled("Project:  ", Style::default().fg(muted())),
            Span::styled(
                app.project_dir.display().to_string(),
                Style::default().fg(text()),
            ),
        ]),
        Line::from(vec![
            Span::styled("Model:    ", Style::default().fg(muted())),
            Span::styled(
                format!("{} via {}", app.loaded_config.config.model.name, provider),
                Style::default().fg(text()),
            ),
        ]),
        Line::from(vec![
            Span::styled("API key:  ", Style::default().fg(muted())),
            Span::styled(
                current_provider_credential_status(app),
                Style::default().fg(accent()),
            ),
        ]),
        Line::from(vec![
            Span::styled("State:    ", Style::default().fg(muted())),
            Span::styled(active_state, Style::default().fg(accent())),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Recent diagnostics",
            Style::default().fg(pink()),
        )),
    ];
    if app.diagnostics().is_empty() {
        lines.push(Line::from(Span::styled(
            "none",
            Style::default().fg(muted()),
        )));
    } else {
        for diagnostic in app.diagnostics().iter().rev().take(8) {
            lines.push(Line::from(Span::styled(
                diagnostic.clone(),
                Style::default().fg(text()),
            )));
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(text()).bg(panel()))
            .wrap(Wrap { trim: false }),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(panel())),
        rows[1],
    );
    app.register_hit(
        line_rect(rows[1], 0),
        20,
        "close debug",
        HitAction::CloseModal,
    );
}
