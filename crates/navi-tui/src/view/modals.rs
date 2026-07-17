use navi_sdk::NaviUsageReport;
use navi_sdk::{ToolInvocation, model_can_run_publicly};
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::TuiApp;

use crate::providers::*;
use crate::render::*;
use crate::state::MessageAction;
use crate::theme::*;
use crate::ui::interaction::{HitAction, line_rect};
use crate::ui::list::render_scrollbar;
// selection_fg/bg via theme::*
use crate::ui::{
    TextInputRenderSpec, floor_char_boundary, next_char_boundary, render_text_input_line,
};

pub(crate) fn render_sudo_password(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    frame.render_widget(modal_block("Sudo password"), area);
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
        ])
        .split(inner);

    let prompt = app.sudo_password_prompt.as_ref();
    let summary = prompt.map(|p| p.command_summary.as_str()).unwrap_or("sudo");
    let password = prompt.map(|p| p.password.as_str()).unwrap_or("");
    let cursor = prompt.map(|p| p.cursor).unwrap_or(0);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "A command needs elevated privileges.",
            Style::default().fg(muted()).bg(modal_bg()),
        )))
        .style(Style::default().bg(modal_bg())),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Command: ", Style::default().fg(ghost()).bg(modal_bg())),
            Span::styled(
                summary.to_string(),
                Style::default().fg(text()).bg(modal_bg()),
            ),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Password is never shown to the model or saved in chat history.",
            Style::default().fg(ghost()).bg(modal_bg()),
        )))
        .style(Style::default().bg(modal_bg())),
        rows[2],
    );

    let masked: String = "*".repeat(password.chars().count());
    let masked_cursor = password[..cursor.min(password.len())].chars().count();
    render_text_input_line(
        frame,
        rows[4],
        TextInputRenderSpec {
            value: &masked,
            cursor: masked_cursor,
            placeholder: "password",
            prefix: "› ",
            focused: true,
            text_style: Style::default().fg(text()).bg(modal_bg()),
            placeholder_style: Style::default().fg(ghost()).bg(modal_bg()),
            prefix_style: Style::default().fg(signal()).bg(modal_bg()),
            cursor_style: Style::default().fg(bg()).bg(signal()),
            background_style: Style::default().bg(modal_bg()),
        },
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("enter", Style::default().fg(red()).bg(modal_bg())),
            Span::styled(" submit  ·  ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled("esc", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(" cancel", Style::default().fg(muted()).bg(modal_bg())),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[5],
    );
}

pub(crate) fn render_api_key_entry(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
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

    let masked_key = mask_key_segment(&app.api_key_input);
    let masked_cursor = masked_cursor_for_key(&app.api_key_input, &masked_key, app.api_key_cursor);
    render_text_input_line(
        frame,
        rows[4],
        TextInputRenderSpec {
            value: &masked_key,
            cursor: masked_cursor,
            placeholder: "sk-...",
            prefix: "> ",
            focused: true,
            text_style: Style::default().fg(text()).bg(modal_bg()),
            placeholder_style: Style::default().fg(ghost()).bg(modal_bg()),
            prefix_style: Style::default().fg(signal()).bg(modal_bg()),
            cursor_style: Style::default().fg(bg()).bg(signal()),
            background_style: Style::default().bg(modal_bg()),
        },
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

pub(crate) fn render_oauth(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    frame.render_widget(modal_block("OAuth Login"), area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let Some(state) = &app.oauth_state else {
        return;
    };

    // Device-code (Grok Build): short user_code like WWG6-9PSY, no paste_slot.
    // Browser PKCE: empty user_code + paste_slot for long authorize codes.
    let is_device = !state.user_code.trim().is_empty() && state.paste_slot.is_none();
    let show_code = !state.user_code.trim().is_empty();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(if show_code { 1 } else { 0 }),
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
            if is_device {
                Span::styled(
                    "  ·  Grok Build device",
                    Style::default().fg(muted()).bg(modal_bg()),
                )
            } else {
                Span::raw("")
            },
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[0],
    );
    let instruction = if is_device {
        "Confirm this code in your browser (accounts.x.ai). Do not paste long codes."
    } else if state.paste_slot.is_some() {
        "Browser PKCE: if the page shows a long code, copy it and press p / Ctrl+V here."
    } else {
        "Complete login in your browser."
    };
    frame.render_widget(
        Paragraph::new(instruction).style(Style::default().fg(text()).bg(modal_bg())),
        rows[1],
    );

    if show_code {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    if is_device {
                        "Confirm code: "
                    } else {
                        "Code: "
                    },
                    Style::default().fg(muted()),
                ),
                Span::styled(
                    state.user_code.clone(),
                    Style::default()
                        .fg(accent())
                        .bg(modal_bg())
                        .add_modifier(Modifier::BOLD),
                ),
            ]))
            .style(Style::default().bg(modal_bg())),
            rows[2],
        );
    }

    let link_style = Style::default()
        .fg(signal())
        .bg(modal_bg())
        .add_modifier(Modifier::UNDERLINED);
    let link_lines = wrap_plain(&state.verification_uri, rows[4].width as usize);
    for (offset, line) in link_lines.iter().take(rows[4].height as usize).enumerate() {
        let row = line_rect(rows[4], offset);
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

    let help = if let Some(status) = state.paste_status.as_deref() {
        format!("{status}     c copy link     ctrl+o open     p paste code     esc close")
    } else if is_device {
        "c copy link     ctrl+o reopen browser     esc close · waiting for confirmation…"
            .to_string()
    } else if state.paste_slot.is_some() {
        "c copy link     ctrl+o open browser     p/ctrl+v paste code     esc close".to_string()
    } else {
        "c copy link     ctrl+o open browser     esc close".to_string()
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(muted()).bg(modal_bg())),
        rows[5],
    );
}

pub(crate) fn render_message_queue(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    frame.render_widget(modal_block("Message Queue"), area);

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

    let header = if app.queued_user_messages.is_empty() {
        "No queued messages".to_string()
    } else {
        format!("{} queued", app.queued_user_messages.len())
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            header,
            Style::default().fg(text()).bg(modal_bg()),
        )))
        .style(Style::default().bg(modal_bg())),
        rows[0],
    );

    if app.queued_user_messages.is_empty() {
        frame.render_widget(
            Paragraph::new("Messages sent while the agent is working will wait here.")
                .style(Style::default().fg(muted()).bg(modal_bg())),
            rows[1],
        );
    } else {
        let visible_rows = rows[1].height as usize;
        let items = app
            .queued_user_messages
            .iter()
            .enumerate()
            .map(|(index, message)| {
                let selected = index == app.queued_message_selected;
                let style = if selected {
                    active_item_style()
                } else {
                    inactive_item_style()
                };
                let line = queued_message_line(index, message, rows[1].width as usize, style);
                ListItem::new(line).style(style)
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default()
            .with_offset(app.queued_message_scroll)
            .with_selected(Some(app.queued_message_selected));
        frame.render_stateful_widget(
            List::new(items)
                .style(Style::default().bg(modal_bg()))
                .highlight_style(modal_list_highlight_style()),
            rows[1],
            &mut state,
        );
        render_scrollbar(
            frame,
            app,
            rows[1],
            app.queued_user_messages.len(),
            app.queued_message_scroll,
            crate::ui::interaction::ScrollTarget::MessageQueue,
        );
        let start = app
            .queued_message_scroll
            .min(app.queued_user_messages.len());
        for index in start..(start + visible_rows).min(app.queued_user_messages.len()) {
            let row = line_rect(rows[1], index.saturating_sub(start));
            let main = Rect {
                x: row.x,
                y: row.y,
                width: row.width.saturating_sub(4).max(1),
                height: row.height,
            };
            app.register_hit(
                main,
                20,
                format!("queued message {}", index + 1),
                HitAction::QueuedMessage(index),
            );
            if row.width > 4 {
                let remove = Rect {
                    x: row.x + row.width.saturating_sub(3),
                    y: row.y,
                    width: 3.min(row.width),
                    height: row.height,
                };
                app.register_hit(
                    remove,
                    25,
                    format!("remove queued message {}", index + 1),
                    HitAction::RemoveQueuedMessage(index),
                );
            }
        }
    }

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("enter", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(" edit  ·  ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled("d/del", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(" remove  ·  ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled("D", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(
                " clear all  ·  ",
                Style::default().fg(muted()).bg(modal_bg()),
            ),
            Span::styled("ctrl+↑/↓", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(" move  ·  ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled("esc", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(" close", Style::default().fg(muted()).bg(modal_bg())),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[2],
    );
}

fn queued_message_line(
    index: usize,
    message: &crate::state::QueuedUserMessage,
    width: usize,
    style: Style,
) -> Line<'static> {
    let prefix = format!("{:>2}  ", index + 1);
    let remove = " × ".to_string();
    let badges = if message.images.is_empty() {
        String::new()
    } else {
        format!("  image{}", if message.images.len() > 1 { "s" } else { "" })
    };
    let text_width = width
        .saturating_sub(prefix.len())
        .saturating_sub(badges.len())
        .saturating_sub(remove.len())
        .max(1);
    let summary = queued_message_summary(&message.text, text_width);
    Line::from(vec![
        Span::styled(prefix, style),
        Span::styled(summary, style),
        Span::styled(badges, Style::default().fg(code_const()).bg(modal_bg())),
        Span::styled(remove, Style::default().fg(muted()).bg(modal_bg())),
    ])
}

fn queued_message_summary(text_value: &str, width: usize) -> String {
    let compact = text_value.split_whitespace().collect::<Vec<_>>().join(" ");
    fit_inline(
        if compact.is_empty() {
            "(empty message)"
        } else {
            &compact
        },
        width,
    )
}

fn fit_inline(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_string();
    }
    let mut result = value.chars().take(width - 1).collect::<String>();
    result.push('…');
    result
}

pub(crate) fn render_queued_message_edit(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    frame.render_widget(modal_block("Edit Queued Message"), area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(inner);

    let index_label = app
        .queued_edit_index
        .map(|index| format!("Message {}", index + 1))
        .unwrap_or_else(|| "Message".to_string());
    frame.render_widget(
        Paragraph::new(index_label).style(Style::default().fg(muted()).bg(modal_bg())),
        rows[0],
    );

    render_text_area(
        frame,
        rows[1],
        &app.queued_edit_text,
        app.queued_edit_cursor,
        app.mode == crate::state::Mode::QueuedMessageEdit,
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("ctrl+enter", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(" save  ·  ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled("enter", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(" newline  ·  ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled("esc", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(" discard", Style::default().fg(muted()).bg(modal_bg())),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[2],
    );
}

fn render_text_area(frame: &mut Frame<'_>, area: Rect, value: &str, cursor: usize, focused: bool) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent()))
        .style(Style::default().fg(text()).bg(modal_bg()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let ranges = crate::input::input_visual_line_ranges(value, inner.width as usize);
    let cursor = floor_char_boundary(value, cursor.min(value.len()));
    let cursor_line = ranges
        .iter()
        .position(|(start, end)| cursor >= *start && cursor <= *end)
        .unwrap_or(0);
    let visible_start = cursor_line
        .saturating_add(1)
        .saturating_sub(inner.height as usize);
    let mut lines = Vec::new();
    for (line_index, (start, end)) in ranges
        .iter()
        .copied()
        .enumerate()
        .skip(visible_start)
        .take(inner.height as usize)
    {
        let line_value = value.get(start..end).unwrap_or_default();
        if focused && line_index == cursor_line {
            let cursor_in_line = cursor.saturating_sub(start).min(line_value.len());
            let (before, rest) = line_value.split_at(cursor_in_line);
            let mut spans = vec![Span::styled(
                before.to_string(),
                Style::default().fg(text()).bg(modal_bg()),
            )];
            if rest.is_empty() {
                spans.push(Span::styled(" ", Style::default().fg(bg()).bg(signal())));
            } else {
                let next =
                    next_char_boundary(line_value, cursor_in_line).unwrap_or(line_value.len());
                let (cursor_text, after) = rest.split_at(next - cursor_in_line);
                spans.push(Span::styled(
                    cursor_text.to_string(),
                    Style::default().fg(bg()).bg(signal()),
                ));
                spans.push(Span::styled(
                    after.to_string(),
                    Style::default().fg(text()).bg(modal_bg()),
                ));
            }
            lines.push(Line::from(spans));
        } else {
            lines.push(Line::from(Span::styled(
                line_value.to_string(),
                Style::default().fg(text()).bg(modal_bg()),
            )));
        }
    }
    while lines.len() < inner.height as usize {
        lines.push(Line::from(""));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(modal_bg())),
        inner,
    );

    if focused {
        let (line_start, line_end) = ranges.get(cursor_line).copied().unwrap_or((0, 0));
        let cursor_column = value
            .get(line_start..cursor.min(line_end))
            .map(|slice| slice.chars().count())
            .unwrap_or(0);
        frame.set_cursor_position(ratatui::layout::Position::new(
            inner
                .x
                .saturating_add(cursor_column.min(inner.width.saturating_sub(1) as usize) as u16),
            inner.y.saturating_add(
                cursor_line
                    .saturating_sub(visible_start)
                    .min(inner.height.saturating_sub(1) as usize) as u16,
            ),
        ));
    }
}

pub(crate) fn render_confirm_mcp_merge(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    frame.render_widget(modal_block("Merge MCP config?"), area);
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let path = app
        .pending_mcp_merge
        .as_ref()
        .map(|p| p.join("mcp.json").display().to_string())
        .unwrap_or_else(|| "mcp.json".into());
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(inner);
    frame.render_widget(
        Paragraph::new(format!(
            "Plugin install finished. Merge this server definition into your global NAVI config?\n\n{path}"
        ))
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(text()).bg(modal_bg())),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("y / enter", Style::default().fg(signal()).bg(modal_bg())),
            Span::styled(" merge  ·  ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled("n / esc", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(" skip", Style::default().fg(muted()).bg(modal_bg())),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[1],
    );
}

pub(crate) fn render_confirm_cancel_turn(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    frame.render_widget(modal_block("Cancel Turn"), area);
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let queued = app.queued_user_messages.len();
    let message = if queued == 0 {
        "Cancel the active turn?".to_string()
    } else {
        format!("Cancel the active turn and clear {queued} queued message(s)?")
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(2), Constraint::Length(1)])
        .split(inner);
    frame.render_widget(
        Paragraph::new(message)
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(text()).bg(modal_bg())),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("enter", Style::default().fg(red()).bg(modal_bg())),
            Span::styled(" cancel  ·  ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled("esc", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(" keep working", Style::default().fg(muted()).bg(modal_bg())),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[1],
    );
}

pub(crate) fn render_confirm_plan(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let title = app
        .plan_review
        .as_ref()
        .map(|r| format!("Plan · {}", r.title))
        .or_else(|| {
            app.proposed_plan
                .as_ref()
                .map(|p| format!("Plan · {}", p.title))
        })
        .unwrap_or_else(|| "Proposed Plan".to_string());
    frame.render_widget(modal_block(&title), area);
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    // review when we have full state.
    if let Some(review) = &app.plan_review {
        use crate::plan_review::PlanReviewFocus;
        let footer_h: u16 = match review.focus {
            PlanReviewFocus::CommentInput | PlanReviewFocus::Prompt => 2,
            PlanReviewFocus::Preview => 1,
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(2), Constraint::Length(footer_h)])
            .split(inner);

        let visible = rows[0].height as usize;
        let (sel_lo, sel_hi) = review.selected_range();
        let mut body: Vec<Line> = Vec::new();
        let end = (review.scroll + visible).min(review.lines.len());
        for (idx, line) in review.lines[review.scroll..end].iter().enumerate() {
            let global = review.scroll + idx;
            let in_sel = global >= sel_lo && global <= sel_hi;
            let has_comment = review.comment_on_line(global).is_some();
            let marker = if has_comment { "💬 " } else { "  " };
            let style = if in_sel {
                Style::default()
                    .fg(selection_fg())
                    .bg(selection_bg())
                    .add_modifier(Modifier::BOLD)
            } else if global == review.cursor_line {
                Style::default()
                    .fg(signal())
                    .bg(modal_bg())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(text()).bg(modal_bg())
            };
            body.push(Line::from(Span::styled(format!("{marker}{line}"), style)));

            // Register mouse hit for this line.
            if app.mode == crate::state::Mode::ConfirmPlan {
                let line_area = Rect::new(
                    rows[0].x,
                    rows[0].y.saturating_add(idx as u16),
                    rows[0].width,
                    1,
                );
                app.register_hit(
                    line_area,
                    20,
                    "plan line",
                    crate::ui::interaction::HitAction::PlanReviewLine(global),
                );
            }
        }
        frame.render_widget(
            Paragraph::new(Text::from(body)).style(Style::default().bg(modal_bg())),
            rows[0],
        );

        let approve_label = if review.comments.is_empty() {
            "a approve"
        } else {
            "a approve w/ comments"
        };
        let mut footer_spans = vec![
            Span::styled(
                approve_label,
                Style::default()
                    .fg(red())
                    .bg(modal_bg())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(ghost()).bg(modal_bg())),
            Span::styled("s changes", Style::default().fg(text()).bg(modal_bg())),
            Span::styled("  ·  ", Style::default().fg(ghost()).bg(modal_bg())),
            Span::styled("c comment", Style::default().fg(text()).bg(modal_bg())),
            Span::styled("  ·  ", Style::default().fg(ghost()).bg(modal_bg())),
            Span::styled("q quit", Style::default().fg(text()).bg(modal_bg())),
            Span::styled("  ·  ", Style::default().fg(ghost()).bg(modal_bg())),
            Span::styled("tab focus", Style::default().fg(muted()).bg(modal_bg())),
        ];
        if !review.comments.is_empty() {
            footer_spans.push(Span::styled(
                format!("  ·  {} comments", review.comments.len()),
                Style::default().fg(code_const()).bg(modal_bg()),
            ));
        }

        match review.focus {
            PlanReviewFocus::Preview => {
                frame.render_widget(
                    Paragraph::new(Line::from(footer_spans)).style(Style::default().bg(modal_bg())),
                    rows[1],
                );
            }
            PlanReviewFocus::CommentInput => {
                let input = format!("› comment: {}_", review.comment_draft);
                frame.render_widget(
                    Paragraph::new(Text::from(vec![
                        Line::from(footer_spans),
                        Line::from(Span::styled(
                            input,
                            Style::default().fg(signal()).bg(modal_bg()),
                        )),
                    ]))
                    .style(Style::default().bg(modal_bg())),
                    rows[1],
                );
            }
            PlanReviewFocus::Prompt => {
                let input = format!("› changes: {}_", review.prompt_draft);
                frame.render_widget(
                    Paragraph::new(Text::from(vec![
                        Line::from(footer_spans),
                        Line::from(Span::styled(
                            input,
                            Style::default().fg(signal()).bg(modal_bg()),
                        )),
                    ]))
                    .style(Style::default().bg(modal_bg())),
                    rows[1],
                );
            }
        }

        // Action hits for mouse.
        if app.mode == crate::state::Mode::ConfirmPlan {
            let w = rows[1].width.max(1);
            let seg = (w / 4).max(1);
            for (i, action) in [
                crate::ui::interaction::HitAction::PlanReviewApprove,
                crate::ui::interaction::HitAction::PlanReviewChanges,
                crate::ui::interaction::HitAction::PlanReviewComment,
                crate::ui::interaction::HitAction::PlanReviewQuit,
            ]
            .into_iter()
            .enumerate()
            {
                let x = rows[1].x.saturating_add((i as u16).saturating_mul(seg));
                app.register_hit(Rect::new(x, rows[1].y, seg, 1), 25, "plan action", action);
            }
        }
        return;
    }

    // Legacy thin proposed_plan fallback.
    let Some(plan) = &app.proposed_plan else {
        return;
    };
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        plan.title.clone(),
        Style::default()
            .fg(text())
            .bg(modal_bg())
            .add_modifier(Modifier::BOLD),
    )));
    for step in &plan.steps {
        lines.push(Line::from(Span::styled(
            format!("• {step}"),
            Style::default().fg(muted()).bg(modal_bg()),
        )));
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(2), Constraint::Length(1)])
        .split(inner);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(text()).bg(modal_bg())),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("a", Style::default().fg(red()).bg(modal_bg())),
            Span::styled(" approve  ·  ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled("q", Style::default().fg(text()).bg(modal_bg())),
            Span::styled(" quit", Style::default().fg(muted()).bg(modal_bg())),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[1],
    );
}

pub(crate) fn render_tool_approval(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
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
                "shift+tab",
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" permission mode", Style::default().fg(muted())),
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

pub(crate) fn render_path_mentions(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let title = if app.path_filter.is_empty() {
        "Mention path  @".to_string()
    } else {
        format!("Mention path  @{}", app.path_filter)
    };
    frame.render_widget(modal_block(&title), area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(1)])
        .split(inner);

    let candidates = crate::path_mentions::filtered_path_candidates(app);
    let visible = rows[0].height as usize;
    let scroll = app.path_scroll.min(candidates.len().saturating_sub(1));
    let slice = candidates
        .iter()
        .skip(scroll)
        .take(visible)
        .enumerate()
        .collect::<Vec<_>>();

    let items = if candidates.is_empty() {
        vec![ListItem::new(Span::styled(
            "  (no matches)",
            Style::default().fg(muted()),
        ))]
    } else {
        slice
            .iter()
            .map(|(i, c)| {
                let index = scroll + i;
                let selected = index == app.selected_path;
                let hovered = app.hover_index == Some(index);
                let style = if hovered || selected {
                    active_item_style()
                } else {
                    inactive_item_style()
                };
                let marker = if selected { "● " } else { "  " };
                let kind = if c.is_dir { "dir  " } else { "file " };
                ListItem::new(Span::styled(format!("{marker}{kind}{}", c.rel), style)).style(style)
            })
            .collect()
    };

    frame.render_widget(
        List::new(items).style(Style::default().bg(modal_bg())),
        rows[0],
    );
    for (i, _) in slice {
        let index = scroll + i;
        app.register_hit(
            line_rect(rows[0], i),
            20,
            format!("path {index}"),
            HitAction::Key {
                code: crossterm::event::KeyCode::Enter,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
        );
    }
    frame.render_widget(
        Paragraph::new("↑↓ select  enter insert  esc cancel  ·  files hydrate on send")
            .style(Style::default().fg(muted()).bg(modal_bg())),
        rows[1],
    );
}

pub(crate) fn render_thinking_picker(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let model_label = app
        .models
        .get(app.selected_model)
        .map(|m| m.name.as_str())
        .unwrap_or("model");
    let title = format!("Effort Level · {model_label}");
    let block = modal_block(&title);
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(1)])
        .split(inner);

    let options = crate::keybindings::modals::thinking_options_for_app(app);
    let binary = crate::keybindings::modals::effort_is_binary_for_app(app);
    // Cursor follows `selected_thinking` only. Do NOT fall back to
    // `thinking_level` in the same `.position` — that kept the highlight on
    // the current level forever while arrow keys updated selected_thinking.
    let selected_local = options
        .iter()
        .position(|l| l.index() == app.selected_thinking)
        .or_else(|| options.iter().position(|l| *l == app.thinking_level))
        .unwrap_or(0);

    let items = options
        .iter()
        .enumerate()
        .map(|(index, level)| {
            let selected = index == selected_local;
            let hovered = app.hover_index == Some(index);
            let current = if binary {
                // Binary mode collapses any non-off selection onto "thinking on".
                (level.is_off() && app.thinking_level.is_off())
                    || (!level.is_off() && !app.thinking_level.is_off())
            } else {
                *level == app.thinking_level
            };
            let style = if hovered || selected {
                active_item_style()
            } else {
                inactive_item_style()
            };

            let marker = if current { "● " } else { "  " };
            ListItem::new(Span::styled(
                format!("{}{}", marker, level.display_label(binary)),
                style,
            ))
            .style(style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        List::new(items).style(Style::default().bg(modal_bg())),
        rows[0],
    );
    for (index, level) in options.iter().enumerate().take(rows[0].height as usize) {
        // Selecting a row should set selected_thinking to that level's global index.
        let level = *level;
        app.register_hit(
            line_rect(rows[0], index),
            20,
            format!("effort {}", level.display_label(binary)),
            HitAction::Key {
                code: crossterm::event::KeyCode::Enter,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
        );
        // Ensure Enter uses the level under the cursor: sync selected_thinking on hover path
        // is handled by key/mouse handlers; click path uses current selected_thinking.
        let _ = level;
    }
    let footer = if binary {
        "model has no effort levels · thinking off or on"
    } else {
        "effort levels for this model"
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(muted()).bg(modal_bg())),
        rows[1],
    );
}

pub(crate) fn render_question(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
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

pub(crate) fn render_settings(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    use crate::settings::{
        SETTINGS_ROWS, SettingAction, SettingRow, SettingValueKind, format_setting_line,
        setting_display,
    };
    use ratatui::style::Modifier;

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

    let selected = app
        .selected_setting
        .min(SETTINGS_ROWS.len().saturating_sub(1));
    // Leave room for values; keep labels aligned across rows.
    let label_col = ((inner.width as usize).saturating_sub(4) / 2).clamp(12, 18);

    let items = SETTINGS_ROWS
        .iter()
        .enumerate()
        .map(|(index, row)| match row {
            SettingRow::Section(title) => {
                // Quiet section header — no decorative dashes.
                let style = Style::default()
                    .fg(muted())
                    .bg(modal_bg())
                    .add_modifier(Modifier::BOLD);
                ListItem::new(Span::styled(format!(" {title}"), style)).style(style)
            }
            SettingRow::Action(action) => {
                let (label, val, kind) = setting_display(app, *action);
                let is_selected = index == selected;
                let hovered = app.hover_index == Some(index);
                let base = if hovered || is_selected {
                    active_item_style()
                } else {
                    inactive_item_style()
                };
                let line = format_setting_line(label, &val, kind, label_col);
                // Dim the trailing › so the label/value stay primary.
                if kind == SettingValueKind::Link && line.contains('›') {
                    let (main, _) = line.rsplit_once('›').unwrap_or((line.as_str(), ""));
                    ListItem::new(Line::from(vec![
                        Span::styled(main.to_string(), base),
                        Span::styled(
                            "›",
                            Style::default()
                                .fg(ghost())
                                .bg(base.bg.unwrap_or(modal_bg())),
                        ),
                    ]))
                    .style(base)
                } else if kind == SettingValueKind::Toggle {
                    // On = accent dot, off = muted ring.
                    let on = val == "on";
                    let mark = if on { "●" } else { "○" };
                    let mark_style = if on {
                        Style::default()
                            .fg(signal())
                            .bg(base.bg.unwrap_or(modal_bg()))
                    } else {
                        Style::default()
                            .fg(ghost())
                            .bg(base.bg.unwrap_or(modal_bg()))
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{mark}  "), mark_style),
                        Span::styled(label.to_string(), base),
                    ]))
                    .style(base)
                } else {
                    ListItem::new(Span::styled(line, base)).style(base)
                }
            }
        })
        .collect::<Vec<_>>();

    let visible = rows[0].height as usize;
    let total = SETTINGS_ROWS.len();
    let selected = selected.min(total.saturating_sub(1));
    // Keep selection in view (simple sticky scroll).
    let mut offset = 0usize;
    if selected >= visible {
        offset = selected + 1 - visible;
    }
    let mut list_state = ListState::default()
        .with_offset(offset)
        .with_selected(Some(selected));
    frame.render_stateful_widget(
        List::new(items)
            .style(Style::default().bg(modal_bg()))
            .highlight_style(active_item_style()),
        rows[0],
        &mut list_state,
    );
    for (row_offset, index) in (offset..total).take(visible).enumerate() {
        let row = &SETTINGS_ROWS[index];
        let hit = match row {
            SettingRow::Section(_) => continue,
            SettingRow::Action(SettingAction::Theme) => HitAction::ThemePicker,
            SettingRow::Action(_) => HitAction::Setting(index),
        };
        let label = match row {
            SettingRow::Section(t) => format!("section {t}"),
            SettingRow::Action(a) => format!("setting {:?}", a),
        };
        app.register_hit(line_rect(rows[0], row_offset), 20, label, hit);
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("[enter]", Style::default().fg(signal())),
            Span::styled("  ", Style::default().fg(muted())),
            Span::styled("[esc]", Style::default().fg(signal())),
            Span::styled(" close", Style::default().fg(muted())),
        ]))
        .style(Style::default().bg(modal_bg())),
        rows[1],
    );
    app.register_hit(
        line_rect(rows[1], 0),
        20,
        "close settings",
        HitAction::CloseModal,
    );
}

pub(crate) fn render_message_actions(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
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
        rows[2],
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

fn masked_cursor_for_key(original: &str, masked: &str, cursor: usize) -> usize {
    let cursor = floor_char_boundary(original, cursor.min(original.len()));
    let char_position = original[..cursor].chars().count();
    masked
        .char_indices()
        .nth(char_position)
        .map(|(byte, _)| byte)
        .unwrap_or(masked.len())
}

pub(crate) fn render_plugin_approval(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
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

pub(crate) fn render_theme_picker(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
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

    render_text_input_line(
        frame,
        rows[0],
        TextInputRenderSpec {
            value: &app.theme_filter,
            cursor: app.theme_filter_cursor,
            placeholder: "search",
            prefix: "> ",
            focused: true,
            text_style: Style::default().fg(text()).bg(modal_bg()),
            placeholder_style: Style::default().fg(muted()).bg(modal_bg()),
            prefix_style: Style::default().fg(signal()).bg(modal_bg()),
            cursor_style: Style::default().fg(bg()).bg(signal()),
            background_style: Style::default().bg(modal_bg()),
        },
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

pub(crate) fn render_background_models(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let block = modal_block("Agent Model Routes");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    let tasks: &[(&str, &str)] = &[
        ("memory_extraction", "Automatic durable-memory extraction"),
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
        let row_bg = if selected { selection_bg() } else { modal_bg() };

        let task_style = if selected {
            Style::default()
                .fg(selection_fg())
                .bg(row_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(text()).bg(row_bg)
        };
        let model_style = if selected {
            Style::default()
                .fg(selection_fg())
                .bg(row_bg)
                .add_modifier(Modifier::BOLD)
        } else if has_override {
            Style::default().fg(accent()).bg(row_bg)
        } else {
            Style::default().fg(muted()).bg(row_bg)
        };
        let desc_style = Style::default()
            .fg(if selected { selection_fg() } else { ghost() })
            .bg(row_bg);
        let marker = if selected { "› " } else { "  " };

        rows.push(Line::from(vec![
            Span::styled(marker, task_style),
            Span::styled(format!("{:>18}", task_id), task_style),
            Span::styled("  →  ", desc_style),
            Span::styled(resolved_label, model_style),
        ]));
        rows.push(Line::from(vec![
            Span::styled("  ", desc_style),
            Span::styled(format!("  {:>18}  ", description), desc_style),
        ]));
    }

    let list = Paragraph::new(rows).style(Style::default().bg(modal_bg()));
    frame.render_widget(list, inner);

    // Footer hints.
    let footer_y = inner.y + inner.height.saturating_sub(1);
    if footer_y > inner.y {
        let footer_rect = Rect::new(inner.x, footer_y, inner.width, 1);
        let hints = Line::from(vec![
            Span::styled("  [↑/↓]", Style::default().fg(signal())),
            Span::styled(" select  ", Style::default().fg(muted())),
            Span::styled("[enter]", Style::default().fg(signal())),
            Span::styled(" pick  ", Style::default().fg(muted())),
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
    if task == "memory_extraction" && bg.memory_extraction.is_none() {
        return "not configured (automatic extraction off)".to_string();
    }
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
        "memory_extraction" => bg.memory_extraction.is_some(),
        "compaction" => bg.compaction.is_some(),
        "repo_search" => bg.repo_search.is_some(),
        "subagent_research" => bg.subagent_research.is_some(),
        "simple_code_edit" => bg.simple_code_edit.is_some(),
        _ => bg.default.is_some(),
    }
}

pub(crate) fn render_model_routing(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let block = modal_block("Model Routing");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tabs
            Constraint::Length(1), // blank
            Constraint::Min(6),    // content
            Constraint::Length(1), // footer
        ])
        .split(inner);

    // Tab bar
    let mut tab_spans: Vec<Span> = Vec::new();
    for (i, tab) in crate::state::ModelRoutingTab::ALL.iter().enumerate() {
        if i > 0 {
            tab_spans.push(Span::styled(
                "  ",
                Style::default().fg(muted()).bg(modal_bg()),
            ));
        }
        let active = *tab == app.model_routing_tab;
        let style = if active {
            Style::default()
                .fg(signal())
                .bg(modal_bg())
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(muted()).bg(modal_bg())
        };
        let label = if active {
            format!("[{}]", tab.label())
        } else {
            format!(" {} ", tab.label())
        };
        tab_spans.push(Span::styled(label, style));
    }
    frame.render_widget(
        Paragraph::new(Line::from(tab_spans)).style(Style::default().bg(modal_bg())),
        chunks[0],
    );

    match app.model_routing_tab {
        crate::state::ModelRoutingTab::Chat => {
            render_model_routing_chat_body(frame, app, chunks[2]);
        }
        crate::state::ModelRoutingTab::Agents => {
            render_model_routing_agents_body(frame, app, chunks[2]);
        }
        crate::state::ModelRoutingTab::Attachments => {
            render_model_routing_attachments_body(frame, app, chunks[2]);
        }
    }

    let footer = match app.model_routing_tab {
        crate::state::ModelRoutingTab::Chat => {
            "[←/→] tabs  [enter] open chat model picker  [esc] close"
        }
        crate::state::ModelRoutingTab::Agents | crate::state::ModelRoutingTab::Attachments => {
            "[←/→] tabs  [↑/↓] select  [enter] pick  [d] reset  [esc] close"
        }
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            footer,
            Style::default().fg(muted()).bg(modal_bg()),
        ))
        .style(Style::default().bg(modal_bg())),
        chunks[3],
    );
}

fn render_model_routing_chat_body(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let model = &app.loaded_config.config.model;
    let effort = app.thinking_level.label();
    let lines = vec![
        Line::from(Span::styled(
            "Session chat model",
            Style::default().fg(ghost()).bg(modal_bg()),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Provider  ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled(
                model.provider.clone(),
                Style::default()
                    .fg(text())
                    .bg(modal_bg())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Model     ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled(
                model.name.clone(),
                Style::default()
                    .fg(accent())
                    .bg(modal_bg())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Effort    ", Style::default().fg(muted()).bg(modal_bg())),
            Span::styled(effort, Style::default().fg(text()).bg(modal_bg())),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Press enter to open the full model picker (ctrl+m).",
            Style::default().fg(ghost()).bg(modal_bg()),
        )),
        Line::from(Span::styled(
            "Agents and Attachments tabs set specialized fallbacks.",
            Style::default().fg(ghost()).bg(modal_bg()),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(modal_bg())),
        area,
    );
}

fn render_model_routing_agents_body(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let tasks: &[(&str, &str)] = &[
        ("memory_extraction", "Automatic durable-memory extraction"),
        ("compaction", "Conversation summarization"),
        ("repo_search", "Repository exploration"),
        ("subagent_research", "Research subagents"),
        ("simple_code_edit", "Code edit subagents"),
    ];
    let mut rows: Vec<Line> = Vec::new();
    let bg = &app.loaded_config.config.background_models;
    // Each task is 2 visual lines; honor bg_models_scroll so ↓/↑ stay visible.
    let start = app.bg_models_scroll.min(tasks.len().saturating_sub(1));
    for (i, (task_id, description)) in tasks.iter().enumerate().skip(start) {
        let selected = i == app.bg_models_selected;
        let resolved_label = resolve_bg_model_label(app, task_id);
        let has_override = bg_model_has_override(bg, task_id);
        let row_bg = if selected { selection_bg() } else { modal_bg() };
        let task_style = if selected {
            Style::default()
                .fg(selection_fg())
                .bg(row_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(text()).bg(row_bg)
        };
        let model_style = if selected {
            Style::default()
                .fg(selection_fg())
                .bg(row_bg)
                .add_modifier(Modifier::BOLD)
        } else if has_override {
            Style::default().fg(accent()).bg(row_bg)
        } else {
            Style::default().fg(muted()).bg(row_bg)
        };
        let desc_style = Style::default()
            .fg(if selected { selection_fg() } else { ghost() })
            .bg(row_bg);
        let marker = if selected { "› " } else { "  " };
        rows.push(Line::from(vec![
            Span::styled(marker, task_style),
            Span::styled(format!("{:>18}", task_id), task_style),
            Span::styled("  →  ", desc_style),
            Span::styled(resolved_label, model_style),
        ]));
        rows.push(Line::from(vec![
            Span::styled("  ", desc_style),
            Span::styled(format!("  {:>18}  ", description), desc_style),
        ]));
    }
    frame.render_widget(
        Paragraph::new(rows).style(Style::default().bg(modal_bg())),
        area,
    );
}

fn render_model_routing_attachments_body(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let modalities: &[(&str, &str)] = &[
        ("image", "Image analysis fallback"),
        ("audio", "Audio analysis fallback"),
        ("video", "Video analysis fallback"),
        ("document", "Document analysis fallback"),
    ];
    let mut rows: Vec<Line> = Vec::new();
    let config = &app.loaded_config.config.attachment_models;
    for (i, (modality, description)) in modalities.iter().enumerate() {
        let selected = i == app.selected_attachment_model;
        let resolved_label =
            crate::keybindings::modals::resolve_attachment_model_label(app, modality);
        let has_override =
            crate::keybindings::modals::attachment_model_has_override(config, modality);
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
            Span::styled(format!("{:>20}", modality), task_style),
            Span::styled("  →  ", desc_style),
            Span::styled(resolved_label, model_style),
        ]));
        rows.push(Line::from(vec![Span::styled(
            format!("  {:>20}  ", description),
            desc_style,
        )]));
    }
    frame.render_widget(
        Paragraph::new(rows).style(Style::default().bg(modal_bg())),
        area,
    );
}

pub(crate) fn render_extensions_hub(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let block = modal_block("Extensions");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(6), Constraint::Length(1)])
        .split(inner);

    let items: Vec<ListItem> = crate::state::ExtensionsHubItem::ALL
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let selected = index == app.selected_extensions_item;
            let hovered = app.hover_index == Some(index);
            let style = if hovered || selected {
                active_item_style()
            } else {
                inactive_item_style()
            };
            let line = format!("{}  —  {}", item.label(), item.description());
            ListItem::new(Span::styled(line, style)).style(style)
        })
        .collect();

    frame.render_widget(
        List::new(items).style(Style::default().bg(modal_bg())),
        chunks[0],
    );
    for (index, item) in crate::state::ExtensionsHubItem::ALL.iter().enumerate() {
        app.register_hit(
            line_rect(chunks[0], index),
            20,
            format!("extension {}", item.label()),
            HitAction::ExtensionsItem(index),
        );
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("[enter]", Style::default().fg(signal())),
            Span::styled(" open  ", Style::default().fg(muted())),
            Span::styled("[esc]", Style::default().fg(signal())),
            Span::styled(" close", Style::default().fg(muted())),
        ]))
        .style(Style::default().bg(modal_bg())),
        chunks[1],
    );
}

pub(crate) fn render_usage(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
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

    // Always show local session / context usage (works for every provider).
    render_session_usage(&mut lines, app);
    lines.push(Line::from(""));

    if app.usage_state.loading {
        lines.push(Line::from(Span::styled(
            if app.usage_state.report.is_some() || app.usage_state.remaining_credits.is_some() {
                "Refreshing account usage…"
            } else {
                "Loading account usage…"
            },
            Style::default().fg(signal()),
        )));
        // Keep last-known account report visible while refreshing so Hyper
        // balance never disappears between turns.
        if let Some(ref report) = app.usage_state.report {
            lines.push(Line::from(""));
            render_usage_report(&mut lines, report);
        }
    } else if let Some(ref report) = app.usage_state.report {
        render_usage_report(&mut lines, report);
        if let Some(ref error) = app.usage_state.error {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Last refresh note: {error}"),
                Style::default().fg(muted()),
            )));
        }
    } else if let Some(ref error) = app.usage_state.error {
        lines.push(Line::from(Span::styled(
            format!("Account usage error: {error}"),
            Style::default().fg(red()),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "No account usage data yet — press r to refresh.",
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

fn render_session_usage(lines: &mut Vec<Line<'_>>, app: &TuiApp) {
    lines.push(Line::from(Span::styled(
        "This session",
        Style::default().fg(text()).add_modifier(Modifier::BOLD),
    )));

    let context = app.compact_state.usage_label(0);
    lines.push(Line::from(vec![
        Span::styled("  Context  ", Style::default().fg(muted())),
        Span::styled(context, Style::default().fg(text())),
    ]));

    let fmt = |t: u64| {
        if t >= 1_000_000 {
            format!("{:.1}M", t as f64 / 1_000_000.0)
        } else if t >= 1_000 {
            format!("{}k", t / 1_000)
        } else {
            t.to_string()
        }
    };

    lines.push(Line::from(vec![
        Span::styled("  Totals   ", Style::default().fg(muted())),
        Span::styled(
            format!(
                "{} in · {} out",
                fmt(app.usage_state.session_input_tokens),
                fmt(app.usage_state.session_output_tokens)
            ),
            Style::default().fg(text()),
        ),
    ]));

    if app.is_loading {
        let input = app
            .usage_state
            .estimated_request_input_tokens
            .unwrap_or_else(|| app.compact_state.total_estimated_tokens(0));
        let output = app.usage_state.estimated_request_output_tokens();
        lines.push(Line::from(vec![
            Span::styled("  In progress", Style::default().fg(muted())),
            Span::styled(
                format!(" ≈ {} in · {} out", fmt(input), fmt(output)),
                Style::default().fg(signal()),
            ),
            Span::styled("  estimate", Style::default().fg(ghost())),
        ]));
    }

    if let (Some(inp), Some(out)) = (
        app.usage_state.last_input_tokens,
        app.usage_state.last_output_tokens,
    ) {
        lines.push(Line::from(vec![
            Span::styled("  Last turn", Style::default().fg(muted())),
            Span::styled(
                format!(" {} in · {} out", fmt(inp), fmt(out)),
                Style::default().fg(text()),
            ),
        ]));
    }

    // Session spend from list rates (USD) and prepaid credits when applicable.
    let rates = current_model_pricing(app);
    if app.usage_state.session_cost_known {
        lines.push(Line::from(vec![
            Span::styled("  Est. cost", Style::default().fg(muted())),
            Span::styled(
                format!(" {}", format_usd(app.usage_state.session_cost_usd)),
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
        ]));
        if let (Some(credits), Some(unit)) = (
            app.usage_state.session_credits_spent,
            app.usage_state.session_credit_unit.as_deref(),
        ) {
            let credit_label = if unit.eq_ignore_ascii_case("hypercredits") {
                format!(" ◆ {} Hypercredits", format_credit_count(credits))
            } else {
                format!(" {credits:.2} {unit}")
            };
            lines.push(Line::from(vec![
                Span::styled("  Est. credits", Style::default().fg(muted())),
                Span::styled(credit_label, Style::default().fg(text())),
            ]));
        }
    } else if let Some((in_rate, out_rate)) = rates {
        lines.push(Line::from(vec![
            Span::styled("  Est. cost", Style::default().fg(muted())),
            Span::styled(
                format!(
                    " $0.00  (rates ${:.2}/1M in · ${:.2}/1M out)",
                    in_rate, out_rate
                ),
                Style::default().fg(text()),
            ),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("  Est. cost", Style::default().fg(muted())),
            Span::styled(
                " unknown (no list pricing for this model)".to_string(),
                Style::default().fg(ghost()),
            ),
        ]));
    }

    // Remaining prepaid balance (Charm Hyper), shown even before account report loads.
    if let (Some(remaining), Some(unit)) = (
        app.usage_state.remaining_credits,
        app.usage_state.remaining_credit_unit.as_deref(),
    ) {
        let remaining_label = if unit.eq_ignore_ascii_case("hypercredits") {
            format!(" ◆ {} Hypercredits", format_credit_count(remaining))
        } else {
            format!(" {remaining:.2} {unit}")
        };
        lines.push(Line::from(vec![
            Span::styled("  Remaining ", Style::default().fg(muted())),
            Span::styled(
                remaining_label,
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    if let Some((in_rate, out_rate)) = rates {
        lines.push(Line::from(vec![
            Span::styled("  Rates    ", Style::default().fg(muted())),
            Span::styled(
                format!("${in_rate:.2}/1M in · ${out_rate:.2}/1M out"),
                Style::default().fg(ghost()),
            ),
        ]));
    }

    let provider = &app.loaded_config.config.model.provider;
    let model = &app.loaded_config.config.model.name;
    lines.push(Line::from(vec![
        Span::styled("  Model    ", Style::default().fg(muted())),
        Span::styled(format!("{model} ({provider})"), Style::default().fg(text())),
    ]));
}

fn format_usd(amount: f64) -> String {
    if amount >= 1.0 {
        format!("${amount:.2}")
    } else if amount >= 0.01 {
        format!("${amount:.3}")
    } else if amount > 0.0 {
        format!("${amount:.4}")
    } else {
        "$0.00".to_string()
    }
}

fn format_credit_count(amount: f64) -> String {
    // Prefer whole credits with thousands separators for Hyper-style balances.
    if (amount - amount.round()).abs() < 0.005 {
        navi_sdk::format_hypercredits(amount)
    } else {
        format!("{amount:.2}")
    }
}

fn current_model_pricing(app: &TuiApp) -> Option<(f64, f64)> {
    let provider_id = app.loaded_config.config.model.provider.as_str();
    let model_name = app.loaded_config.config.model.name.as_str();
    navi_sdk::model_list_pricing(&app.loaded_config.config, provider_id, model_name)
}

fn render_usage_report(lines: &mut Vec<Line<'_>>, report: &NaviUsageReport) {
    // Header: provider + plan + source
    let plan_label = report.plan_type.as_deref().unwrap_or("account");
    lines.push(Line::from(vec![
        Span::styled(
            format!("{} ", report.provider_label),
            Style::default().fg(text()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("({plan_label})"), Style::default().fg(muted())),
    ]));
    if !report.source.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  source: {}", report.source),
            Style::default().fg(muted()),
        )));
    }

    if let Some(ref kind) = report.limit_reached_kind {
        lines.push(Line::from(Span::styled(
            format!("⚠ Limit reached: {kind}"),
            Style::default().fg(red()),
        )));
    }
    if let Some(ref notes) = report.notes {
        lines.push(Line::from(Span::styled(
            format!("  {notes}"),
            Style::default().fg(muted()),
        )));
    }

    // Limit bars first — these are the main account metrics (weekly % etc.).
    if !report.limits.is_empty() {
        lines.push(Line::from(""));
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

            if let Some(ref primary) = limit.primary {
                let label = if primary.limit_window_seconds > 0 {
                    "primary"
                } else {
                    "usage"
                };
                render_window_line(lines, label, primary);
            }
            if let Some(ref secondary) = limit.secondary {
                render_window_line(lines, "secondary", secondary);
            }

            if i < report.limits.len() - 1 {
                lines.push(Line::from(""));
            }
        }
    }

    if !report.details.is_empty() {
        lines.push(Line::from(""));
        for detail in &report.details {
            // Keep JSON-ish blobs short in the TUI.
            let value = if detail.value.len() > 80 {
                format!("{}…", &detail.value[..77])
            } else {
                detail.value.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:14} ", detail.label),
                    Style::default().fg(muted()),
                ),
                Span::styled(value, Style::default().fg(text())),
            ]));
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
    let bar = format!(
        "{}{}",
        "█".repeat(filled as usize),
        "░".repeat(empty as usize)
    );

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
        Span::styled(format!(" {remaining}% left"), Style::default().fg(text())),
    ]));
    if !reset_text.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:8} ", ""), Style::default().fg(muted())),
            Span::styled(format!("resets {reset_text}"), Style::default().fg(muted())),
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

pub(crate) fn render_attachment_models(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    clear_modal_area(frame, area);
    let block = modal_block("Attachment Fallbacks");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    let modalities: &[(&str, &str)] = &[
        ("image", "Image analysis fallback"),
        ("audio", "Audio analysis fallback"),
        ("video", "Video analysis fallback"),
        ("document", "Document analysis fallback"),
    ];

    let mut rows: Vec<Line> = Vec::new();
    let config = &app.loaded_config.config.attachment_models;

    for (i, (modality, description)) in modalities.iter().enumerate() {
        let selected = i == app.selected_attachment_model;
        let resolved_label =
            crate::keybindings::modals::resolve_attachment_model_label(app, modality);
        let has_override =
            crate::keybindings::modals::attachment_model_has_override(config, modality);

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
            Span::styled(format!("{:>20}", modality), task_style),
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
