use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use crate::TuiApp;
use crate::render::{cursor_span, split_input_spans};
use crate::theme::*;
use crate::ui::interaction::HitAction;
use crate::ui::text_input::{floor_char_boundary, next_char_boundary};

pub(super) fn render_input(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 0,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    let border_style = if app.is_loading {
        Style::default().fg(accent()).bg(bg())
    } else {
        Style::default().fg(ghost()).bg(bg())
    };
    frame.render_widget(
        Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(border_style)
            .style(Style::default().bg(bg())),
        rows[0],
    );

    let input_area = rows[0].inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let input_lines = visible_input_lines(input_lines(app), input_area.height as usize);
    frame.render_widget(
        Paragraph::new(Text::from(input_lines))
            .style(Style::default().bg(bg()))
            .wrap(Wrap { trim: false })
            .block(Block::new().borders(Borders::NONE)),
        input_area,
    );
    frame.render_widget(
        Paragraph::new(shortcut_tips(app, rows[1].width as usize)).style(Style::default().bg(bg())),
        rows[1],
    );
    if !app.pending_questions.is_empty() {
        app.register_hit(
            rows[1],
            3,
            "reopen pending question",
            HitAction::ReopenQuestion,
        );
    }
}

fn visible_input_lines(lines: Vec<Line<'_>>, height: usize) -> Vec<Line<'_>> {
    let height = height.max(1);
    let start = lines.len().saturating_sub(height);
    lines.into_iter().skip(start).collect()
}

fn input_lines(app: &TuiApp) -> Vec<Line<'_>> {
    let prompt = "} ";
    let continuation = " ".repeat(prompt.chars().count());
    let mut spans = vec![Span::styled(
        prompt,
        Style::default().fg(signal()).add_modifier(Modifier::BOLD),
    )];

    if app.input.is_empty() {
        spans.push(cursor_span(" "));
        let placeholder = if app.is_loading { " thinking..." } else { "" };
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
    if !app.pending_questions.is_empty() {
        return Line::from(vec![
            Span::styled(" ", Style::default().fg(muted())),
            Span::styled("pending question  ", Style::default().fg(muted())),
            Span::styled(
                "ctrl+enter",
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" reopen", Style::default().fg(muted())),
        ]);
    }

    if app.messages.is_empty() && app.conversation_history.len() <= 1 && app.input.is_empty() {
        return Line::from(vec![
            Span::styled(" ", Style::default().fg(muted())),
            Span::styled("type a task, or ", Style::default().fg(muted())),
            Span::styled(
                "ctrl+p",
                Style::default().fg(text()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" for commands", Style::default().fg(muted())),
        ]);
    }

    let items = [
        ("Ctrl+.", "shortcuts", text()),
        ("Ctrl+P", "commands", text()),
        ("Ctrl+M", "models", text()),
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

    let mode_label = if app.yolo_mode {
        "always-approve"
    } else {
        "approve"
    };
    let composer_text = format!("Composer {} · {mode_label}", selected_model_label(app));
    let composer_width = composer_text.chars().count();
    if used + composer_width + 2 < width {
        let padding = width.saturating_sub(used + composer_width + 1);
        spans.push(Span::styled(
            " ".repeat(padding),
            Style::default().fg(muted()),
        ));
        spans.push(Span::styled(composer_text, Style::default().fg(muted())));
    }

    Line::from(spans)
}

fn selected_model_label(app: &TuiApp) -> String {
    let label = app
        .models
        .get(app.selected_model)
        .map(|model| model.name.as_str())
        .unwrap_or("model");
    if label.chars().count() <= 24 {
        return label.to_string();
    }
    let mut shortened = label.chars().take(23).collect::<String>();
    shortened.push('…');
    shortened
}
