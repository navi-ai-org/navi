use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use crate::TuiApp;
use crate::render::modal_block;
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};
use crate::ui::list::render_scrollbar;

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
            Span::styled("> ", Style::default().fg(signal())),
            Span::styled(filter, Style::default().fg(muted())),
        ]))
        .style(Style::default().bg(panel())),
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
            let is_selected = index == selected;
            let is_hovered = app.hover_index == Some(index);

            let (name_style, status_icon) = if is_hovered {
                (
                    Style::default()
                        .fg(Color::White)
                        .bg(accent())
                        .add_modifier(Modifier::BOLD),
                    if is_active { "✓" } else { " " },
                )
            } else if is_selected {
                (
                    Style::default()
                        .fg(signal())
                        .bg(panel())
                        .add_modifier(Modifier::BOLD),
                    if is_active { "✓" } else { " " },
                )
            } else if is_active {
                (Style::default().fg(signal()).bg(panel()), "✓")
            } else {
                (Style::default().fg(muted()).bg(panel()), " ")
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

    let offset = app
        .skill_scroll
        .min(skills.len().saturating_sub(rows[1].height as usize));
    let mut list_state = ListState::default()
        .with_offset(offset)
        .with_selected((!skills.is_empty()).then_some(app.hover_index.unwrap_or(selected)));
    frame.render_stateful_widget(
        List::new(items)
            .style(Style::default().bg(panel()))
            .highlight_style(Style::default()),
        rows[1],
        &mut list_state,
    );
    render_scrollbar(
        frame,
        app,
        rows[1],
        skills.len(),
        offset,
        crate::ui::interaction::ScrollTarget::Skills,
    );
    for (row_offset, index) in (offset..skills.len())
        .take(rows[1].height as usize)
        .enumerate()
    {
        app.register_hit(
            line_rect(rows[1], row_offset),
            20,
            format!("skill {}", skills[index].name),
            HitAction::Skill(index),
        );
    }

    // Active skills summary
    let active_count = app.active_skills.len();
    let total_count = app.available_skills.len();
    let summary = format!(" {} active / {} available ", active_count, total_count);
    frame.render_widget(
        Paragraph::new(summary).style(Style::default().fg(muted()).bg(panel())),
        rows[2],
    );

    // Footer
    frame.render_widget(
        Paragraph::new("tab/↑↓ choose  •  enter toggle  •  esc close")
            .style(Style::default().fg(muted()).bg(panel())),
        rows[3],
    );
}
