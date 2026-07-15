//! MCP servers modal — live status, split list/detail, light card chrome.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph, Wrap};

use crate::app::TuiApp;
use crate::render::{clear_modal_area, fill_modal_surface, modal_block};
use crate::state::McpLiveServer;
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};

pub(crate) fn draw_mcp_modal(f: &mut Frame, area: Rect, app: &mut TuiApp) {
    clear_modal_area(f, area);

    let title = if app.mcp_ui_state.loading {
        "MCP Servers · checking…"
    } else {
        "MCP Servers"
    };
    f.render_widget(modal_block(title), area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    if inner.width < 20 || inner.height < 4 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(1)])
        .split(inner);

    let body = chunks[0];
    let footer = chunks[1];

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(body);

    let left = cols[0];
    let right = cols[1].inner(Margin {
        horizontal: 1,
        vertical: 0,
    });

    // Vertical rule between panes
    if cols[1].x > 0 {
        let rule = Rect::new(cols[1].x.saturating_sub(1), body.y, 1, body.height);
        fill_modal_surface(f, rule);
        f.render_widget(
            Paragraph::new(
                (0..body.height)
                    .map(|_| {
                        Line::from(Span::styled(
                            "│",
                            Style::default().fg(ghost()).bg(modal_bg()),
                        ))
                    })
                    .collect::<Vec<_>>(),
            )
            .style(Style::default().bg(modal_bg())),
            rule,
        );
    }

    let servers = effective_servers(app);
    if servers.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                "No MCP servers configured.",
                Style::default().fg(muted()).bg(modal_bg()),
            )]))
            .style(Style::default().bg(modal_bg())),
            left,
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Add servers in ~/.config/navi/config.toml under [mcp]",
                Style::default().fg(ghost()).bg(modal_bg()),
            )))
            .style(Style::default().bg(modal_bg())),
            right,
        );
        render_footer(f, footer, false);
        return;
    }

    if app.mcp_ui_state.selected_server >= servers.len() {
        app.mcp_ui_state.selected_server = servers.len().saturating_sub(1);
    }

    let max_visible = left.height as usize;
    if app.mcp_ui_state.selected_server < app.mcp_ui_state.scroll {
        app.mcp_ui_state.scroll = app.mcp_ui_state.selected_server;
    }
    if max_visible > 0 && app.mcp_ui_state.selected_server >= app.mcp_ui_state.scroll + max_visible
    {
        app.mcp_ui_state.scroll = app
            .mcp_ui_state
            .selected_server
            .saturating_add(1)
            .saturating_sub(max_visible);
    }

    let mut list_items = Vec::new();
    for i in app.mcp_ui_state.scroll..servers.len() {
        if i >= app.mcp_ui_state.scroll + max_visible {
            break;
        }
        let server = &servers[i];
        let (dot, dot_color) = status_dot(server, app.mcp_ui_state.loading);
        let selected =
            i == app.mcp_ui_state.selected_server && !app.mcp_ui_state.is_focused_on_tools;
        let name_style = if selected {
            Style::default()
                .fg(text())
                .bg(modal_bg())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(text()).bg(modal_bg())
        };
        let line = Line::from(vec![
            Span::styled(
                format!("{dot} "),
                Style::default().fg(dot_color).bg(modal_bg()),
            ),
            Span::styled(server.id.clone(), name_style),
        ]);
        list_items.push(ListItem::new(line).style(Style::default().bg(modal_bg())));

        let visible_index = i - app.mcp_ui_state.scroll;
        app.register_hit(
            line_rect(left, visible_index),
            20,
            format!("Select MCP {}", server.id),
            HitAction::McpServer(i),
        );
    }

    f.render_widget(
        List::new(list_items).style(Style::default().bg(modal_bg())),
        left,
    );

    // Detail pane
    if let Some(server) = servers.get(app.mcp_ui_state.selected_server) {
        render_detail(f, app, right, server);
    }

    render_footer(f, footer, true);
}

fn effective_servers(app: &TuiApp) -> Vec<McpLiveServer> {
    if !app.mcp_ui_state.live.is_empty() {
        return app.mcp_ui_state.live.clone();
    }
    // Fallback before seed/probe: config only, unknown connection (not failed).
    app.loaded_config
        .config
        .mcp
        .servers
        .iter()
        .map(|s| McpLiveServer {
            id: s.id.clone(),
            enabled: s.enabled,
            connected: false,
            known: !s.enabled, // disabled is a known state
            tools: Vec::new(),
            command: s.command.clone(),
            args: s.args.clone(),
            url: s.url.clone(),
        })
        .collect()
}

fn status_dot(server: &McpLiveServer, loading: bool) -> (&'static str, ratatui::style::Color) {
    if !server.enabled {
        ("○", muted())
    } else if server.connected {
        ("●", accent())
    } else if !server.known || loading {
        // Pending — never paint red "failed" before we know.
        ("●", signal())
    } else {
        ("●", red())
    }
}

fn status_label(server: &McpLiveServer, loading: bool) -> (&'static str, ratatui::style::Color) {
    if !server.enabled {
        ("disabled", muted())
    } else if server.connected {
        ("connected", accent())
    } else if !server.known || loading {
        ("checking…", signal())
    } else {
        ("failed", red())
    }
}

fn render_detail(f: &mut Frame, app: &mut TuiApp, area: Rect, server: &McpLiveServer) {
    let mut lines: Vec<Line> = Vec::new();
    let loading = app.mcp_ui_state.loading;
    let (status_text, status_color) = status_label(server, loading);

    lines.push(Line::from(vec![
        Span::styled(
            server.id.clone(),
            Style::default()
                .fg(text())
                .bg(modal_bg())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default().bg(modal_bg())),
        Span::styled(
            status_text,
            Style::default()
                .fg(status_color)
                .bg(modal_bg())
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(""));

    if let Some(url) = &server.url {
        lines.push(kv("Transport", "HTTP/SSE"));
        lines.push(kv("URL", url));
    } else if let Some(cmd) = &server.command {
        lines.push(kv("Transport", "stdio"));
        let cmd_disp = if server.args.is_empty() {
            cmd.clone()
        } else {
            format!("{} {}", cmd, server.args.join(" "))
        };
        lines.push(kv("Command", &cmd_disp));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Tools",
        Style::default()
            .fg(ghost())
            .bg(modal_bg())
            .add_modifier(Modifier::BOLD),
    )));

    if (!server.known || loading) && server.tools.is_empty() && server.enabled {
        lines.push(Line::from(Span::styled(
            "  checking connection…",
            Style::default().fg(signal()).bg(modal_bg()),
        )));
    } else if !server.enabled {
        lines.push(Line::from(Span::styled(
            "  server disabled",
            Style::default().fg(muted()).bg(modal_bg()),
        )));
    } else if server.connected {
        if server.tools.is_empty() {
            lines.push(Line::from(Span::styled(
                "  connected · no tools advertised",
                Style::default().fg(muted()).bg(modal_bg()),
            )));
        } else {
            for (j, tool) in server.tools.iter().enumerate() {
                let selected =
                    j == app.mcp_ui_state.selected_tool && app.mcp_ui_state.is_focused_on_tools;
                let style = if selected {
                    Style::default()
                        .fg(text())
                        .bg(modal_bg())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(text()).bg(modal_bg())
                };
                // Show short tool name (strip server__ prefix when present).
                let short = tool
                    .rsplit_once("__")
                    .map(|(_, name)| name)
                    .unwrap_or(tool.as_str());
                lines.push(Line::from(Span::styled(format!("  · {short}"), style)));
                app.register_hit(
                    Rect {
                        x: area.x,
                        y: area.y.saturating_add(lines.len() as u16),
                        width: area.width,
                        height: 1,
                    },
                    15,
                    format!("Select tool {tool}"),
                    HitAction::McpTool(j),
                );
            }
        }
    } else {
        lines.push(Line::from(Span::styled(
            "  could not connect (see logs · press r to retry)",
            Style::default().fg(red()).bg(modal_bg()),
        )));
    }

    if let Some(err) = &app.mcp_ui_state.probe_error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  probe error: {err}"),
            Style::default().fg(red()).bg(modal_bg()),
        )));
    }

    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(modal_bg())),
        area,
    );
}

fn kv(key: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{key}: "),
            Style::default().fg(ghost()).bg(modal_bg()),
        ),
        Span::styled(
            value.to_string(),
            Style::default().fg(text()).bg(modal_bg()),
        ),
    ])
}

fn render_footer(f: &mut Frame, area: Rect, has_servers: bool) {
    let hints = if has_servers {
        Line::from(vec![
            Span::styled("[↑↓]", Style::default().fg(signal()).bg(modal_bg())),
            Span::styled(" select  ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled("[enter]", Style::default().fg(signal()).bg(modal_bg())),
            Span::styled(
                " enable/disable  ",
                Style::default().fg(muted()).bg(modal_bg()),
            ),
            Span::styled("[r]", Style::default().fg(signal()).bg(modal_bg())),
            Span::styled(" refresh  ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled("[esc]", Style::default().fg(signal()).bg(modal_bg())),
            Span::styled(" close", Style::default().fg(muted()).bg(modal_bg())),
        ])
    } else {
        Line::from(vec![
            Span::styled("[esc]", Style::default().fg(signal()).bg(modal_bg())),
            Span::styled(" close", Style::default().fg(muted()).bg(modal_bg())),
        ])
    };
    f.render_widget(
        Paragraph::new(hints).style(Style::default().bg(modal_bg())),
        area,
    );
}
