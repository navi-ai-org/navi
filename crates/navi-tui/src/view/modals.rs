use navi_sdk::{ToolInvocation, model_can_run_publicly};
use navi_sdk::NaviUsageReport;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::TuiApp;
use crate::keybindings::THINKING_OPTIONS;
use crate::providers::*;
use crate::render::*;
use crate::state::MessageAction;
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};
use crate::ui::list::render_scrollbar;
use crate::ui::text_input::next_char_boundary;

pub(super) fn render_api_key_entry(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let block = Block::new()
        .title(Line::from(vec![Span::styled(
            " Enter API Key ",
            Style::default().fg(signal()),
        )]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent()))
        .style(Style::default().fg(text()).bg(modal_bg()));
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
        .style(Style::default().bg(modal_bg())),
        rows[0],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Env var:   ", Style::default().fg(muted())),
            Span::styled(env_var, Style::default().fg(ghost())),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Paste your API key:",
            Style::default().fg(muted()),
        )))
        .style(Style::default().bg(modal_bg())),
        rows[3],
    );

    let key_display = api_key_input_line(app, rows[4].width as usize);
    frame.render_widget(
        Paragraph::new(key_display).style(Style::default().bg(modal_bg())),
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
        Paragraph::new(status).style(Style::default().bg(modal_bg())),
        rows[6],
    );

    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(modal_bg())),
        rows[7],
    );
    app.register_hit(
        line_rect(rows[7], 0),
        20,
        "cancel api key entry",
        HitAction::CloseModal,
    );
}

pub(super) fn render_oauth(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    frame.render_widget(modal_block("OAuth Login"), area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let Some(state) = &app.oauth_state else {
        return;
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Provider: ", Style::default().fg(muted())),
            Span::styled(
                state.provider_id.clone(),
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new("Complete login in your browser.")
            .style(Style::default().fg(text()).bg(modal_bg())),
        rows[1],
    );

    let link_style = Style::default()
        .fg(signal())
        .bg(modal_bg())
        .add_modifier(Modifier::UNDERLINED);
    let link_lines = wrap_plain(&state.verification_uri, rows[3].width as usize);
    for (offset, line) in link_lines.iter().take(rows[3].height as usize).enumerate() {
        let row = line_rect(rows[3], offset);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(line.clone(), link_style)))
                .style(Style::default().bg(modal_bg())),
            row,
        );
        app.register_hit(
            Rect::new(row.x, row.y, line.len().min(row.width as usize) as u16, 1),
            30,
            "open oauth link",
            HitAction::OAuthOpen,
        );
    }

    let help = if state.user_code.is_empty() {
        "c copy link     ctrl+o open browser     esc close"
    } else {
        "c copy link     ctrl+o open browser     esc close"
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(muted()).bg(modal_bg())),
        rows[4],
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
    clear_modal_area(frame, area);
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
            .style(Style::default().bg(modal_bg())),
        inner,
    );
    app.register_hit(
        line_rect(inner, 4),
        30,
        "approve tool",
        HitAction::ToolApprove,
    );
    app.register_hit(
        Rect::new(inner.x + 13, inner.y + 4, 10, 1),
        31,
        "deny tool",
        HitAction::ToolDeny,
    );
}

pub(super) fn render_thinking_picker(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
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
            let hovered = app.hover_index == Some(index);
            let current = *level == app.thinking_level;
            let style = if hovered || selected {
                active_item_style()
            } else {
                inactive_item_style()
            };

            let marker = if current { "● " } else { "  " };
            ListItem::new(Span::styled(format!("{}{}", marker, level.label()), style)).style(style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        List::new(items).style(Style::default().bg(modal_bg())),
        rows[0],
    );
    for (index, level) in THINKING_OPTIONS
        .iter()
        .enumerate()
        .take(rows[0].height as usize)
    {
        app.register_hit(
            line_rect(rows[0], index),
            20,
            format!("thinking {}", level.label()),
            HitAction::Key {
                code: crossterm::event::KeyCode::Enter,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
        );
    }
    frame.render_widget(
        Paragraph::new("").style(Style::default().fg(muted()).bg(modal_bg())),
        rows[1],
    );
}

pub(super) fn render_question(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let Some(question) = app.pending_questions.first() else {
        return;
    };

    clear_modal_area(frame, area);
    frame.render_widget(modal_block("Decision"), area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(inner);

    let pending = app.pending_questions.len();
    let eyebrow = if pending > 1 {
        format!("NAVI needs a decision  •  1/{pending} pending")
    } else {
        "NAVI needs a decision".to_string()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "? ",
                Style::default().fg(signal()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                eyebrow,
                Style::default().fg(signal()).add_modifier(Modifier::BOLD),
            ),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[0],
    );

    let kind = if question.request.multiple {
        "Choose one or more options, write your own answer, or deny explicitly."
    } else {
        "Choose an option, write your own answer, or deny explicitly."
    };
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(Span::styled(
                truncate_display(&question.request.question, 220),
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(kind, Style::default().fg(muted()))),
        ]))
        .wrap(Wrap { trim: false })
        .style(Style::default().bg(modal_bg())),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Options",
            Style::default().fg(muted()).add_modifier(Modifier::BOLD),
        )))
        .style(Style::default().bg(modal_bg())),
        rows[2],
    );

    let visible_rows = rows[3].height as usize;
    let option_count = question.request.options.len();
    let start = question
        .option_scroll
        .min(option_count.saturating_sub(visible_rows));
    let end = option_count.min(start.saturating_add(visible_rows));
    let items = (start..end)
        .filter_map(|row| {
            question
                .request
                .options
                .get(row)
                .map(|option| (row, option))
        })
        .map(|(row, option)| {
            let selected = row == question.selected_row;
            let hovered = app.hover_index == Some(row);
            let style = if hovered || selected {
                active_item_style()
            } else {
                inactive_item_style()
            };
            let mark = if question.request.multiple {
                if question.selected_options.get(row).copied().unwrap_or(false) {
                    "[x]"
                } else {
                    "[ ]"
                }
            } else if selected {
                "(*)"
            } else {
                "( )"
            };
            let number = format!("{}.", row + 1);
            let mut label = format!("{number:<3} {mark} {}", option.label);
            if let Some(description) = &option.description {
                label.push_str("  - ");
                label.push_str(description);
            }
            ListItem::new(Line::from(Span::styled(
                truncate_display(&label, 180),
                style,
            )))
            .style(style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).style(Style::default().bg(modal_bg())),
        rows[3],
    );
    render_scrollbar(
        frame,
        app,
        rows[3],
        option_count,
        start,
        crate::ui::interaction::ScrollTarget::QuestionOptions,
    );
    for (offset, row) in (start..end).enumerate() {
        if let Some(option) = question.request.options.get(row) {
            app.register_hit(
                line_rect(rows[3], offset),
                30,
                format!("question option {}", option.label),
                HitAction::QuestionOption(row),
            );
        }
    }

    let text_border = if question.selected_is_custom() {
        accent()
    } else {
        ghost()
    };
    let text_block = Block::new()
        .title(Line::from(Span::styled(
            " Your answer ",
            Style::default().fg(muted()),
        )))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(text_border))
        .style(Style::default().bg(modal_bg()));
    let text_inner = rows[4].inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    frame.render_widget(text_block, rows[4]);
    frame.render_widget(
        Paragraph::new(question_text_line(question)).style(Style::default().bg(modal_bg())),
        text_inner,
    );
    app.register_hit(rows[4], 30, "question text answer", HitAction::QuestionText);

    let deny_style = if question.selected_is_deny() {
        Style::default()
            .fg(Color::White)
            .bg(red())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(text()).bg(modal_bg())
    };
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            question_preview_line(question),
            Line::from(Span::styled(
                "[deny] Cannot answer this question",
                deny_style,
            )),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[5],
    );
    app.register_hit(
        line_rect(rows[5], 0),
        30,
        "send question answer",
        HitAction::QuestionSend,
    );
    app.register_hit(
        line_rect(rows[5], 1),
        30,
        "deny question",
        HitAction::QuestionDeny,
    );

    let footer = if question.selected_is_custom() {
        "type to edit  •  ←→ move"
    } else if question.request.multiple {
        "1-9 toggle  •  space toggle"
    } else {
        "1-9 select  •  type for text"
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(text()).bg(modal_bg())),
        rows[6],
    );
}

fn question_text_line(question: &crate::state::QuestionUiState) -> Line<'static> {
    let mut spans = vec![Span::styled(
        "> ",
        Style::default().fg(signal()).add_modifier(Modifier::BOLD),
    )];
    if question.custom_answer.is_empty() {
        if question.selected_is_custom() {
            spans.push(question_cursor_span(" "));
        }
        spans.push(Span::styled(
            "write a plain-text answer",
            Style::default().fg(ghost()),
        ));
        return Line::from(spans);
    }

    let mut cursor = question.custom_cursor.min(question.custom_answer.len());
    while !question.custom_answer.is_char_boundary(cursor) {
        cursor = cursor.saturating_sub(1);
    }
    let (before, rest) = question.custom_answer.split_at(cursor);
    spans.push(Span::styled(
        before.to_string(),
        Style::default().fg(text()),
    ));
    if question.selected_is_custom() {
        if rest.is_empty() {
            spans.push(question_cursor_span(" "));
        } else {
            let next = next_char_boundary(&question.custom_answer, cursor)
                .unwrap_or(question.custom_answer.len());
            let (cursor_text, after) = question.custom_answer[cursor..].split_at(next - cursor);
            spans.push(question_cursor_span(cursor_text.to_string()));
            spans.push(Span::styled(after.to_string(), Style::default().fg(text())));
        }
    } else {
        spans.push(Span::styled(rest.to_string(), Style::default().fg(text())));
    }
    Line::from(spans)
}

fn question_cursor_span(value: impl Into<String>) -> Span<'static> {
    Span::styled(
        value.into(),
        Style::default()
            .fg(bg())
            .bg(signal())
            .add_modifier(Modifier::BOLD),
    )
}

fn question_preview_line(question: &crate::state::QuestionUiState) -> Line<'static> {
    if question.selected_is_deny() {
        return Line::from(Span::styled(
            "Will deny this question.",
            Style::default().fg(red()).add_modifier(Modifier::BOLD),
        ));
    }
    let answers = question.selected_answers();
    if answers.is_empty() {
        return Line::from(Span::styled(
            "No answer selected yet.",
            Style::default().fg(muted()),
        ));
    }
    Line::from(vec![
        Span::styled("Will send: ", Style::default().fg(muted())),
        Span::styled(
            truncate_display(&answers.join(", "), 96),
            Style::default().fg(signal()).add_modifier(Modifier::BOLD),
        ),
    ])
}

pub(super) fn render_settings(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
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

    let settings_list: [(&str, String); 4] = [
        (
            "Show Reasoning",
            if app.show_thinking {
                "[x]".into()
            } else {
                "[ ]".into()
            },
        ),
        (
            "Compact Tool View",
            if !app.full_tool_view {
                "[x]".into()
            } else {
                "[ ]".into()
            },
        ),
        (
            "Compact Tool Rows",
            app.compact_tool_visible_limit.to_string(),
        ),
        ("Theme", format!("Select Theme ({})", app.theme_id.label())),
    ];

    let items = settings_list
        .iter()
        .enumerate()
        .map(|(index, (label, val))| {
            let selected = index == app.selected_setting;
            let hovered = app.hover_index == Some(index);
            let style = if hovered || selected {
                active_item_style()
            } else {
                inactive_item_style()
            };

            let line = if index >= 2 {
                format!("{label}: {val}")
            } else {
                format!("{val} {label}")
            };
            ListItem::new(Span::styled(line, style)).style(style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        List::new(items).style(Style::default().bg(modal_bg())),
        rows[0],
    );
    for (index, setting) in settings_list
        .iter()
        .enumerate()
        .take(rows[0].height as usize)
    {
        let action = if index == 3 {
            HitAction::ThemePicker
        } else {
            HitAction::Setting(index)
        };
        app.register_hit(
            line_rect(rows[0], index),
            20,
            format!("setting {}", setting.0),
            action,
        );
    }
    frame.render_widget(
        Paragraph::new("").style(Style::default().fg(muted()).bg(modal_bg())),
        rows[1],
    );
    app.register_hit(
        line_rect(rows[1], 0),
        20,
        "close settings",
        HitAction::CloseModal,
    );
}

pub(super) fn render_help_modal(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
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
        ("ctrl+t", "background tasks"),
        ("ctrl+b", "background agents"),
        ("ctrl+a", "select input text"),
        ("ctrl+v", "paste image"),
        ("tab", "refresh/provider actions"),
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
            .style(Style::default().fg(text()).bg(modal_bg()))
            .wrap(Wrap { trim: false }),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(modal_bg())),
        rows[1],
    );
    app.register_hit(
        line_rect(rows[1], 0),
        20,
        "close help",
        HitAction::CloseModal,
    );
}

pub(super) fn render_message_actions(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let block = modal_block("Message Actions");
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
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);

    let selected_text = app
        .message_action_target
        .and_then(|index| app.messages.get(index))
        .map(|message| truncate_display(&message.content.replace('\n', " "), 44))
        .unwrap_or_else(|| "message no longer available".to_string());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Selected: ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled(selected_text, Style::default().fg(text()).bg(modal_bg())),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[0],
    );
    frame.render_widget(Paragraph::new(""), rows[1]);

    let items = MessageAction::ALL
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let selected = index == app.selected_message_action;
            let hovered = app.hover_index == Some(index);
            let style = if hovered || selected {
                active_item_style()
            } else {
                inactive_item_style()
            };
            let description_style = if hovered || selected {
                active_item_style()
            } else {
                Style::default().fg(muted()).bg(modal_bg())
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<16}", action.label()), style),
                Span::styled(action.description(), description_style),
            ]))
            .style(style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).style(Style::default().bg(modal_bg())),
        rows[0],
    );

    for (index, action) in MessageAction::ALL.iter().enumerate() {
        app.register_hit(
            line_rect(rows[2], index),
            20,
            format!("message action {}", action.label()),
            HitAction::MessageAction(index),
        );
    }

    frame.render_widget(
        Paragraph::new("enter select · esc close")
            .style(Style::default().fg(muted()).bg(modal_bg())),
        rows[3],
    );
    app.register_hit(rows[3], 20, "close message actions", HitAction::CloseModal);
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

    clear_modal_area(frame, area);
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
    let line_count = lines.len();

    let body = Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .scroll((app.plugin_approval_scroll as u16, 0))
        .style(Style::default().bg(modal_bg()));
    frame.render_widget(body, rows[0]);
    render_scrollbar(
        frame,
        app,
        rows[0],
        line_count,
        app.plugin_approval_scroll,
        crate::ui::interaction::ScrollTarget::PluginApproval,
    );

    let blocked = req.reconsent_action.as_deref() == Some("BLOCKED");
    let footer = if blocked {
        Line::from(vec![
            Span::styled(
                "BLOCKED: ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "publisher change - update refused",
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
            Span::styled(" scroll", Style::default().fg(muted())),
        ])
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().bg(modal_bg())),
        rows[1],
    );
    if !blocked {
        app.register_hit(
            Rect::new(rows[1].x, rows[1].y, 12.min(rows[1].width), 1),
            30,
            "approve plugin",
            HitAction::PluginApprove,
        );
        app.register_hit(
            Rect::new(
                rows[1].x + 14.min(rows[1].width),
                rows[1].y,
                12.min(rows[1].width.saturating_sub(14)),
                1,
            ),
            31,
            "deny plugin",
            HitAction::PluginDeny,
        );
    }
}

pub(super) fn render_theme_picker(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let block = modal_block("Theme");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(inner);

    let filter = if app.theme_filter.is_empty() {
        "search"
    } else {
        app.theme_filter.as_str()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(signal())),
            Span::styled(filter, Style::default().fg(text())),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[0],
    );

    let filtered = filtered_theme_options(&app.theme_filter);
    let visible_selected = filtered
        .iter()
        .position(|(orig_index, _)| *orig_index == app.selected_theme)
        .unwrap_or(0);

    let items = filtered
        .iter()
        .map(|(orig_index, theme)| {
            let current = *theme == app.theme_id;
            let selected = *orig_index == app.selected_theme;
            let hovered = app.hover_index == Some(*orig_index);
            let style = if hovered || selected {
                active_item_style()
            } else {
                inactive_item_style()
            };

            let marker = if current { "● " } else { "  " };
            ListItem::new(Span::styled(format!("{}{}", marker, theme.label()), style)).style(style)
        })
        .collect::<Vec<_>>();

    let mut list_state = ListState::default()
        .with_offset(0)
        .with_selected((!filtered.is_empty()).then_some(visible_selected));
    frame.render_stateful_widget(
        List::new(items)
            .style(Style::default().bg(modal_bg()))
            .highlight_style(modal_list_highlight_style()),
        rows[1],
        &mut list_state,
    );
    for (row_index, (orig_index, theme)) in
        filtered.iter().take(rows[1].height as usize).enumerate()
    {
        app.register_hit(
            line_rect(rows[1], row_index),
            20,
            format!("theme {}", theme.label()),
            HitAction::ThemeSelect(*orig_index),
        );
    }
    frame.render_widget(
        Paragraph::new("").style(Style::default().fg(muted()).bg(modal_bg())),
        rows[2],
    );
    app.register_hit(
        line_rect(rows[2], 0),
        20,
        "close theme picker",
        HitAction::CloseModal,
    );
}

fn wrap_plain(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if current.chars().count() >= width {
            lines.push(current);
            current = String::new();
        }
        current.push(ch);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

pub(super) fn render_background_models(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let block = modal_block("Background Agents");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    let tasks: &[(&str, &str)] = &[
        ("naming", "Session title generation"),
        ("compaction", "Conversation summarization"),
        ("repo_search", "Repository exploration"),
        ("subagent_research", "Research subagents"),
        ("simple_code_edit", "Code edit subagents"),
    ];

    let mut rows: Vec<Line> = Vec::new();
    let bg = &app.loaded_config.config.background_models;

    for (i, (task_id, description)) in tasks.iter().enumerate() {
        let selected = i == app.bg_models_selected;
        let resolved_label = resolve_bg_model_label(app, task_id);
        let has_override = bg_model_has_override(bg, task_id);

        let task_style = if selected {
            Style::default().fg(signal()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(text())
        };
        let model_style = if selected {
            Style::default().fg(accent()).add_modifier(Modifier::BOLD)
        } else if has_override {
            Style::default().fg(accent())
        } else {
            Style::default().fg(muted())
        };
        let desc_style = Style::default().fg(ghost());

        rows.push(Line::from(vec![
            Span::styled(format!("{:>20}", task_id), task_style),
            Span::styled("  →  ", desc_style),
            Span::styled(resolved_label, model_style),
        ]));
        rows.push(Line::from(vec![Span::styled(
            format!("  {:>20}  ", description),
            desc_style,
        )]));
    }

    let list = Paragraph::new(rows).style(Style::default().bg(modal_bg()));
    frame.render_widget(list, inner);

    // Footer hints.
    let footer_y = inner.y + inner.height.saturating_sub(1);
    if footer_y > inner.y {
        let footer_rect = Rect::new(inner.x, footer_y, inner.width, 1);
        let hints = Line::from(vec![
            Span::styled("  [enter]", Style::default().fg(signal())),
            Span::styled(" pick model  ", Style::default().fg(muted())),
            Span::styled("[d]", Style::default().fg(signal())),
            Span::styled(" reset  ", Style::default().fg(muted())),
            Span::styled("[esc]", Style::default().fg(signal())),
            Span::styled(" close", Style::default().fg(muted())),
        ]);
        frame.render_widget(
            Paragraph::new(hints).style(Style::default().bg(modal_bg())),
            footer_rect,
        );
    }
}

fn resolve_bg_model_label(app: &TuiApp, task: &str) -> String {
    let bg = &app.loaded_config.config.background_models;
    if let Some(entry) = bg.resolve(task) {
        if let (Some(provider), Some(model)) = (&entry.provider, &entry.model) {
            return format!("{provider}:{model}");
        }
    }
    let main_provider = &app.loaded_config.config.model.provider;
    let main_model = &app.loaded_config.config.model.name;
    format!("{main_provider}:{main_model} (default)")
}

fn bg_model_has_override(bg: &navi_sdk::BackgroundModelsConfig, task: &str) -> bool {
    match task {
        "naming" => bg.naming.is_some(),
        "compaction" => bg.compaction.is_some(),
        "repo_search" => bg.repo_search.is_some(),
        "subagent_research" => bg.subagent_research.is_some(),
        "simple_code_edit" => bg.simple_code_edit.is_some(),
        _ => bg.default.is_some(),
    }
}

pub(super) fn render_usage(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    frame.render_widget(modal_block("Usage"), area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(12), Constraint::Length(1)])
        .split(inner);

    let mut lines: Vec<Line<'_>> = Vec::new();

    if app.usage_state.loading {
        lines.push(Line::from(Span::styled(
            "Loading usage data...",
            Style::default().fg(signal()),
        )));
    } else if let Some(ref error) = app.usage_state.error {
        lines.push(Line::from(Span::styled(
            format!("Error: {error}"),
            Style::default().fg(red()),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Usage windows are only available for OpenAI OAuth accounts.",
            Style::default().fg(muted()),
        )));
        lines.push(Line::from(Span::styled(
            "API key auth does not support this endpoint.",
            Style::default().fg(muted()),
        )));
    } else if let Some(ref report) = app.usage_state.report {
        render_usage_report(&mut lines, report);
    } else {
        lines.push(Line::from(Span::styled(
            "No usage data available.",
            Style::default().fg(muted()),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(text()).bg(modal_bg()))
            .wrap(Wrap { trim: false }),
        rows[0],
    );

    let hints = Line::from(vec![
        Span::styled("[r]", Style::default().fg(signal())),
        Span::styled(" refresh  ", Style::default().fg(muted())),
        Span::styled("[esc]", Style::default().fg(signal())),
        Span::styled(" close", Style::default().fg(muted())),
    ]);
    frame.render_widget(
        Paragraph::new(hints).style(Style::default().bg(modal_bg())),
        rows[1],
    );
}

fn render_usage_report(lines: &mut Vec<Line<'_>>, report: &NaviUsageReport) {
    // Header: provider + plan
    let plan_label = report.plan_type.as_deref().unwrap_or("unknown");
    lines.push(Line::from(vec![
        Span::styled(
            format!("{} ", report.provider_label),
            Style::default().fg(text()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("({plan_label})"),
            Style::default().fg(muted()),
        ),
    ]));

    if let Some(ref kind) = report.limit_reached_kind {
        lines.push(Line::from(Span::styled(
            format!("⚠ Limit reached: {kind}"),
            Style::default().fg(red()),
        )));
    }

    lines.push(Line::from(""));

    // Render each limit snapshot
    for (i, limit) in report.limits.iter().enumerate() {
        let name = limit
            .limit_name
            .as_deref()
            .or(limit.metered_feature.as_deref())
            .unwrap_or("Limit");

        let status_icon = if limit.limit_reached { "●" } else { "○" };
        let status_color = if limit.limit_reached { red() } else { signal() };
        lines.push(Line::from(vec![
            Span::styled(format!("{status_icon} "), Style::default().fg(status_color)),
            Span::styled(
                name.to_string(),
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
        ]));

        // Primary window (5h)
        if let Some(ref primary) = limit.primary {
            render_window_line(lines, "5h limit", primary);
        }
        // Secondary window (weekly)
        if let Some(ref secondary) = limit.secondary {
            render_window_line(lines, "Weekly", secondary);
        }

        // Spacing between limits
        if i < report.limits.len() - 1 {
            lines.push(Line::from(""));
        }
    }
}

fn render_window_line(lines: &mut Vec<Line<'_>>, label: &str, window: &navi_sdk::NaviUsageWindow) {
    let used = window.used_percent.clamp(0, 100) as u16;
    let remaining = 100u16.saturating_sub(used);

    // Build progress bar: 20 chars wide
    let bar_width: u16 = 20;
    let filled = (used as u32 * bar_width as u32 / 100) as u16;
    let empty = bar_width.saturating_sub(filled);
    let bar = format!("{}{}", "█".repeat(filled as usize), "░".repeat(empty as usize));

    let bar_color = if used >= 90 {
        red()
    } else if used >= 70 {
        Color::Yellow
    } else {
        signal()
    };

    let reset_text = format_reset(window.reset_after_seconds);

    lines.push(Line::from(vec![
        Span::styled(format!("  {label:8} "), Style::default().fg(muted())),
        Span::styled(bar, Style::default().fg(bar_color)),
        Span::styled(
            format!(" {remaining}% left"),
            Style::default().fg(text()),
        ),
    ]));
    if !reset_text.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:8} ", ""),
                Style::default().fg(muted()),
            ),
            Span::styled(
                format!("resets {reset_text}"),
                Style::default().fg(muted()),
            ),
        ]));
    }
}

/// Format reset_after_seconds into a human-friendly string.
fn format_reset(seconds: i32) -> String {
    if seconds <= 0 {
        return String::new();
    }
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours >= 24 {
        let days = hours / 24;
        let rem_hours = hours % 24;
        if rem_hours > 0 {
            format!("in {days}d {rem_hours}h")
        } else {
            format!("in {days}d")
        }
    } else if hours > 0 {
        if minutes > 0 {
            format!("in {hours}h {minutes}m")
        } else {
            format!("in {hours}h")
        }
    } else {
        format!("in {minutes}m")
    }
}
