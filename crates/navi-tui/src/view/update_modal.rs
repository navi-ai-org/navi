//! Modal shown when an update is available (manual check or install confirm).

use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Paragraph, Wrap};

use crate::TuiApp;
use crate::render::{clear_modal_area, modal_block};
use crate::theme::*;

pub(crate) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    frame.render_widget(modal_block("Update available"), area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);

    let (title, body) = match &app.available_update {
        Some(info) => (
            format!("NAVI {} → {}", info.current_version, info.latest_version),
            info.body
                .as_deref()
                .unwrap_or("A newer release is ready to install via the official installer.")
                .to_string(),
        ),
        None => (
            "No pending update".to_string(),
            "You appear to be up to date.".to_string(),
        ),
    };

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            title,
            Style::default()
                .fg(signal())
                .bg(modal_bg())
                .add_modifier(Modifier::BOLD),
        )))
        .style(Style::default().bg(modal_bg())),
        rows[0],
    );

    // Keep release notes short for the modal.
    let clipped: String = body.chars().take(600).collect();
    frame.render_widget(
        Paragraph::new(clipped)
            .style(Style::default().fg(text()).bg(modal_bg()))
            .wrap(Wrap { trim: true }),
        rows[1],
    );

    let installing = if app.update_installing {
        "installing…  "
    } else {
        ""
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(installing, Style::default().fg(code_const()).bg(modal_bg())),
            Span::styled("enter", Style::default().fg(red()).bg(modal_bg())),
            Span::styled(" install  ·  ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled("a", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(
                " auto-update  ·  ",
                Style::default().fg(muted()).bg(modal_bg()),
            ),
            Span::styled("o", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(
                " release notes  ·  ",
                Style::default().fg(muted()).bg(modal_bg()),
            ),
            Span::styled("esc", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(" later", Style::default().fg(muted()).bg(modal_bg())),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[2],
    );
}
