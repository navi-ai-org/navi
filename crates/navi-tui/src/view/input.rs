use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, Paragraph};

use crate::TuiApp;
use crate::input::{COMPOSER_MAX_VISIBLE_LINES, input_visual_line_count};
use crate::providers::selected_provider_label;
use crate::render::cursor_span;
use crate::theme::*;
use crate::ui::interaction::HitAction;
use crate::ui::text_input::floor_char_boundary;

pub(super) fn render_input(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 0,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(2), Constraint::Length(1)])
        .split(inner);

    frame.render_widget(
        Block::new().style(Style::default().fg(text()).bg(composer_panel_bg(app))),
        rows[0],
    );

    let panel_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(rows[0].inner(Margin {
            horizontal: 2,
            vertical: 0,
        }));
    let input_area = panel_rows[0];
    app.input_wrap_width = input_area.width as usize;
    let (lines, cursor_line) = input_lines(app, input_area.width as usize);
    let input_lines = visible_input_lines(lines, input_area.height as usize, cursor_line);
    frame.render_widget(
        Paragraph::new(Text::from(input_lines))
            .style(Style::default().fg(text()).bg(composer_panel_bg(app)))
            .block(Block::new()),
        input_area,
    );
    frame.render_widget(
        Paragraph::new(composer_status_line(app, panel_rows[1].width as usize))
            .style(Style::default().bg(composer_panel_bg(app))),
        panel_rows[1],
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

pub(super) fn composer_height(app: &TuiApp, input_width: usize) -> u16 {
    let visible_lines = input_visual_line_count(&app.input, input_width)
        .clamp(1, COMPOSER_MAX_VISIBLE_LINES) as u16;
    visible_lines + 3
}

fn composer_panel_bg(app: &TuiApp) -> ratatui::style::Color {
    if app.is_loading {
        interactive_hover_bg()
    } else {
        panel()
    }
}

fn visible_input_lines(
    lines: Vec<Line<'static>>,
    height: usize,
    cursor_line: usize,
) -> Vec<Line<'static>> {
    let height = height.max(1);
    let mut start = cursor_line.saturating_add(1).saturating_sub(height);
    if start + height > lines.len() {
        start = lines.len().saturating_sub(height);
    }
    lines.into_iter().skip(start).take(height).collect()
}

fn input_lines(app: &TuiApp, width: usize) -> (Vec<Line<'static>>, usize) {
    let prompt = "";
    let continuation = " ".repeat(prompt.chars().count());
    let width = width.max(prompt.chars().count() + 1);
    let text_style = Style::default().fg(text());
    let prefix_style = Style::default().fg(signal()).add_modifier(Modifier::BOLD);
    let mut lines = Vec::new();
    let mut current = vec![Span::styled(prompt.to_string(), prefix_style)];
    let mut current_width = prompt.chars().count();
    let mut cursor_line = 0usize;
    let mut cursor_drawn = false;

    let push_wrapped_char = |lines: &mut Vec<Line<'static>>,
                             current: &mut Vec<Span<'static>>,
                             current_width: &mut usize,
                             span: Span<'static>,
                             width: usize,
                             continuation: &str,
                             prefix_style: Style| {
        if *current_width >= width {
            lines.push(Line::from(std::mem::take(current)));
            current.push(Span::styled(continuation.to_string(), prefix_style));
            *current_width = continuation.chars().count();
        }
        *current_width += span.content.chars().count();
        current.push(span);
    };

    if app.input.is_empty() {
        let context = app.compact_state.usage_label(0);
        let model = selected_model_label(app);
        let provider = selected_provider_label(app);
        let thinking = app.thinking_level.label();
        let status = if app.is_loading { "Build" } else { "Ready" };
        current.push(Span::styled(
            format!("{status} · {model} {provider} · {thinking}"),
            Style::default().fg(muted()),
        ));
        current.push(cursor_span(" "));
        lines.push(Line::from(current));
        return (lines, cursor_line);
    }

    let cursor = app.input_cursor.min(app.input.len());
    let cursor = floor_char_boundary(&app.input, cursor);
    for (byte, ch) in app.input.char_indices() {
        if !cursor_drawn && cursor == byte {
            if ch != '\n' {
                if current_width >= width {
                    lines.push(Line::from(std::mem::take(&mut current)));
                    current.push(Span::styled(continuation.clone(), prefix_style));
                    current_width = continuation.chars().count();
                }
                cursor_line = lines.len();
                push_wrapped_char(
                    &mut lines,
                    &mut current,
                    &mut current_width,
                    cursor_span(ch.to_string()),
                    width,
                    &continuation,
                    prefix_style,
                );
                cursor_drawn = true;
                continue;
            }
            if current_width >= width {
                lines.push(Line::from(std::mem::take(&mut current)));
                current.push(Span::styled(continuation.clone(), prefix_style));
                current_width = continuation.chars().count();
            }
            cursor_line = lines.len();
            push_wrapped_char(
                &mut lines,
                &mut current,
                &mut current_width,
                cursor_span(" "),
                width,
                &continuation,
                prefix_style,
            );
            cursor_drawn = true;
        }

        if ch == '\n' {
            lines.push(Line::from(std::mem::take(&mut current)));
            current.push(Span::styled(continuation.clone(), prefix_style));
            current_width = continuation.chars().count();
            continue;
        }

        push_wrapped_char(
            &mut lines,
            &mut current,
            &mut current_width,
            Span::styled(ch.to_string(), text_style),
            width,
            &continuation,
            prefix_style,
        );
    }

    if !cursor_drawn {
        if current_width >= width {
            lines.push(Line::from(std::mem::take(&mut current)));
            current.push(Span::styled(continuation.clone(), prefix_style));
            current_width = continuation.chars().count();
        }
        cursor_line = lines.len();
        push_wrapped_char(
            &mut lines,
            &mut current,
            &mut current_width,
            cursor_span(" "),
            width,
            &continuation,
            prefix_style,
        );
    }

    lines.push(Line::from(current));
    (lines, cursor_line)
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

    let context = app.compact_state.usage_label(0);
    let help = "ctrl+p commands";
    let footer = format!("{context}  {help}");
    let padding = width.saturating_sub(footer.chars().count());
    Line::from(vec![
        Span::styled(" ".repeat(padding), Style::default().fg(muted())),
        Span::styled(context, Style::default().fg(muted())),
        Span::styled("  ", Style::default().fg(ghost())),
        Span::styled(
            "ctrl+p",
            Style::default().fg(text()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" commands", Style::default().fg(muted())),
    ])
}

fn composer_status_line(app: &TuiApp, width: usize) -> Line<'static> {
    let provider = selected_provider_label(app);
    let thinking = app.thinking_level.label();
    let status = if app.is_loading { "Build" } else { "Ready" };
    let model = selected_model_label(app);
    let text_width = status.chars().count()
        + model.chars().count()
        + provider.chars().count()
        + thinking.chars().count()
        + 9;
    let mut spans = vec![Span::styled(
        status.to_string(),
        Style::default().fg(accent()),
    )];

    if text_width <= width {
        spans.extend([
            Span::styled(" · ", Style::default().fg(ghost())),
            Span::styled(model, Style::default().fg(muted())),
            Span::styled(" ", Style::default().fg(ghost())),
            Span::styled(provider.to_string(), Style::default().fg(muted())),
            Span::styled(" · ", Style::default().fg(ghost())),
            Span::styled(
                thinking.to_string(),
                Style::default()
                    .fg(code_const())
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn input_lines_wrap_long_text_and_keep_cursor_visible() {
        let mut app = crate::tests::test_app(&"a".repeat(30));
        app.input_cursor = app.input.len();

        let (lines, cursor_line) = input_lines(&app, 12);
        let visible = visible_input_lines(lines, 2, cursor_line);
        let text = visible.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert_eq!(visible.len(), 2);
        assert!(cursor_line >= 2);
        assert!(text.contains("aaa"));
        assert!(text.ends_with(' '));
    }

    #[test]
    fn input_lines_show_previous_line_after_trailing_newline() {
        let mut app = crate::tests::test_app("abc\n");
        app.input_cursor = app.input.len();

        let (lines, cursor_line) = input_lines(&app, 20);
        let visible = visible_input_lines(lines, 2, cursor_line);
        let text = visible.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert_eq!(visible.len(), 2);
        assert_eq!(cursor_line, 1);
        assert!(text.contains("abc"));
        assert!(text.lines().last().unwrap_or_default().starts_with(' '));
    }
}
