use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use crate::TuiApp;
use crate::commands::filtered_commands;
use crate::render::{command_row, command_scroll_offset, modal_block};
use crate::theme::*;

pub(super) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Commands");
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
            Constraint::Length(1),
        ])
        .split(inner);

    let filter = if app.command_filter.is_empty() {
        "type to filter"
    } else {
        app.command_filter.as_str()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(signal())),
            Span::styled(filter, Style::default().fg(muted())),
        ]))
        .style(Style::default().bg(panel())),
        rows[0],
    );

    let commands = filtered_commands(app);
    let selected_command = app.selected_command.min(commands.len().saturating_sub(1));
    let command_width = rows[1].width as usize;
    let items = commands
        .iter()
        .enumerate()
        .map(|(index, command)| {
            let selected = index == selected_command;
            let style = if selected {
                Style::default()
                    .fg(Color::White)
                    .bg(accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(text()).bg(panel())
            };

            let shortcut = command.shortcut.unwrap_or("");
            ListItem::new(Span::styled(
                command_row(command.label, shortcut, command_width),
                style,
            ))
            .style(style)
        })
        .collect::<Vec<_>>();

    let mut list_state = ListState::default()
        .with_offset(command_scroll_offset(
            selected_command,
            rows[1].height as usize,
        ))
        .with_selected((!commands.is_empty()).then_some(selected_command));
    frame.render_stateful_widget(
        List::new(items).style(Style::default().bg(panel())),
        rows[1],
        &mut list_state,
    );
    frame.render_widget(
        Paragraph::new("tab/↑↓ choose  •  enter confirm  •  esc cancel")
            .style(Style::default().fg(muted()).bg(panel())),
        rows[2],
    );
}
