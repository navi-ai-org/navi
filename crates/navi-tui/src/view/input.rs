use ratatui::layout::{Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, Paragraph};

use crate::TuiApp;
use crate::input::{
    COMPOSER_MAX_VISIBLE_LINES, input_visual_line_count, input_visual_line_ranges,
    selected_input_range,
};
use crate::providers::selected_provider_label;
use crate::render::cursor_span;
use crate::theme::*;
use crate::ui::interaction::HitAction;
use crate::ui::text_input::floor_char_boundary;

const INPUT_TOP_PADDING_ROWS: u16 = 1;
const FOOTER_BOTTOM_PADDING_ROWS: u16 = 1;
const INPUT_TEXT_INSET_COLUMNS: u16 = 3;

pub(super) fn render_input(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 0,
    });

    let block = Block::new()
        .borders(ratatui::widgets::Borders::LEFT)
        .border_set(ratatui::symbols::border::Set {
            vertical_left: "▌",
            ..ratatui::symbols::border::PLAIN
        })
        .border_style(Style::default().fg(accent()).bg(composer_panel_bg(app)))
        .style(Style::default().fg(text()).bg(composer_panel_bg(app)));

    let inner_block_area = block.inner(inner);
    frame.render_widget(block, inner);

    frame.render_widget(
        Block::new().style(Style::default().bg(composer_panel_bg(app))),
        inner_block_area,
    );

    let panel_area = inner_block_area.inner(Margin {
        horizontal: INPUT_TEXT_INSET_COLUMNS,
        vertical: 0,
    });
    let input_y = panel_area.y + INPUT_TOP_PADDING_ROWS.min(panel_area.height);
    let last_panel_y = panel_area.y + panel_area.height.saturating_sub(1);
    let desired_footer_y = panel_area.y
        + panel_area
            .height
            .saturating_sub(1 + FOOTER_BOTTOM_PADDING_ROWS);
    let footer_y = desired_footer_y
        .max(input_y.saturating_add(1))
        .min(last_panel_y);
    let footer_area = Rect::new(
        panel_area.x,
        footer_y,
        panel_area.width,
        1.min(panel_area.height),
    );
    let input_bottom = footer_area.y;
    let input_area = Rect::new(
        panel_area.x,
        input_y,
        panel_area.width,
        input_bottom.saturating_sub(input_y).max(1),
    );
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
        Paragraph::new(composer_footer_line(app, footer_area.width as usize))
            .style(Style::default().bg(composer_panel_bg(app))),
        footer_area,
    );

    if !app.pending_questions.is_empty() {
        app.register_hit(
            footer_area,
            3,
            "reopen pending question",
            HitAction::ReopenQuestion,
        );
    }
}

pub(super) fn composer_height(app: &TuiApp, input_width: usize) -> u16 {
    let wrap_width = input_width.saturating_sub(6);
    let visible_lines =
        input_visual_line_count(&app.input, wrap_width).clamp(1, COMPOSER_MAX_VISIBLE_LINES) as u16;
    // top inset + text area (min 3 rows) + footer
    INPUT_TOP_PADDING_ROWS + visible_lines.max(3) + 1
}

pub(crate) fn composer_panel_bg(_app: &TuiApp) -> ratatui::style::Color {
    interactive_bg()
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
    let width = width.max(1);
    let text_style = Style::default().fg(text());

    if app.input.is_empty() {
        let mut current = vec![cursor_span(" ")];
        let placeholder = if app.is_loading {
            let glitches = [
                "processing...",
                "pr0cessing...",
                "processing...",
                "proce55ing...",
                "processing...",
                "pr0c3ss1ng...",
                "p#ocessing...",
                "processing...",
                "process!ng...",
                "pr0cessin9...",
                "processing...",
                "p-r-o-c-e-s-s-i-n-g...",
            ];
            let frame = (app.tick() / 4) as usize % glitches.len();
            format!(" {}", glitches[frame])
        } else {
            "Describe the task...".to_string()
        };
        current.push(Span::styled(placeholder, Style::default().fg(muted())));
        return (vec![Line::from(current)], 0);
    }

    let cursor = app.input_cursor.min(app.input.len());
    let cursor = floor_char_boundary(&app.input, cursor);
    let selected = selected_input_range(app);
    let ranges = input_visual_line_ranges(&app.input, width);
    let mut cursor_line = ranges.len().saturating_sub(1);
    let mut lines = Vec::new();

    for (line_index, (start, end)) in ranges.iter().copied().enumerate() {
        if cursor >= start && cursor <= end {
            cursor_line = line_index;
        }
        let mut spans = Vec::new();
        let mut cursor_drawn = false;
        for (offset, ch) in app.input[start..end].char_indices() {
            let byte = start + offset;
            if cursor == byte {
                let cursor_text = if ch == '\t' {
                    " ".to_string()
                } else {
                    ch.to_string()
                };
                spans.push(cursor_span(cursor_text));
                cursor_drawn = true;
                continue;
            }
            let mut style = text_style;
            if selected.is_some_and(|(sel_start, sel_end)| byte >= sel_start && byte < sel_end) {
                style = style.fg(selection_fg()).bg(selection_bg());
            }
            spans.push(Span::styled(ch.to_string(), style));
        }
        if !cursor_drawn && cursor == end {
            spans.push(cursor_span(" "));
        }
        lines.push(Line::from(spans));
    }
    (lines, cursor_line)
}

fn composer_footer_line(app: &TuiApp, _width: usize) -> Line<'static> {
    if !app.pending_questions.is_empty() {
        return Line::from(vec![
            Span::styled("Question pending", Style::default().fg(code_const())),
            Span::styled(" · ", Style::default().fg(ghost())),
            Span::styled(
                "ctrl+enter",
                Style::default().fg(signal()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" reopen", Style::default().fg(muted())),
        ]);
    }

    let mut spans: Vec<Span<'static>> = Vec::new();

    // Show pending image indicator if any images are attached.
    if !app.pending_images.is_empty() {
        let count = app.pending_images.len();
        spans.push(Span::styled(
            format!("{} image{}", count, if count > 1 { "s" } else { "" }),
            Style::default().fg(signal()).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(ghost())));
    } else if app.input.is_empty() {
        spans.push(Span::styled(
            "ctrl+v paste image",
            Style::default().fg(ghost()).add_modifier(Modifier::ITALIC),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(ghost())));
    }

    let provider = selected_provider_label(app);
    let thinking = app.thinking_level.label();
    let model = selected_model_label(app);
    let context = app.compact_state.usage_label(0);

    spans.push(Span::styled(model, Style::default().fg(code_type())));
    spans.push(Span::styled(" ", Style::default().fg(ghost())));
    spans.push(Span::styled(
        provider.to_string(),
        Style::default().fg(signal()).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(" · ", Style::default().fg(ghost())));
    spans.push(Span::styled(
        thinking.to_string(),
        Style::default()
            .fg(code_const())
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(" · ", Style::default().fg(ghost())));
    spans.push(Span::styled(
        context,
        Style::default()
            .fg(code_number())
            .add_modifier(Modifier::BOLD),
    ));

    Line::from(spans)
}

fn selected_model_label(app: &TuiApp) -> String {
    let label = app
        .models
        .get(app.selected_model)
        .map(|model| model.name.as_str())
        .unwrap_or("model");
    let label = label.rsplit('/').next().unwrap_or(label);
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

    #[test]
    fn selected_model_label_hides_provider_prefix() {
        let mut app = crate::tests::test_app("");
        let selected = app.selected_model;
        app.models[selected].name = "ai21/jamba-large-1.7".to_string();

        assert_eq!(selected_model_label(&app), "jamba-large-1.7");
    }
}
