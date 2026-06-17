use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui_image::Image;

use crate::app::TuiApp;
use crate::theme::*;
use crate::ui::interaction::HitAction;
use crate::view::input::composer_panel_bg;

/// Height of the image preview area in rows (including borders).
const IMAGE_PREVIEW_HEIGHT: u16 = 6;
/// Width of each image thumbnail in characters.
const THUMBNAIL_WIDTH: u16 = 14;

/// Render image previews above the input area.
/// Returns the area consumed by the preview, so the input can be placed below.
pub(super) fn render_image_previews(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) -> Rect {
    if app.pending_images.is_empty() {
        return area;
    }

    let preview_height = IMAGE_PREVIEW_HEIGHT.min(area.height);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(preview_height), Constraint::Min(0)])
        .split(area);

    let preview_area = chunks[0];
    let remaining_area = chunks[1];

    render_image_strip(frame, app, preview_area);

    remaining_area
}

/// Render the horizontal strip of image thumbnails with remove buttons.
fn render_image_strip(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 0,
    });

    let block = Block::default()
        .borders(Borders::LEFT)
        .border_set(ratatui::symbols::border::Set {
            vertical_left: "▌",
            ..ratatui::symbols::border::PLAIN
        })
        .border_style(Style::default().fg(accent()).bg(composer_panel_bg(app)))
        .style(Style::default().bg(composer_panel_bg(app)));

    let inner_area = block.inner(inner);
    frame.render_widget(block, inner);

    // Render background
    frame.render_widget(
        Block::new().style(Style::default().bg(composer_panel_bg(app))),
        inner_area,
    );

    let image_count = app.pending_images.len();
    let available_width = inner_area.width as usize;

    // Calculate layout for each image thumbnail
    let thumb_width = THUMBNAIL_WIDTH as usize;
    let spacing = 1;
    let total_needed = image_count * thumb_width + (image_count.saturating_sub(1)) * spacing;

    // If we have enough space, render thumbnails side by side
    if total_needed <= available_width && inner_area.height >= 4 {
        render_thumbnail_row(frame, app, inner_area);
    } else {
        // Fallback: render as compact text labels
        render_compact_labels(frame, app, inner_area);
    }
}

/// Render image thumbnails as a horizontal row.
fn render_thumbnail_row(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let thumb_width = THUMBNAIL_WIDTH.min(area.width);
    let thumb_height = area.height.min(6);

    let num_images = app.pending_images.len();
    for i in 0..num_images {
        let x_offset = (i as u16) * (thumb_width + 1);
        if x_offset >= area.width {
            break;
        }

        let thumb_area = Rect {
            x: area.x + x_offset,
            y: area.y,
            width: thumb_width.min(area.width - x_offset),
            height: thumb_height,
        };

        let title_span = Span::styled(
            " Ⓧ ",
            Style::default()
                .fg(ratatui::style::Color::White)
                .add_modifier(Modifier::BOLD),
        );

        let title_line = Line::from(title_span).alignment(Alignment::Right);

        let container = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(muted()))
            .style(Style::default().bg(panel()))
            .title_top(title_line);

        let inner_thumb_area = container.inner(thumb_area);
        frame.render_widget(container, thumb_area);

        let close_btn_rect = Rect {
            x: thumb_area.x + thumb_area.width.saturating_sub(4),
            y: thumb_area.y,
            width: 3,
            height: 1,
        };
        app.register_hit(
            close_btn_rect,
            10,
            format!("remove_image_{}", i),
            HitAction::RemoveImage(i),
        );
        app.register_hit(
            thumb_area,
            5,
            format!("maximize_image_{}", i),
            HitAction::MaximizeImage(i),
        );

        // Render thumbnail if protocol is available
        if let Some(ref mut protocol) = app.pending_images[i].protocol {
            let image_widget = ratatui_image::StatefulImage::new();
            frame.render_stateful_widget(image_widget, inner_thumb_area, protocol);
        } else {
            let fallback = Paragraph::new(app.pending_images[i].label())
                .style(Style::default().fg(muted()))
                .alignment(Alignment::Center)
                .block(Block::default().padding(ratatui::widgets::Padding::vertical(1)));
            frame.render_widget(fallback, inner_thumb_area);
        }
    }
}

/// Render compact text labels when thumbnails don't fit.
fn render_compact_labels(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let mut current_x = area.x;
    let labels: Vec<Span<'static>> = app
        .pending_images
        .iter()
        .enumerate()
        .flat_map(|(i, img)| {
            let text_label = format!(" {} ", img.label());
            let close_label = "Ⓧ ";
            
            let text_width = text_label.chars().count() as u16;
            let close_width = close_label.chars().count() as u16;
            
            app.register_hit(
                Rect {
                    x: current_x,
                    y: area.y,
                    width: text_width,
                    height: 1,
                },
                10,
                format!("maximize_image_{}", i),
                HitAction::MaximizeImage(i),
            );
            current_x += text_width;

            app.register_hit(
                Rect {
                    x: current_x,
                    y: area.y,
                    width: close_width,
                    height: 1,
                },
                10,
                format!("remove_image_{}", i),
                HitAction::RemoveImage(i),
            );
            current_x += close_width;

            let mut spans = vec![
                Span::styled(text_label, Style::default().fg(muted())),
                Span::styled(close_label, Style::default().fg(muted())),
            ];
            if i < app.pending_images.len() - 1 {
                spans.push(Span::styled("| ", Style::default().fg(ghost())));
                current_x += 2;
            }
            spans
        })
        .collect();

    let line = Line::from(labels);
    frame.render_widget(Paragraph::new(line), area);
}

pub(crate) fn render_maximized_image(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect, idx: usize) {
    use ratatui::widgets::Clear;
    use crate::render::modal_rect;

    let max_modal_width = area.width.saturating_mul(90) / 100;
    let max_modal_height = area.height.saturating_mul(90) / 100;

    let title_span = Span::styled(
        " Ⓧ ",
        Style::default()
            .fg(ratatui::style::Color::White)
            .add_modifier(Modifier::BOLD),
    );
    let title_line = Line::from(title_span).alignment(Alignment::Right);

    let container = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(muted()))
        .style(Style::default().bg(bg()))
        .title_top(title_line);

    let mut modal_area = modal_rect(area, max_modal_width, max_modal_height);

    if let Some(img) = app.pending_images.get(idx) {
        if let Some(ref protocol) = img.protocol {
            let max_inner_area = ratatui::layout::Rect {
                x: 0,
                y: 0,
                width: max_modal_width.saturating_sub(2),
                height: max_modal_height.saturating_sub(2),
            };

            let img_size = protocol.size_for(ratatui_image::Resize::Scale(None), max_inner_area.into());
            
            let target_modal_width = img_size.width.saturating_add(2).min(max_modal_width).max(20);
            let target_modal_height = img_size.height.saturating_add(2).min(max_modal_height).max(5);
            
            modal_area = modal_rect(area, target_modal_width, target_modal_height);
        }
    }

    let inner_area = container.inner(modal_area);

    // Register close hit areas before mutable borrow
    app.register_hit(area, 20, "close_maximized_bg", HitAction::CloseMaximizedImage);
    app.register_hit(modal_area, 21, "close_maximized_fg", HitAction::CloseMaximizedImage);

    frame.render_widget(Clear, modal_area);
    frame.render_widget(container, modal_area);

    if let Some(img) = app.pending_images.get_mut(idx) {
        if let Some(ref mut protocol) = img.protocol {
            let image_widget = ratatui_image::StatefulImage::new().resize(ratatui_image::Resize::Scale(None));
            frame.render_stateful_widget(image_widget, inner_area, protocol);
        } else {
            let fallback = Paragraph::new(img.label())
                .style(Style::default().fg(muted()))
                .alignment(Alignment::Center)
                .block(Block::default().padding(ratatui::widgets::Padding::vertical(1)));
            frame.render_widget(fallback, inner_area);
        }
    }
}
