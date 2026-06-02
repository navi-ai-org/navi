use navi_sdk::{ToolInvocation, model_can_run_publicly};
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::TuiApp;
use crate::keybindings::THINKING_OPTIONS;
use crate::providers::*;
use crate::render::*;
use crate::theme::*;
use crate::ui::text_input::next_char_boundary;

pub(super) fn render_api_key_entry(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = Block::new()
        .title(Line::from(vec![Span::styled(
            " Enter API Key ",
            Style::default().fg(signal()),
        )]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent()))
        .style(Style::default().fg(text()).bg(panel()));
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let provider_id = selected_or_pending_provider_id(app);
    let provider_label = selected_or_pending_provider_label(app);
    let env_var = current_provider_env_var(app);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Provider:  ", Style::default().fg(muted())),
            Span::styled(
                format!("{provider_label} ({provider_id})"),
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
        ]))
        .style(Style::default().bg(panel())),
        rows[0],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Env var:   ", Style::default().fg(muted())),
            Span::styled(env_var, Style::default().fg(ghost())),
        ]))
        .style(Style::default().bg(panel())),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Paste your API key:",
            Style::default().fg(muted()),
        )))
        .style(Style::default().bg(panel())),
        rows[3],
    );

    let key_display = api_key_input_line(app, rows[4].width as usize);
    frame.render_widget(
        Paragraph::new(key_display).style(Style::default().bg(panel())),
        rows[4],
    );

    let status = if provider_has_api_key(app, &provider_id) {
        Line::from(Span::styled(
            "● Provider connected",
            Style::default().fg(signal()),
        ))
    } else if app
        .pending_model_selection
        .and_then(|index| app.models.get(index))
        .is_some_and(|model| model_can_run_publicly(&model.provider_id, &model.name))
    {
        Line::from(Span::styled(
            "● Free model access available without key",
            Style::default().fg(signal()),
        ))
    } else {
        Line::from(Span::styled(
            "○ No key configured",
            Style::default().fg(red()),
        ))
    };
    frame.render_widget(
        Paragraph::new(status).style(Style::default().bg(panel())),
        rows[6],
    );

    frame.render_widget(
        Paragraph::new("enter save  •  esc cancel").style(Style::default().fg(muted()).bg(panel())),
        rows[7],
    );
}

pub(super) fn render_tool_approval(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let Some(req) = app.pending_approvals.first() else {
        return;
    };
    let default_inv;
    let invocation = if let Some(inv) = app.tool_invocations.get(&req.id) {
        inv
    } else {
        default_inv = ToolInvocation {
            id: req.id.clone(),
            tool_name: "unknown".to_string(),
            input: serde_json::json!({ "summary": req.summary }),
        };
        &default_inv
    };
    frame.render_widget(Clear, area);
    let block = modal_block("Tool Approval");
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    frame.render_widget(block, area);

    let input = serde_json::to_string_pretty(&invocation.input)
        .unwrap_or_else(|_| invocation.input.to_string());
    let text = Text::from(vec![
        Line::from(vec![
            Span::styled("Tool: ", Style::default().fg(muted())),
            Span::styled(
                invocation.tool_name.clone(),
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            truncate_display(&input, 420),
            Style::default().fg(signal()),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "y",
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" approve  •  ", Style::default().fg(muted())),
            Span::styled(
                "n",
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" deny  •  ", Style::default().fg(muted())),
            Span::styled(
                "ctrl+g",
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" yolo mode", Style::default().fg(muted())),
        ]),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(panel())),
        inner,
    );
}

pub(super) fn render_thinking_picker(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Thinking Mode");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(1)])
        .split(inner);

    let items = THINKING_OPTIONS
        .iter()
        .enumerate()
        .map(|(index, level)| {
            let selected = index == app.selected_thinking;
            let current = *level == app.thinking_level;
            let style = if selected {
                Style::default()
                    .fg(Color::White)
                    .bg(accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(text()).bg(panel())
            };

            let marker = if current { "● " } else { "  " };
            ListItem::new(Span::styled(format!("{}{}", marker, level.label()), style)).style(style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        List::new(items).style(Style::default().bg(panel())),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new("↑↓ choose  •  enter confirm  •  esc cancel")
            .style(Style::default().fg(muted()).bg(panel())),
        rows[1],
    );
}

pub(super) fn render_settings(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Settings");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(1)])
        .split(inner);

    let settings_list: [(&str, String); 3] = [
        (
            "Show Reasoning",
            if app.show_thinking {
                "[x]".into()
            } else {
                "[ ]".into()
            },
        ),
        (
            "Verbose Tool Output",
            if app.full_tool_view {
                "[x]".into()
            } else {
                "[ ]".into()
            },
        ),
        ("Theme", app.theme_id.label().to_string()),
    ];

    let items = settings_list
        .iter()
        .enumerate()
        .map(|(index, (label, val))| {
            let selected = index == app.selected_setting;
            let style = if selected {
                Style::default()
                    .fg(Color::White)
                    .bg(accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(text()).bg(panel())
            };

            let line = if index == 2 {
                format!("{label}: {val}")
            } else {
                format!("{val} {label}")
            };
            ListItem::new(Span::styled(line, style)).style(style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        List::new(items).style(Style::default().bg(panel())),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new("↑↓ choose  •  enter configure/toggle  •  esc close")
            .style(Style::default().fg(muted()).bg(panel())),
        rows[1],
    );
}

pub(super) fn render_help_modal(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Shortcuts");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(12), Constraint::Length(1)])
        .split(inner);
    let shortcuts = [
        ("ctrl+p", "commands"),
        ("tab", "agent"),
        ("ctrl+m", "models"),
        ("ctrl+n", "new layer"),
        ("ctrl+s", "memory"),
        ("ctrl+o", "compact/full tool output"),
        ("ctrl+d", "debug"),
        ("ctrl+g", "toggle YOLO mode"),
        ("ctrl+enter", "send prompt"),
        ("enter", "new line"),
        ("ctrl+j", "new line"),
        ("/", "commands when input is empty"),
        ("?", "shortcuts"),
        ("esc", "cancel/close"),
    ];
    let lines = shortcuts
        .iter()
        .map(|(key, label)| {
            Line::from(vec![
                Span::styled(format!("{key:<12}"), Style::default().fg(signal())),
                Span::styled(*label, Style::default().fg(text())),
            ])
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(text()).bg(panel()))
            .wrap(Wrap { trim: false }),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new("enter/?/esc close").style(Style::default().fg(muted()).bg(panel())),
        rows[1],
    );
}

fn api_key_input_line(app: &TuiApp, _max_width: usize) -> Line<'_> {
    let mut spans = vec![Span::styled("> ", Style::default().fg(signal()))];

    if app.api_key_input.is_empty() {
        spans.push(Span::styled(" ", Style::default().fg(bg()).bg(signal())));
        spans.push(Span::styled(" sk-...", Style::default().fg(ghost())));
        return Line::from(spans);
    }

    let cursor = app.api_key_cursor.min(app.api_key_input.len());
    let (before, rest) = app.api_key_input.split_at(cursor);

    let display_before = mask_key_segment(before);
    spans.push(Span::styled(display_before, Style::default().fg(text())));

    if rest.is_empty() {
        spans.push(Span::styled(" ", Style::default().fg(bg()).bg(signal())));
    } else {
        let next =
            next_char_boundary(&app.api_key_input, cursor).unwrap_or(app.api_key_input.len());
        let (cursor_ch, after) = rest.split_at(next - cursor);
        spans.push(Span::styled(
            cursor_ch,
            Style::default().fg(bg()).bg(signal()),
        ));
        let display_after = mask_key_segment(after);
        spans.push(Span::styled(display_after, Style::default().fg(text())));
    }

    Line::from(spans)
}

pub(super) fn render_plugin_approval(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    use crate::state::PluginApprovalKind;

    let Some(req) = app.pending_plugin_approvals.first() else {
        return;
    };

    let title = match req.kind {
        PluginApprovalKind::Install => "Plugin Install",
        PluginApprovalKind::Update => "Plugin Update",
    };

    frame.render_widget(Clear, area);
    let block = modal_block(title);
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(1)])
        .split(inner);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Plugin: ", Style::default().fg(muted())),
        Span::styled(
            format!("{} v{}", req.plugin_id, req.version),
            Style::default().fg(text()).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Publisher: ", Style::default().fg(muted())),
        Span::styled(req.publisher.clone(), Style::default().fg(text())),
    ]));
    let risk_color = match req.overall_risk.as_str() {
        "CRITICAL" => Color::Red,
        "HIGH" => Color::LightRed,
        "MEDIUM" => Color::Yellow,
        _ => Color::Green,
    };
    lines.push(Line::from(vec![
        Span::styled("Overall risk: ", Style::default().fg(muted())),
        Span::styled(
            req.overall_risk.clone(),
            Style::default().fg(risk_color).add_modifier(Modifier::BOLD),
        ),
    ]));

    if let Some(action) = &req.reconsent_action {
        let color = if action == "BLOCKED" {
            Color::Red
        } else if action == "REQUIRE RECONSENT" {
            Color::Yellow
        } else {
            Color::Green
        };
        lines.push(Line::from(vec![
            Span::styled("Update action: ", Style::default().fg(muted())),
            Span::styled(
                action.clone(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    if !req.capabilities_text.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Capabilities:",
            Style::default().fg(signal()).add_modifier(Modifier::BOLD),
        )));
        for line in req.capabilities_text.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default().fg(text()),
            )));
        }
    }

    if !req.tools_text.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Tools:",
            Style::default().fg(signal()).add_modifier(Modifier::BOLD),
        )));
        for line in req.tools_text.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default().fg(text()),
            )));
        }
    }

    if !req.changes_text.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Changes:",
            Style::default().fg(signal()).add_modifier(Modifier::BOLD),
        )));
        for line in req.changes_text.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default().fg(text()),
            )));
        }
    }

    if !req.warnings_text.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Warnings:",
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        )));
        for line in req.warnings_text.lines() {
            lines.push(Line::from(Span::styled(
                format!("  ! {line}"),
                Style::default().fg(Color::LightRed),
            )));
        }
    }

    lines.push(Line::from(""));

    let body = Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .scroll((app.plugin_approval_scroll as u16, 0))
        .style(Style::default().bg(panel()));
    frame.render_widget(body, rows[0]);

    let blocked = req.reconsent_action.as_deref() == Some("BLOCKED");
    let footer = if blocked {
        Line::from(vec![
            Span::styled(
                "BLOCKED: ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "publisher change - update refused (esc close)",
                Style::default().fg(muted()),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                "y",
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" install  •  ", Style::default().fg(muted())),
            Span::styled(
                "n",
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" deny  •  ", Style::default().fg(muted())),
            Span::styled(
                "↑↓",
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" scroll  •  ", Style::default().fg(muted())),
            Span::styled(
                "esc",
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" cancel", Style::default().fg(muted())),
        ])
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().bg(panel())),
        rows[1],
    );
}
