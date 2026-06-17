use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::TuiApp;
use crate::theme::ThemePalette;
use crate::ui::interaction::HitAction;

pub(crate) fn draw_mcp_modal(f: &mut Frame, area: Rect, app: &mut TuiApp, palette: &ThemePalette) {
    let block = Block::default()
        .title(" MCP Servers ")
        .borders(Borders::ALL)
        .style(Style::default().fg(palette.text).bg(palette.bg));

    let inner_area = block.inner(area);
    f.render_widget(block, area);

    // Layout: Left (Server list), Right (Details/Tools)
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)].as_slice())
        .split(inner_area);

    let left_area = chunks[0];
    let right_area = chunks[1];

    let config_servers = &app.loaded_config.config.mcp.servers;
    let connected_servers_result = app.engine().list_mcp_servers(app.session_id.as_str());
    let connected_servers = connected_servers_result.unwrap_or_default();

    if config_servers.is_empty() {
        let msg = Paragraph::new("No MCP servers configured.")
            .style(Style::default().fg(palette.ghost))
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(msg, left_area);
        return;
    }

    // Ensure selected_server is within bounds
    if app.mcp_ui_state.selected_server >= config_servers.len() {
        app.mcp_ui_state.selected_server = config_servers.len().saturating_sub(1);
    }

    let mut list_items = Vec::new();
    for (i, server) in config_servers.iter().enumerate() {
        let is_connected = connected_servers.iter().any(|cs| cs.id == server.id);

        let status_symbol = if !server.enabled {
            "⏸ "
        } else if is_connected {
            "🟢 "
        } else {
            "🔴 "
        };

        let status_color = if !server.enabled {
            palette.ghost
        } else if is_connected {
            palette.signal
        } else {
            palette.red
        };

        let mut style = Style::default().fg(palette.text);
        if i == app.mcp_ui_state.selected_server && !app.mcp_ui_state.is_focused_on_tools {
            style = style.bg(palette.bg).add_modifier(Modifier::BOLD);
        }

        let line = Line::from(vec![
            Span::styled(status_symbol, Style::default().fg(status_color)),
            Span::styled(server.id.clone(), Style::default()),
        ]);

        list_items.push(ListItem::new(line).style(style));

        app.interaction_registry.borrow_mut().register(
            Rect {
                x: left_area.x,
                y: left_area.y + i.saturating_sub(app.mcp_ui_state.scroll) as u16,
                width: left_area.width,
                height: 1,
            },
            10,
            format!("Select MCP {}", server.id),
            HitAction::McpServer(i),
        );
    }

    let list = List::new(list_items);
    let mut state = ListState::default();
    state.select(Some(app.mcp_ui_state.selected_server));
    f.render_stateful_widget(list, left_area, &mut state);

    // Right side: Details
    if let Some(server) = config_servers.get(app.mcp_ui_state.selected_server) {
        let mut detail_lines = Vec::new();
        detail_lines.push(Line::from(vec![
            Span::styled("ID: ", Style::default().fg(palette.ghost)),
            Span::styled(
                server.id.clone(),
                Style::default()
                    .fg(palette.text)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        if let Some(url) = &server.url {
            detail_lines.push(Line::from(vec![
                Span::styled("Transport: ", Style::default().fg(palette.ghost)),
                Span::raw("HTTP/SSE"),
            ]));
            detail_lines.push(Line::from(vec![
                Span::styled("URL: ", Style::default().fg(palette.ghost)),
                Span::raw(url.clone()),
            ]));
        } else if let Some(cmd) = &server.command {
            detail_lines.push(Line::from(vec![
                Span::styled("Transport: ", Style::default().fg(palette.ghost)),
                Span::raw("Stdio"),
            ]));
            detail_lines.push(Line::from(vec![
                Span::styled("Command: ", Style::default().fg(palette.ghost)),
                Span::raw(cmd.clone()),
            ]));
        }

        detail_lines.push(Line::from(""));
        detail_lines.push(Line::from(Span::styled(
            "Tools:",
            Style::default()
                .fg(palette.ghost)
                .add_modifier(Modifier::BOLD),
        )));

        if let Some(connected_info) = connected_servers.iter().find(|cs| cs.id == server.id) {
            if connected_info.tools.is_empty() {
                detail_lines.push(Line::from(Span::styled(
                    "  No tools available.",
                    Style::default().fg(palette.ghost),
                )));
            } else {
                for (j, tool) in connected_info.tools.iter().enumerate() {
                    let mut style = Style::default().fg(palette.text);
                    if j == app.mcp_ui_state.selected_tool && app.mcp_ui_state.is_focused_on_tools {
                        style = style.bg(palette.panel).add_modifier(Modifier::BOLD);
                    }
                    detail_lines.push(Line::styled(format!("  • {}", tool), style));
                }
            }
        } else if server.enabled {
            detail_lines.push(Line::from(Span::styled(
                "  Connecting or failed...",
                Style::default().fg(palette.signal),
            )));
        } else {
            detail_lines.push(Line::from(Span::styled(
                "  Server disabled.",
                Style::default().fg(palette.ghost),
            )));
        }

        detail_lines.push(Line::from(""));
        let toggle_msg = if server.enabled {
            "Press Enter to disable server"
        } else {
            "Press Enter to enable server"
        };
        detail_lines.push(Line::from(Span::styled(
            toggle_msg,
            Style::default().fg(palette.accent),
        )));

        let details = Paragraph::new(detail_lines).wrap(Wrap { trim: false });
        f.render_widget(details, right_area);
    }
}
