use navi_sdk::clean_session_title;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Span};
use ratatui::style::Style;
use ratatui::text::Text;
use ratatui::widgets::{Clear, List, ListItem, Paragraph};

use crate::TuiApp;
use crate::render::modal_block;
use crate::session::format_session_timestamp;
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};
use crate::ui::list::render_scrollbar;

pub(super) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Memory");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(1)])
        .split(inner);

    if app.saved_sessions.is_empty() {
        frame.render_widget(
            Paragraph::new(Text::from(vec![
                ratatui::prelude::Line::from(""),
                ratatui::prelude::Line::from(Span::styled(
                    "No saved sessions",
                    Style::default().fg(muted()),
                )),
            ]))
            .style(Style::default().bg(panel())),
            rows[0],
        );
    } else {
        let height = rows[0].height as usize;
        let start = app.session_scroll.min(app.saved_sessions.len());
        let end = (start + height).min(app.saved_sessions.len());
        let items = app
            .saved_sessions
            .get(start..end)
            .unwrap_or(&[])
            .iter()
            .enumerate()
            .map(|(offset, snapshot)| {
                let index = start + offset;
                let selected = index == app.selected_session;
                let hovered = app.hover_index == Some(index);
                let style = if hovered || selected {
                    active_item_style()
                } else {
                    inactive_item_style()
                };

                let project = snapshot
                    .project
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| snapshot.project.to_string_lossy().to_string());
                let title = snapshot
                    .title
                    .as_deref()
                    .and_then(clean_session_title)
                    .unwrap_or_else(|| project.clone());
                let timestamp = format_session_timestamp(snapshot.updated_at);
                let event_count = snapshot.events.len();
                let label = format!("{timestamp}  {title}  ·  {project}  ·  {event_count} events");

                ListItem::new(Span::styled(label, style)).style(style)
            })
            .collect::<Vec<_>>();

        frame.render_widget(
            List::new(items).style(Style::default().bg(panel())),
            rows[0],
        );
        render_scrollbar(
            frame,
            app,
            rows[0],
            app.saved_sessions.len(),
            start,
            crate::ui::interaction::ScrollTarget::Sessions,
        );
        for (offset, snapshot) in app
            .saved_sessions
            .get(start..end)
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            let index = start + offset;
            app.register_hit(
                line_rect(rows[0], offset),
                20,
                format!("session {}", snapshot.id.as_str()),
                HitAction::Session(index),
            );
        }
    }

    frame.render_widget(
        Paragraph::new("del delete").style(Style::default().fg(text()).bg(panel())),
        rows[1],
    );
}
