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
            Style::default().fg(SIGNAL),
        )]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().fg(TEXT).bg(PANEL));
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
            Span::styled("Provider:  ", Style::default().fg(MUTED)),
            Span::styled(
                format!("{provider_label} ({provider_id})"),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]))
        .style(Style::default().bg(PANEL)),
        rows[0],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Env var:   ", Style::default().fg(MUTED)),
            Span::styled(env_var, Style::default().fg(GHOST)),
        ]))
        .style(Style::default().bg(PANEL)),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Paste your API key:",
            Style::default().fg(MUTED),
        )))
        .style(Style::default().bg(PANEL)),
        rows[3],
    );

    let key_display = api_key_input_line(app, rows[4].width as usize);
    frame.render_widget(
        Paragraph::new(key_display).style(Style::default().bg(PANEL)),
        rows[4],
    );

    let status = if provider_has_api_key(app, &provider_id) {
        Line::from(Span::styled(
            "● Provider connected",
            Style::default().fg(SIGNAL),
        ))
    } else if app
        .pending_model_selection
        .and_then(|index| app.models.get(index))
        .is_some_and(|model| model_can_run_publicly(&model.provider_id, &model.name))
    {
        Line::from(Span::styled(
            "● Free model access available without key",
            Style::default().fg(SIGNAL),
        ))
    } else {
        Line::from(Span::styled(
            "○ No key configured",
            Style::default().fg(RED),
        ))
    };
    frame.render_widget(
        Paragraph::new(status).style(Style::default().bg(PANEL)),
        rows[6],
    );

    frame.render_widget(
        Paragraph::new("enter save  •  esc cancel").style(Style::default().fg(MUTED).bg(PANEL)),
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
            Span::styled("Tool: ", Style::default().fg(MUTED)),
            Span::styled(
                invocation.tool_name.clone(),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            truncate_display(&input, 420),
            Style::default().fg(SIGNAL),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("y", Style::default().fg(TEXT).add_modifier(Modifier::BOLD)),
            Span::styled(" approve  •  ", Style::default().fg(MUTED)),
            Span::styled("n", Style::default().fg(TEXT).add_modifier(Modifier::BOLD)),
            Span::styled(" deny  •  ", Style::default().fg(MUTED)),
            Span::styled(
                "ctrl+g",
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" yolo mode", Style::default().fg(MUTED)),
        ]),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(PANEL)),
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
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT).bg(PANEL)
            };

            let marker = if current { "● " } else { "  " };
            ListItem::new(Span::styled(format!("{}{}", marker, level.label()), style)).style(style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(List::new(items).style(Style::default().bg(PANEL)), rows[0]);
    frame.render_widget(
        Paragraph::new("↑↓ choose  •  enter confirm  •  esc cancel")
            .style(Style::default().fg(MUTED).bg(PANEL)),
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

    let settings_list = [
        ("Show Reasoning", Some(app.show_thinking)),
        ("Verbose Tool Output", Some(app.full_tool_view)),
    ];

    let items = settings_list
        .iter()
        .enumerate()
        .map(|(index, (label, val))| {
            let selected = index == app.selected_setting;
            let style = if selected {
                Style::default()
                    .fg(Color::White)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT).bg(PANEL)
            };

            let prefix = match val {
                Some(true) => "[x] ",
                Some(false) => "[ ] ",
                None => "› ",
            };
            ListItem::new(Span::styled(format!("{}{}", prefix, label), style)).style(style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(List::new(items).style(Style::default().bg(PANEL)), rows[0]);
    frame.render_widget(
        Paragraph::new("↑↓ choose  •  enter configure/toggle  •  esc close")
            .style(Style::default().fg(MUTED).bg(PANEL)),
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
                Span::styled(format!("{key:<12}"), Style::default().fg(SIGNAL)),
                Span::styled(*label, Style::default().fg(TEXT)),
            ])
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(TEXT).bg(PANEL))
            .wrap(Wrap { trim: false }),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new("enter/?/esc close").style(Style::default().fg(MUTED).bg(PANEL)),
        rows[1],
    );
}

fn api_key_input_line(app: &TuiApp, _max_width: usize) -> Line<'_> {
    let mut spans = vec![Span::styled("> ", Style::default().fg(SIGNAL))];

    if app.api_key_input.is_empty() {
        spans.push(Span::styled(" ", Style::default().fg(BG).bg(SIGNAL)));
        spans.push(Span::styled(" sk-...", Style::default().fg(GHOST)));
        return Line::from(spans);
    }

    let cursor = app.api_key_cursor.min(app.api_key_input.len());
    let (before, rest) = app.api_key_input.split_at(cursor);

    let display_before = mask_key_segment(before);
    spans.push(Span::styled(display_before, Style::default().fg(TEXT)));

    if rest.is_empty() {
        spans.push(Span::styled(" ", Style::default().fg(BG).bg(SIGNAL)));
    } else {
        let next =
            next_char_boundary(&app.api_key_input, cursor).unwrap_or(app.api_key_input.len());
        let (cursor_ch, after) = rest.split_at(next - cursor);
        spans.push(Span::styled(cursor_ch, Style::default().fg(BG).bg(SIGNAL)));
        let display_after = mask_key_segment(after);
        spans.push(Span::styled(display_after, Style::default().fg(TEXT)));
    }

    Line::from(spans)
}
