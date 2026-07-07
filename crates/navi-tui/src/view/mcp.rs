use ratatui::layout::{Margin, Rect};
use ratatui::prelude::{Frame, Span};
use ratatui::style::Style;
use ratatui::widgets::{List, ListItem, ListState};

use crate::TuiApp;
use crate::render::{clear_modal_area, modal_block, modal_list_highlight_style};
use crate::theme::*;
use crate::ui::list::render_scrollbar;

pub(crate) fn render(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let block = modal_block("MCP Connections");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    let rows = app.mcp_rows();
    let selected = app.selected_mcp_row.min(rows.len().saturating_sub(1));

    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let is_selected = index == selected;

            match row {
                crate::app::McpRow::Server(server_idx) => {
                    let server = &app.mcp_servers[*server_idx];
                    let is_expanded =
                        app.mcp_expanded_server.as_deref() == Some(server.id.as_str());
                    let _status_icon = if server.is_enabled {
                        if server.is_connected { "●" } else { "○" }
                    } else {
                        " "
                    };

                    let toggle_icon = if server.is_enabled { "✓" } else { " " };

                    let name_style = if is_selected {
                        active_item_style()
                    } else if server.is_enabled {
                        Style::default().fg(text())
                    } else {
                        inactive_item_style()
                    };

                    let expand_icon = if is_expanded { "v" } else { ">" };
                    let label = format!(
                        " {} {} {} [{}]",
                        toggle_icon,
                        expand_icon,
                        server.id,
                        if server.is_connected {
                            "connected"
                        } else {
                            "disconnected"
                        }
                    );

                    ListItem::new(Span::styled(label, name_style)).style(name_style)
                }
                crate::app::McpRow::Tool(server_idx, tool_idx) => {
                    let tool = &app.mcp_servers[*server_idx].tools[*tool_idx];
                    let tool_icon = if tool.is_enabled { "✓" } else { " " };
                    let tool_style = if is_selected {
                        active_item_style()
                    } else if tool.is_enabled {
                        Style::default().fg(ghost())
                    } else {
                        inactive_item_style()
                    };
                    let label = format!("     {} {}", tool_icon, tool.name);
                    ListItem::new(Span::styled(label, tool_style)).style(tool_style)
                }
            }
        })
        .collect();

    let offset = app
        .mcp_scroll
        .min(rows.len().saturating_sub(inner.height as usize));
    let mut list_state = ListState::default()
        .with_offset(offset)
        .with_selected((!rows.is_empty()).then_some(selected));

    frame.render_stateful_widget(
        List::new(items)
            .highlight_style(modal_list_highlight_style())
            .highlight_symbol("> "),
        inner,
        &mut list_state,
    );

    render_scrollbar(
        frame,
        app,
        inner,
        rows.len(),
        offset,
        crate::ui::interaction::ScrollTarget::Mcp,
    );
}
