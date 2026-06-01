use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use crate::TuiApp;
use crate::render::{command_scroll_offset, modal_block};
use crate::theme::{ACCENT, MUTED, PANEL, SIGNAL, TEXT};

pub(super) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Skills");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(6),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(inner);

    // Filter input
    let filter = if app.skill_filter.is_empty() {
        "type to filter"
    } else {
        app.skill_filter.as_str()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(SIGNAL)),
            Span::styled(filter, Style::default().fg(MUTED)),
        ]))
        .style(Style::default().bg(PANEL)),
        rows[0],
    );

    // Skills list
    let skills = app.filtered_skills();
    let selected = app.selected_skill.min(skills.len().saturating_sub(1));

    let items: Vec<ListItem> = skills
        .iter()
        .enumerate()
        .map(|(index, skill)| {
            let is_active = app.is_skill_active(&skill.id);
            let selected_style = index == selected;

            let (name_style, status_icon) = if selected_style {
                (
                    Style::default()
                        .fg(Color::White)
                        .bg(ACCENT)
                        .add_modifier(Modifier::BOLD),
                    if is_active { "✓" } else { " " },
                )
            } else if is_active {
                (Style::default().fg(SIGNAL).bg(PANEL), "✓")
            } else {
                (Style::default().fg(TEXT).bg(PANEL), " ")
            };

            let description = skill
                .description
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(50)
                .collect::<String>();

            let version = skill.version.as_deref().unwrap_or("");
            let meta = if version.is_empty() {
                description.clone()
            } else {
                format!("{} [{}]", description, version)
            };

            let label = format!(
                " {} {:<width$} {}",
                status_icon,
                skill.name,
                meta,
                width = 24
            );

            ListItem::new(Span::styled(label, name_style)).style(name_style)
        })
        .collect();

    let mut list_state = ListState::default()
        .with_offset(command_scroll_offset(selected, rows[1].height as usize))
        .with_selected((!skills.is_empty()).then_some(selected));
    frame.render_stateful_widget(
        List::new(items).style(Style::default().bg(PANEL)),
        rows[1],
        &mut list_state,
    );

    // Active skills summary
    let active_count = app.active_skills.len();
    let total_count = app.available_skills.len();
    let summary = format!(" {} active / {} available ", active_count, total_count);
    frame.render_widget(
        Paragraph::new(summary).style(Style::default().fg(MUTED).bg(PANEL)),
        rows[2],
    );

    // Footer
    frame.render_widget(
        Paragraph::new("tab/↑↓ choose  •  enter toggle  •  esc close")
            .style(Style::default().fg(MUTED).bg(PANEL)),
        rows[3],
    );
}
