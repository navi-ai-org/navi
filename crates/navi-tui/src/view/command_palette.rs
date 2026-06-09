use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::Style;
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::TuiApp;
use crate::commands::filtered_commands;
use crate::render::{
    clear_modal_area, command_row, fill_modal_surface, modal_block, modal_list_highlight_style,
};
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};
use crate::ui::list::render_scrollbar;

pub(super) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
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

    for row in rows.iter() {
        fill_modal_surface(frame, *row);
    }

    let filter = if app.command_filter.is_empty() {
        "type to search"
    } else {
        app.command_filter.as_str()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(signal())),
            Span::styled(filter, Style::default().fg(text())),
        ]))
        .style(Style::default().bg(modal_bg())),
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
            let hovered = app.hover_index == Some(index);
            let style = if hovered || selected {
                active_item_style()
            } else {
                inactive_item_style()
            };

            let shortcut = command.shortcut.unwrap_or("");
            ListItem::new(Span::styled(
                command_row(command.label, shortcut, command_width),
                style,
            ))
            .style(style)
        })
        .collect::<Vec<_>>();

    let offset = app
        .command_scroll
        .min(commands.len().saturating_sub(rows[1].height as usize));
    let mut list_state = ListState::default().with_offset(offset).with_selected(
        (!commands.is_empty()).then_some(app.hover_index.unwrap_or(selected_command)),
    );
    frame.render_stateful_widget(
        List::new(items)
            .style(Style::default().bg(modal_bg()))
            .highlight_style(modal_list_highlight_style()),
        rows[1],
        &mut list_state,
    );
    render_scrollbar(
        frame,
        app,
        rows[1],
        commands.len(),
        offset,
        crate::ui::interaction::ScrollTarget::Commands,
    );
    for (row_offset, index) in (offset..commands.len())
        .take(rows[1].height as usize)
        .enumerate()
    {
        app.register_hit(
            line_rect(rows[1], row_offset),
            20,
            format!("command {}", commands[index].label),
            HitAction::Command(index),
        );
    }
    frame.render_widget(
        Paragraph::new("").style(Style::default().fg(muted()).bg(modal_bg())),
        rows[2],
    );
}
