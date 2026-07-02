use ratatui::layout::Rect;
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::TuiApp;
use crate::theme::*;
use crate::ui::interaction::HitAction;
use crate::view::input::composer_panel_bg;

/// Height of the image attachment strip in rows.
pub(crate) const IMAGE_PREVIEW_HEIGHT: u16 = 3;

/// Render image attachment labels above the input area.
pub(super) fn render_image_previews(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) -> Rect {
    if app.pending_images.is_empty() {
        return area;
    }

    let preview_area = Rect {
        height: IMAGE_PREVIEW_HEIGHT.min(area.height),
        ..area
    };
    render_image_strip(frame, app, preview_area);
    preview_area
}

fn render_image_strip(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_set(ratatui::symbols::border::Set {
            vertical_left: "▌",
            ..ratatui::symbols::border::PLAIN
        })
        .border_style(Style::default().fg(accent()).bg(composer_panel_bg(app)))
        .style(Style::default().bg(composer_panel_bg(app)));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut spans = Vec::new();
    let mut current_x = inner.x;
    for i in 0..app.pending_images.len() {
        let label = app.pending_images[i].numbered_label(i);
        let text_label = format!(" {label} ");
        let close_label = "x ";

        spans.push(Span::styled(
            text_label.clone(),
            Style::default().fg(muted()),
        ));
        current_x = current_x.saturating_add(text_label.chars().count() as u16);

        app.register_hit(
            Rect {
                x: current_x,
                y: inner.y,
                width: close_label.chars().count() as u16,
                height: 1,
            },
            10,
            format!("remove_image_{i}"),
            HitAction::RemoveImage(i),
        );
        spans.push(Span::styled(close_label, Style::default().fg(muted())));
        current_x = current_x.saturating_add(close_label.chars().count() as u16);

        if i + 1 < app.pending_images.len() {
            spans.push(Span::styled("| ", Style::default().fg(ghost())));
            current_x = current_x.saturating_add(2);
        }
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(composer_panel_bg(app))),
        inner,
    );
}
