use navi_core::{
    AgentMode, CompactThreshold, ToolInvocation, canonical_provider_id, model_can_run_publicly,
    provider_catalog,
};
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};

use crate::TuiApp;
use crate::commands::filtered_commands;
use crate::keybindings::THINKING_OPTIONS;
use crate::notifications::visible_notification;
use crate::providers::*;
use crate::render::*;
use crate::runtime::provider_supports_oauth;
use crate::session::*;
use crate::state::{ChatRole, Mode};
use crate::theme::*;
use crate::ui::text_input::{floor_char_boundary, next_char_boundary};

// ─── rendering ─────────────────────────────────────────────────────────────────
pub(crate) fn render(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = frame.area();
    frame.render_widget(Block::new().style(Style::default().bg(BG)), area);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(6),
            Constraint::Length(1),
            Constraint::Length(7),
        ])
        .split(area);

    render_chat_area(frame, app, vertical[0]);
    render_input(frame, app, vertical[2]);

    match app.mode {
        Mode::Commands => render_command_palette(frame, app, modal_rect(area, 68, 15)),
        Mode::Models => render_model_picker(frame, app, modal_rect(area, 72, 22)),
        Mode::ApiKeyEntry => render_api_key_entry(frame, app, modal_rect(area, 72, 11)),
        Mode::Thinking => render_thinking_picker(frame, app, modal_rect(area, 40, 10)),
        Mode::Sessions => render_sessions_picker(frame, app, modal_rect(area, 72, 16)),
        Mode::Settings => render_settings(frame, app, modal_rect(area, 50, 10)),
        Mode::Providers => render_provider_settings(frame, app, modal_rect(area, 76, 20)),
        Mode::Debug => render_debug_modal(frame, app, modal_rect(area, 76, 18)),
        Mode::Help => render_help_modal(frame, modal_rect(area, 62, 16)),
        Mode::Normal => {}
    }

    if !app.pending_approvals.is_empty() {
        render_tool_approval(frame, app, modal_rect(area, 72, 12));
    }

    render_notification(frame, app, area);
}

fn render_notification(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let Some(notification) = visible_notification(app) else {
        return;
    };

    let message_width = notification
        .message
        .chars()
        .count()
        .max(notification.title.chars().count())
        .saturating_add(8);
    let available_width = area.width.saturating_sub(4).max(1);
    let width = (message_width.clamp(26, 68) as u16).min(available_width);
    let height = area.height.min(3).max(1);
    let x = area.x + area.width.saturating_sub(width + 2);
    let y = area.y
        + area
            .height
            .saturating_sub(9)
            .min(area.height.saturating_sub(height));
    let rect = Rect::new(x, y, width, height);
    let inner = rect.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    frame.render_widget(Clear, rect);
    frame.render_widget(
        Block::new()
            .title(Line::from(vec![Span::styled(
                format!(" {} ", notification.title),
                Style::default().fg(PINK).add_modifier(Modifier::BOLD),
            )]))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(ACCENT))
            .style(Style::default().bg(PANEL)),
        rect,
    );
    frame.render_widget(
        Paragraph::new(notification.message.clone())
            .style(Style::default().fg(TEXT).bg(PANEL))
            .wrap(Wrap { trim: true }),
        inner,
    );
}

// ─── chat area ─────────────────────────────────────────────────────────────────
fn render_chat_area(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    app.chat_render_cache.borrow_mut().chat_rect = Some(inner);

    if app.messages.is_empty() && !app.is_loading {
        let welcome = welcome_text(app, inner.width as usize);
        frame.render_widget(
            Paragraph::new(welcome)
                .style(Style::default().bg(BG))
                .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }

    let chat_width = inner.width as usize;
    ensure_chat_cache(app, chat_width);
    let cache = app.chat_render_cache.borrow();
    let rendered_lines = &cache.lines;

    let visible_height = inner.height as usize;
    let total_lines = rendered_lines.len();
    let max_scroll = total_lines.saturating_sub(visible_height);
    let effective_scroll = app.scroll_offset.min(max_scroll);
    let start = total_lines
        .saturating_sub(visible_height)
        .saturating_sub(effective_scroll);
    let end = (start + visible_height).min(total_lines);

    let mut visible_lines: Vec<Line<'static>> = rendered_lines[start..end].to_vec();

    if let Some(selection) = &app.selection {
        let sel_start = selection.start.min(selection.end);
        let sel_end = selection.start.max(selection.end);

        for (idx, line) in visible_lines.iter_mut().enumerate() {
            let global_idx = start + idx;
            if global_idx >= sel_start.0 && global_idx <= sel_end.0 {
                let start_char = if global_idx == sel_start.0 {
                    sel_start.1
                } else {
                    0
                };
                let end_char = if global_idx == sel_end.0 {
                    sel_end.1
                } else {
                    usize::MAX
                };

                let mut new_spans = Vec::new();
                let mut current_char = 0;
                for span in line.spans.iter() {
                    let span_len = span.content.chars().count();
                    let span_end = current_char + span_len;

                    if span_end <= start_char || current_char >= end_char {
                        new_spans.push(span.clone());
                    } else if current_char >= start_char && span_end <= end_char {
                        new_spans.push(Span::styled(
                            span.content.clone(),
                            span.style.bg(Color::DarkGray),
                        ));
                    } else {
                        let c1 = start_char.saturating_sub(current_char).min(span_len);
                        let c2 = end_char.saturating_sub(current_char).min(span_len);

                        let s: String = span.content.chars().collect();

                        if c1 > 0 {
                            let p1: String = s.chars().take(c1).collect();
                            new_spans.push(Span::styled(p1, span.style));
                        }
                        if c2 > c1 {
                            let p2: String = s.chars().skip(c1).take(c2 - c1).collect();
                            new_spans.push(Span::styled(p2, span.style.bg(Color::DarkGray)));
                        }
                        if span_len > c2 {
                            let p3: String = s.chars().skip(c2).collect();
                            new_spans.push(Span::styled(p3, span.style));
                        }
                    }
                    current_char = span_end;
                }
                *line = Line::from(new_spans);
            }
        }
    }

    frame.render_widget(
        Paragraph::new(Text::from(visible_lines))
            .style(Style::default().bg(BG))
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn ensure_chat_cache(app: &TuiApp, chat_width: usize) {
    let signature = chat_render_signature(app);
    {
        let cache = app.chat_render_cache.borrow();
        if cache.width == chat_width
            && cache.full_tool_view == app.full_tool_view
            && cache.show_thinking == app.show_thinking
            && cache.signature == signature
        {
            return;
        }
    }

    let lines = build_chat_lines(app, chat_width);
    let mut cache = app.chat_render_cache.borrow_mut();
    cache.width = chat_width;
    cache.full_tool_view = app.full_tool_view;
    cache.show_thinking = app.show_thinking;
    cache.signature = signature;
    cache.lines = lines;
}

fn chat_render_signature(app: &TuiApp) -> String {
    let mut signature = String::with_capacity(app.messages.len() * 48);
    signature.push_str(if app.full_tool_view {
        "full|"
    } else {
        "compact|"
    });
    signature.push_str(if app.show_thinking { "think|" } else { "hide|" });
    for msg in &app.messages {
        signature.push(match msg.role {
            ChatRole::User => 'u',
            ChatRole::Assistant => 'a',
        });
        signature.push(':');
        signature.push_str(&msg.content.len().to_string());
        signature.push(':');
        signature.push_str(&msg.thinking_content.len().to_string());
        signature.push(':');
        signature.push_str(msg.status.as_deref().unwrap_or_default());
        signature.push(':');
        signature.push_str(msg.usage_label.as_deref().unwrap_or_default());
        signature.push(':');
        signature.push_str(&msg.elapsed_ms.unwrap_or_default().to_string());
        signature.push(':');
        signature.push_str(msg.model_label.as_deref().unwrap_or_default());
        signature.push(':');
        signature.push_str(msg.provider_label.as_deref().unwrap_or_default());
        if msg.is_compact_summary {
            signature.push_str(":compact");
        }
        if let Some(result) = &msg.tool_result {
            signature.push(':');
            signature.push_str(if result.ok { "ok" } else { "err" });
        }
        signature.push('|');
    }
    signature
}

pub(crate) fn build_chat_lines(app: &TuiApp, chat_width: usize) -> Vec<Line<'static>> {
    build_chat_lines_for_messages(
        app.messages.iter(),
        chat_width,
        app.full_tool_view,
        app.show_thinking,
    )
}

fn welcome_text(app: &TuiApp, width: usize) -> Text<'static> {
    let mut lines = Vec::new();
    let logo_width = NAVI_COMPACT_LOGO
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let project = project_label();
    let model = app.loaded_config.config.model.name.clone();
    let provider = selected_provider_label(app).to_string();
    let thinking = app.thinking_level.label();
    let context = app.compact_state.usage_label(app.input.len());
    let mode = format!("{:?}", app.loaded_config.config.harness.profile).to_lowercase();
    let router = "auto".to_string();
    let tools = "shell read write grep patch".to_string();
    let session = if app.conversation_history.len() <= 1 {
        "new"
    } else {
        "resumed"
    }
    .to_string();
    let cost = "$0.00".to_string();

    let status_width = [
        project.chars().count() + 10,
        model.chars().count() + provider.chars().count() + 9,
        thinking.len() + 13,
        context.len() + 9,
        mode.len() + 9,
        router.len() + 9,
        tools.len() + 9,
        session.len() + 9,
        cost.len() + 9,
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    let content_width = logo_width + 6 + status_width;
    let left_pad = width.saturating_sub(content_width) / 2;

    lines.push(Line::from(""));

    let total_lines = std::cmp::max(NAVI_COMPACT_LOGO.len(), 10);

    for index in 0..total_lines {
        let mut spans = Vec::new();

        if let Some(logo_line) = NAVI_COMPACT_LOGO.get(index) {
            let color = match (app.tick() / 5 + index as u64) % 4 {
                0 => PINK,
                1 => ACCENT,
                2 => Color::Rgb(236, 218, 255),
                _ => Color::Rgb(132, 20, 204),
            };
            spans.push(Span::styled(
                format!("{}{logo_line}", " ".repeat(left_pad)),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::raw(format!(
                "{}{}",
                " ".repeat(left_pad),
                " ".repeat(logo_width)
            )));
        }

        if let Some(status) = welcome_status_line(
            index, &project, &provider, &model, thinking, &context, &mode, &router, &tools,
            &session, &cost,
        ) {
            spans.push(Span::raw("      "));
            spans.extend(status);
        }
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        format!(
            "{}NAVI · wired code agent for local-first builders",
            " ".repeat(left_pad)
        ),
        Style::default().fg(MUTED),
    )]));

    Text::from(lines)
}

fn welcome_status_line(
    index: usize,
    project: &str,
    provider: &str,
    model: &str,
    thinking: &str,
    context: &str,
    mode: &str,
    router: &str,
    tools: &str,
    session: &str,
    cost: &str,
) -> Option<Vec<Span<'static>>> {
    match index {
        0 => Some(vec![
            Span::styled("project ", Style::default().fg(MUTED)),
            Span::styled(
                project.to_string(),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]),
        1 => Some(vec![
            Span::styled("model   ", Style::default().fg(MUTED)),
            Span::styled(
                model.to_string(),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]),
        2 => Some(vec![
            Span::styled("via     ", Style::default().fg(MUTED)),
            Span::styled(
                provider.to_string(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        ]),
        3 => Some(vec![
            Span::styled("thinking ", Style::default().fg(MUTED)),
            Span::styled(thinking.to_string(), Style::default().fg(TEXT)),
        ]),
        4 => Some(vec![
            Span::styled("context ", Style::default().fg(MUTED)),
            Span::styled(context.to_string(), Style::default().fg(TEXT)),
        ]),
        5 => Some(vec![
            Span::styled("mode    ", Style::default().fg(MUTED)),
            Span::styled(mode.to_string(), Style::default().fg(TEXT)),
        ]),
        6 => Some(vec![
            Span::styled("router  ", Style::default().fg(MUTED)),
            Span::styled(router.to_string(), Style::default().fg(TEXT)),
        ]),
        7 => Some(vec![
            Span::styled("tools   ", Style::default().fg(MUTED)),
            Span::styled(tools.to_string(), Style::default().fg(TEXT)),
        ]),
        8 => Some(vec![
            Span::styled("session ", Style::default().fg(MUTED)),
            Span::styled(session.to_string(), Style::default().fg(TEXT)),
        ]),
        9 => Some(vec![
            Span::styled("cost    ", Style::default().fg(MUTED)),
            Span::styled(cost.to_string(), Style::default().fg(TEXT)),
        ]),
        _ => None,
    }
}

// ─── input ─────────────────────────────────────────────────────────────────────
fn render_input(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    let input_lines = visible_input_lines(input_lines(app), rows[0].height as usize);
    frame.render_widget(
        Paragraph::new(Text::from(input_lines))
            .style(Style::default().bg(BG))
            .wrap(Wrap { trim: false })
            .block(Block::new().borders(Borders::NONE)),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(shortcut_tips(app, rows[1].width as usize)).style(Style::default().bg(BG)),
        rows[1],
    );
}

fn visible_input_lines(lines: Vec<Line<'_>>, height: usize) -> Vec<Line<'_>> {
    let height = height.max(1);
    let start = lines.len().saturating_sub(height);
    lines.into_iter().skip(start).collect()
}

fn input_lines(app: &TuiApp) -> Vec<Line<'_>> {
    let prompt = "> ";
    let continuation = " ".repeat(prompt.chars().count());
    let mut spans = vec![Span::styled(
        prompt,
        Style::default().fg(SIGNAL).add_modifier(Modifier::BOLD),
    )];

    if app.input.is_empty() {
        spans.push(cursor_span(" "));
        let placeholder = if app.is_loading {
            " Thinking..."
        } else {
            " Ready!"
        };
        spans.push(Span::styled(placeholder, Style::default().fg(MUTED)));
        return vec![Line::from(spans)];
    }

    let cursor = app.input_cursor.min(app.input.len());
    let cursor = floor_char_boundary(&app.input, cursor);
    let (before, rest) = app.input.split_at(cursor);
    spans.push(Span::styled(before, Style::default().fg(TEXT)));

    if rest.is_empty() {
        spans.push(cursor_span(" "));
    } else {
        let next = next_char_boundary(&app.input, cursor).unwrap_or(app.input.len());
        let (cursor_text, after) = app.input[cursor..].split_at(next - cursor);
        spans.push(cursor_span(cursor_text));
        spans.push(Span::styled(after, Style::default().fg(TEXT)));
    }

    split_input_spans(spans, &continuation)
}

fn shortcut_tips(app: &TuiApp, width: usize) -> Line<'static> {
    let agent_label = app.selected_agent.map(AgentMode::label).unwrap_or("none");
    if app.messages.is_empty() && app.conversation_history.len() <= 1 && app.input.is_empty() {
        return Line::from(vec![
            Span::styled(" ", Style::default().fg(MUTED)),
            Span::styled(
                "type a task, /plan, /edit, /review, or ",
                Style::default().fg(MUTED),
            ),
            Span::styled(
                "ctrl+p",
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" for commands; ", Style::default().fg(MUTED)),
            Span::styled(
                "tab",
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" changes agent ({agent_label})"),
                Style::default().fg(MUTED),
            ),
        ]);
    }

    let items = [
        ("?", "for shortcuts", TEXT),
        ("ctrl+p", "commands", TEXT),
        ("tab", agent_label, TEXT),
        ("ctrl+c", "quit", TEXT),
    ];

    let mut spans = vec![Span::styled(" ", Style::default().fg(MUTED))];
    let mut used = 3usize;

    for (index, (key, label, key_color)) in items.iter().enumerate() {
        let item_width = key.chars().count()
            + if label.is_empty() {
                0
            } else {
                1 + label.chars().count()
            };
        let separator_width = if index == 0 { 0 } else { 5 };
        if used + separator_width + item_width > width {
            break;
        }
        if index > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(GHOST)));
            used += separator_width;
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::default().fg(*key_color).add_modifier(Modifier::BOLD),
        ));
        used += key.chars().count();
        if !label.is_empty() {
            spans.push(Span::styled(
                format!(" {label}"),
                Style::default().fg(MUTED),
            ));
            used += 1 + label.chars().count();
        }
    }

    let compact_state = &app.compact_state;
    let threshold = compact_state.threshold_level(app.input.len());
    let pct_label = format!(" {}", compact_state.usage_label(app.input.len()));
    let pct_color = match threshold {
        CompactThreshold::CircuitOpen => SIGNAL,
        CompactThreshold::Error => SIGNAL,
        CompactThreshold::Warning => ACCENT,
        CompactThreshold::Normal => MUTED,
    };
    let threshold_label = match threshold {
        CompactThreshold::CircuitOpen => " ⚠circuit",
        CompactThreshold::Error => " ⚠compact",
        CompactThreshold::Warning => " ~compact",
        CompactThreshold::Normal => "",
    };
    let context_text = format!("ctx:{pct_label}{threshold_label}");
    let context_width = context_text.chars().count();
    if used + context_width + 2 < width {
        let padding = width.saturating_sub(used + context_width + 1);
        spans.push(Span::styled(
            " ".repeat(padding),
            Style::default().fg(MUTED),
        ));
        spans.push(Span::styled(format!("ctx:"), Style::default().fg(MUTED)));
        spans.push(Span::styled(pct_label, Style::default().fg(pct_color)));
        if !threshold_label.is_empty() {
            spans.push(Span::styled(
                threshold_label.to_string(),
                Style::default().fg(pct_color),
            ));
        }
    }

    Line::from(spans)
}

// ─── api key entry modal ───────────────────────────────────────────────────────
fn render_api_key_entry(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
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

// ─── thinking picker ───────────────────────────────────────────────────────────
fn render_tool_approval(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
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

fn render_thinking_picker(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
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

fn render_settings(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
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

fn render_provider_settings(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    frame.render_widget(modal_block("Provider Accounts"), area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(10),
            Constraint::Length(2),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new("Configure API keys or OAuth sign-in for supported providers.")
            .style(Style::default().fg(MUTED).bg(PANEL)),
        rows[0],
    );

    let providers = provider_catalog(&app.loaded_config.config);
    let height = rows[1].height as usize;
    let start = app.provider_settings_scroll.min(providers.len());
    let end = (start + height).min(providers.len());
    let items = providers[start..end]
        .iter()
        .enumerate()
        .map(|(offset, provider)| {
            let index = start + offset;
            let selected = index == app.selected_provider_setting;
            let status = provider_auth_status(app, provider);
            let oauth = if provider_supports_oauth(&provider.id) {
                "OAuth"
            } else {
                "API key"
            };
            let line = format!(
                "{:<18} {:<12} {:<10} {}",
                provider.label, status.label, oauth, provider.description
            );
            let style = if selected {
                Style::default()
                    .fg(Color::White)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else if status.configured {
                Style::default().fg(SIGNAL).bg(PANEL)
            } else {
                Style::default().fg(MUTED).bg(PANEL)
            };
            ListItem::new(Span::styled(line, style)).style(style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(List::new(items).style(Style::default().bg(PANEL)), rows[1]);

    frame.render_widget(
        Paragraph::new("enter/k API key  •  o OAuth  •  r sync models  •  esc close")
            .style(Style::default().fg(MUTED).bg(PANEL)),
        rows[2],
    );
}

fn render_debug_modal(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Debug");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(12), Constraint::Length(1)])
        .split(inner);

    let active_state = if app.has_stream_task() {
        "streaming"
    } else if app.has_tool_task() {
        "tool"
    } else if app.is_loading {
        "loading"
    } else {
        "idle"
    };
    let provider = selected_provider_label(app);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Log file: ", Style::default().fg(MUTED)),
            Span::styled(
                app.log_path().display().to_string(),
                Style::default().fg(TEXT),
            ),
        ]),
        Line::from(vec![
            Span::styled("Session:  ", Style::default().fg(MUTED)),
            Span::styled(app.session_id.0.clone(), Style::default().fg(TEXT)),
        ]),
        Line::from(vec![
            Span::styled("Project:  ", Style::default().fg(MUTED)),
            Span::styled(
                app.project_dir.display().to_string(),
                Style::default().fg(TEXT),
            ),
        ]),
        Line::from(vec![
            Span::styled("Model:    ", Style::default().fg(MUTED)),
            Span::styled(
                format!("{} via {}", app.loaded_config.config.model.name, provider),
                Style::default().fg(TEXT),
            ),
        ]),
        Line::from(vec![
            Span::styled("API key:  ", Style::default().fg(MUTED)),
            Span::styled(
                current_provider_credential_status(app),
                Style::default().fg(ACCENT),
            ),
        ]),
        Line::from(vec![
            Span::styled("State:    ", Style::default().fg(MUTED)),
            Span::styled(active_state, Style::default().fg(ACCENT)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Recent diagnostics",
            Style::default().fg(PINK),
        )),
    ];
    if app.diagnostics().is_empty() {
        lines.push(Line::from(Span::styled("none", Style::default().fg(MUTED))));
    } else {
        for diagnostic in app.diagnostics().iter().rev().take(8) {
            lines.push(Line::from(Span::styled(
                diagnostic.clone(),
                Style::default().fg(TEXT),
            )));
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(TEXT).bg(PANEL))
            .wrap(Wrap { trim: false }),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new("esc close").style(Style::default().fg(MUTED).bg(PANEL)),
        rows[1],
    );
}

fn render_help_modal(frame: &mut Frame<'_>, area: Rect) {
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

// ─── sessions picker ───────────────────────────────────────────────────────────
fn render_sessions_picker(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Memory");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(1)])
        .split(inner);

    if app.saved_sessions.is_empty() {
        frame.render_widget(
            Paragraph::new(Text::from(vec![
                Line::from(""),
                Line::from(Span::styled(
                    "No saved sessions",
                    Style::default().fg(MUTED),
                )),
            ]))
            .style(Style::default().bg(PANEL)),
            rows[0],
        );
    } else {
        let height = rows[0].height as usize;
        let start = app.session_scroll.min(app.saved_sessions.len());
        let end = (start + height).min(app.saved_sessions.len());
        let items = app
            .saved_sessions
            .get(start..end)
            .unwrap_or(&[])
            .iter()
            .enumerate()
            .map(|(offset, snapshot)| {
                let index = start + offset;
                let selected = index == app.selected_session;
                let style = if selected {
                    Style::default()
                        .fg(Color::White)
                        .bg(ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(TEXT).bg(PANEL)
                };

                let project = snapshot
                    .project
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| snapshot.project.to_string_lossy().to_string());
                let title = snapshot
                    .title
                    .as_deref()
                    .and_then(clean_session_title)
                    .unwrap_or_else(|| project.clone());
                let timestamp = format_session_timestamp(snapshot.updated_at);
                let event_count = snapshot.events.len();
                let label = format!("{timestamp}  {title}  ·  {project}  ·  {event_count} events");

                ListItem::new(Span::styled(label, style)).style(style)
            })
            .collect::<Vec<_>>();

        frame.render_widget(List::new(items).style(Style::default().bg(PANEL)), rows[0]);
    }

    frame.render_widget(
        Paragraph::new("↑↓ choose  •  enter load  •  del delete  •  esc cancel")
            .style(Style::default().fg(MUTED).bg(PANEL)),
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

// ─── command palette ───────────────────────────────────────────────────────────
fn render_command_palette(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
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
            Span::styled("> ", Style::default().fg(SIGNAL)),
            Span::styled(filter, Style::default().fg(MUTED)),
        ]))
        .style(Style::default().bg(PANEL)),
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
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT).bg(PANEL)
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
        List::new(items).style(Style::default().bg(PANEL)),
        rows[1],
        &mut list_state,
    );
    frame.render_widget(
        Paragraph::new("tab/↑↓ choose  •  enter confirm  •  esc cancel")
            .style(Style::default().fg(MUTED).bg(PANEL)),
        rows[2],
    );
}

// ─── model picker ──────────────────────────────────────────────────────────────
fn render_model_picker(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Switch Protocol");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(inner);

    let filter_text = if app.model_filter.is_empty() {
        "search providers or models"
    } else {
        app.model_filter.as_str()
    };
    frame.render_widget(
        Paragraph::new(Text::from(vec![Line::from(vec![
            Span::styled("> ", Style::default().fg(SIGNAL)),
            Span::styled(
                filter_text,
                Style::default().fg(if app.model_filter.is_empty() {
                    MUTED
                } else {
                    TEXT
                }),
            ),
        ])]))
        .style(Style::default().bg(PANEL)),
        rows[0],
    );

    let list_rows = build_model_rows(app);
    let list_area = rows[1];
    let row_width = list_area.width as usize;

    let selected_row = selected_model_in_rows(&list_rows, app.selected_model).unwrap_or(0);
    let mut list_state = ListState::default()
        .with_offset(app.model_scroll)
        .with_selected(Some(selected_row));

    let items = list_rows
        .iter()
        .map(|row| match row {
            ListRow::Header { label, .. } => {
                let header_style = Style::default()
                    .fg(TEXT)
                    .bg(PANEL)
                    .add_modifier(Modifier::BOLD);
                let refresh_style = Style::default().fg(GHOST).bg(PANEL);

                let mut spans = vec![Span::styled(format!("  {}", label), header_style)];
                spans.push(Span::styled("  ↻ tab", refresh_style));
                ListItem::new(Line::from(spans)).style(header_style)
            }
            ListRow::Model { index } => {
                let model = &app.models[*index];
                let selected = *index == app.selected_model;
                let configured = model.name == app.loaded_config.config.model.name
                    && canonical_provider_id(&model.provider_id)
                        == canonical_provider_id(&app.loaded_config.config.model.provider);
                let style = if selected {
                    Style::default()
                        .fg(Color::White)
                        .bg(ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(TEXT).bg(PANEL)
                };

                ListItem::new(Span::styled(
                    model_row_simple(model.name.as_str(), configured, row_width),
                    style,
                ))
                .style(style)
            }
        })
        .collect::<Vec<_>>();

    frame.render_stateful_widget(
        List::new(items).style(Style::default().bg(PANEL)),
        list_area,
        &mut list_state,
    );
    frame.render_widget(
        Paragraph::new(
            "type search  •  ↑↓ choose  •  ctrl+e edit setup  •  tab refresh provider  •  ctrl+r refresh all  •  enter confirm  •  esc exit",
        )
        .style(Style::default().fg(MUTED).bg(PANEL)),
        rows[2],
    );
}
