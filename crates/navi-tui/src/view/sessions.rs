use navi_sdk::clean_session_title;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Span};
use ratatui::style::Style;
use ratatui::text::Text;
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::TuiApp;
use crate::app::session_belongs_to_project;
use crate::render::{clear_modal_area, modal_block};
use crate::session::format_session_timestamp;
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};
use crate::ui::list::render_scrollbar;

pub(super) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let block = modal_block("Memory");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(inner);

    // Filter input
    let filter = if app.session_filter.is_empty() {
        "search"
    } else {
        app.session_filter.as_str()
    };
    frame.render_widget(
        Paragraph::new(ratatui::prelude::Line::from(vec![
            Span::styled("> ", Style::default().fg(signal())),
            Span::styled(filter, Style::default().fg(text())),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[0],
    );

    let sessions = app.filtered_sessions();

    if sessions.is_empty() {
        frame.render_widget(
            Paragraph::new(Text::from(vec![
                ratatui::prelude::Line::from(""),
                ratatui::prelude::Line::from(Span::styled(
                    if app.session_filter.is_empty() {
                        "No saved sessions"
                    } else {
                        "No matching sessions"
                    },
                    Style::default().fg(muted()),
                )),
            ]))
            .style(Style::default().bg(modal_bg())),
            rows[1],
        );
    } else {
        let height = rows[1].height as usize;
        let start = app.session_scroll.min(sessions.len());
        let end = (start + height).min(sessions.len());
        let items = sessions
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
                let scope = if session_belongs_to_project(&snapshot.project, &app.project_dir) {
                    "current project"
                } else {
                    "other project"
                };
                let label = format!(
                    "{timestamp}  {title}  ·  {project}  ·  {scope}  ·  {event_count} events"
                );

                ListItem::new(Span::styled(label, style)).style(style)
            })
            .collect::<Vec<_>>();

        frame.render_widget(
            List::new(items).style(Style::default().bg(modal_bg())),
            rows[1],
        );
        render_scrollbar(
            frame,
            app,
            rows[1],
            sessions.len(),
            start,
            crate::ui::interaction::ScrollTarget::Sessions,
        );
        for (offset, snapshot) in sessions.get(start..end).unwrap_or(&[]).iter().enumerate() {
            let index = start + offset;
            app.register_hit(
                line_rect(rows[1], offset),
                20,
                format!("session {}", snapshot.id.as_str()),
                HitAction::Session(index),
            );
        }
    }

    frame.render_widget(
        Paragraph::new("del delete").style(Style::default().fg(text()).bg(modal_bg())),
        rows[2],
    );
}
