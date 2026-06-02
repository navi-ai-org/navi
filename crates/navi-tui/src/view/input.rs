use navi_sdk::{AgentMode, CompactThreshold};
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::TuiApp;
use crate::render::{cursor_span, split_input_spans};
use crate::theme::*;
use crate::ui::text_input::{floor_char_boundary, next_char_boundary};

pub(super) fn render_input(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
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
            .style(Style::default().bg(bg()))
            .wrap(Wrap { trim: false })
            .block(Block::new().borders(Borders::NONE)),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(shortcut_tips(app, rows[1].width as usize)).style(Style::default().bg(bg())),
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
        Style::default().fg(signal()).add_modifier(Modifier::BOLD),
    )];

    if app.input.is_empty() {
        spans.push(cursor_span(" "));
        let placeholder = if app.is_loading {
            " Thinking..."
        } else {
            " Ready!"
        };
        spans.push(Span::styled(placeholder, Style::default().fg(muted())));
        return vec![Line::from(spans)];
    }

    let cursor = app.input_cursor.min(app.input.len());
    let cursor = floor_char_boundary(&app.input, cursor);
    let (before, rest) = app.input.split_at(cursor);
    spans.push(Span::styled(before, Style::default().fg(text())));

    if rest.is_empty() {
        spans.push(cursor_span(" "));
    } else {
        let next = next_char_boundary(&app.input, cursor).unwrap_or(app.input.len());
        let (cursor_text, after) = app.input[cursor..].split_at(next - cursor);
        spans.push(cursor_span(cursor_text));
        spans.push(Span::styled(after, Style::default().fg(text())));
    }

    split_input_spans(spans, &continuation)
}

fn shortcut_tips(app: &TuiApp, width: usize) -> Line<'static> {
    let agent_label = app.selected_agent.map(AgentMode::label).unwrap_or("none");
    if app.messages.is_empty() && app.conversation_history.len() <= 1 && app.input.is_empty() {
        return Line::from(vec![
            Span::styled(" ", Style::default().fg(muted())),
            Span::styled(
                "type a task, /plan, /edit, /review, or ",
                Style::default().fg(muted()),
            ),
            Span::styled(
                "ctrl+p",
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" for commands; ", Style::default().fg(muted())),
            Span::styled(
                "tab",
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" changes agent ({agent_label})"),
                Style::default().fg(muted()),
            ),
        ]);
    }

    let items = [
        ("?", "for shortcuts", text()),
        ("ctrl+p", "commands", text()),
        ("tab", agent_label, text()),
        ("ctrl+c", "quit", text()),
    ];

    let mut spans = vec![Span::styled(" ", Style::default().fg(muted()))];
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
            spans.push(Span::styled(" · ", Style::default().fg(ghost())));
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
                Style::default().fg(muted()),
            ));
            used += 1 + label.chars().count();
        }
    }

    let compact_state = &app.compact_state;
    let threshold = compact_state.threshold_level(app.input.len());
    let pct_label = format!(" {}", compact_state.usage_label(app.input.len()));
    let pct_color = match threshold {
        CompactThreshold::CircuitOpen => signal(),
        CompactThreshold::Error => signal(),
        CompactThreshold::Warning => accent(),
        CompactThreshold::Normal => muted(),
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
            Style::default().fg(muted()),
        ));
        spans.push(Span::styled("ctx:".to_string(), Style::default().fg(muted())));
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
