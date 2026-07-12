use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Span};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::TuiApp;
use crate::commands::{CommandRow, command_rows};
use crate::render::{
    clear_modal_area, command_row, fill_modal_surface, modal_block, modal_list_highlight_style,
};
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};
use crate::ui::list::render_scrollbar;
use crate::ui::{TextInputRenderSpec, render_text_input_line};

pub(crate) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
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

    render_text_input_line(
        frame,
        rows[0],
        TextInputRenderSpec {
            value: &app.command_filter,
            cursor: app.command_filter_cursor,
            placeholder: "type to search",
            prefix: "> ",
            focused: true,
            text_style: Style::default().fg(text()).bg(modal_bg()),
            placeholder_style: Style::default().fg(muted()).bg(modal_bg()),
            prefix_style: Style::default().fg(signal()).bg(modal_bg()),
            cursor_style: Style::default().fg(bg()).bg(signal()),
            background_style: Style::default().bg(modal_bg()),
        },
    );

    let command_list = command_rows(app);
    let selected_command = app
        .selected_command
        .min(command_list.len().saturating_sub(1));
    let command_width = rows[1].width as usize;
    let items = command_list
        .iter()
        .enumerate()
        .map(|(index, row)| match row {
            CommandRow::Section(title) => {
                let style = Style::default()
                    .fg(ghost())
                    .bg(modal_bg())
                    .add_modifier(Modifier::BOLD);
                ListItem::new(Span::styled(format!("— {title} —"), style)).style(style)
            }
            CommandRow::Item(command) => {
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
            }
        })
        .collect::<Vec<_>>();

    let offset = app
        .command_scroll
        .min(command_list.len().saturating_sub(rows[1].height as usize));
    let mut list_state = ListState::default().with_offset(offset).with_selected(
        (!command_list.is_empty()).then_some(app.hover_index.unwrap_or(selected_command)),
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
        command_list.len(),
        offset,
        crate::ui::interaction::ScrollTarget::Commands,
    );
    for (row_offset, index) in (offset..command_list.len())
        .take(rows[1].height as usize)
        .enumerate()
    {
        let label = match &command_list[index] {
            CommandRow::Section(title) => format!("section {title}"),
            CommandRow::Item(command) => format!("command {}", command.label),
        };
        app.register_hit(
            line_rect(rows[1], row_offset),
            20,
            label,
            HitAction::Command(index),
        );
    }
    frame.render_widget(
        Paragraph::new("").style(Style::default().fg(muted()).bg(modal_bg())),
        rows[2],
    );
}
