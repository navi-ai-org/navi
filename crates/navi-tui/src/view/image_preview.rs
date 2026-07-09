use ratatui::layout::Rect;
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui_image::{Resize, StatefulImage};

use crate::app::TuiApp;
use crate::render::markdown::{is_image_tag, parse_image_tag_index};
use crate::render::text::display_width;
use crate::state::ImageHoverPreview;
use crate::theme::*;
use crate::ui::interaction::HitAction;
use crate::view::terminal_graphics;

/// Height of the legacy attachment strip (kept for layout callers that still reserve space).
#[allow(dead_code)]
pub(crate) const IMAGE_PREVIEW_HEIGHT: u16 = 0;

/// Composer strip is no longer used — images show as `[Image N]` chips in the input.
#[allow(dead_code)]
pub(crate) fn render_image_previews(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) -> Rect {
    let _ = (frame, app);
    Rect {
        height: 0,
        ..area
    }
}

/// Floating hover modal for an `[Image N]` chip (composer or chat).
///
/// - **Kitty / Sixel / iTerm2:** bordered card with metadata header + live image.
/// - **Other terminals:** compact text-only metadata card (no fake halfblock preview).
pub(crate) fn render_image_hover_modal(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let Some(preview) = app.image_hover.clone() else {
        return;
    };
    if area.width < 24 || area.height < 3 {
        return;
    }

    let graphics = terminal_graphics::session_graphics();
    let show_pixels = graphics.supports_image_preview() && app.image_hover_protocol.is_some();

    let header = preview.header_line();
    let header_w = display_width(&header) as u16;

    let max_body = ratatui::layout::Size::new(
        area.width.saturating_sub(6).max(20),
        area.height.saturating_sub(6).max(4),
    );

    let (modal_width, modal_height) = if show_pixels {
        let est = graphics.estimate_cells(preview.width, preview.height, max_body);
        let w = header_w
            .saturating_add(4)
            .max(est.width.saturating_add(4))
            .min(area.width.saturating_sub(2).max(24));
        let h = est
            .height
            .saturating_add(3) // borders + title
            .min(area.height.saturating_sub(2).max(8));
        (w, h)
    } else {
        // Text-only: slim card with metadata in the title (like the reference chrome).
        let w = header_w
            .saturating_add(6)
            .clamp(28, area.width.saturating_sub(4).max(24));
        (w, 3.min(area.height))
    };

    let x = area
        .x
        .saturating_add(area.width.saturating_sub(modal_width) / 2);
    let y = area.y.saturating_add(1);
    let modal = Rect {
        x,
        y,
        width: modal_width,
        height: modal_height,
    };

    frame.render_widget(Clear, modal);

    let title = Line::from(vec![
        Span::styled(" ", Style::default().bg(panel())),
        Span::styled(
            header,
            Style::default()
                .fg(text())
                .bg(panel())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::default().bg(panel())),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ghost()).bg(panel()))
        .style(Style::default().bg(panel()).fg(text()))
        .title(title);
    let inner = block.inner(modal);
    frame.render_widget(block, modal);

    if show_pixels {
        if let Some(protocol) = app.image_hover_protocol.as_mut() {
            // Image fills the body; StatefulImage fits to the area.
            let image = StatefulImage::default().resize(Resize::Fit(None));
            frame.render_stateful_widget(image, inner, protocol);
        }
    } else if inner.height > 0 {
        // Text-only: fill the slim card body so the border doesn't look empty.
        frame.render_widget(
            Paragraph::new("").style(Style::default().bg(panel())),
            inner,
        );
    }
}

/// Register hit regions for `[Image N]` chips on a rendered chat line.
pub(crate) fn register_chat_image_hits(
    app: &mut TuiApp,
    line: &ratatui::text::Line<'_>,
    line_area: Rect,
    message_index: usize,
) {
    let Some(message) = app.messages.get(message_index) else {
        return;
    };
    if message.images.is_empty() {
        return;
    }

    let mut col = 0u16;
    for span in &line.spans {
        let width = display_width(&span.content) as u16;
        if is_image_tag(&span.content)
            && let Some(one_based) = parse_image_tag_index(&span.content)
        {
            let image_index = one_based.saturating_sub(1);
            if image_index < message.images.len() {
                let hit = Rect {
                    x: line_area.x.saturating_add(col),
                    y: line_area.y,
                    width: width.max(1),
                    height: 1,
                };
                app.register_hit(
                    hit,
                    20,
                    format!("preview_chat_image_{message_index}_{image_index}"),
                    HitAction::PreviewChatImage {
                        message_index,
                        image_index,
                    },
                );
            }
        }
        col = col.saturating_add(width);
    }
}

/// Register hit regions for `[Image N]` chips inside the composer input area.
pub(crate) fn register_pending_image_hits(
    app: &mut TuiApp,
    input_text: &str,
    line_start_byte: usize,
    line_text: &str,
    line_area: Rect,
) {
    if app.pending_images.is_empty() {
        return;
    }
    let mut col = 0u16;
    let mut idx = 0usize;
    while idx < line_text.len() {
        let rest = &line_text[idx..];
        let rest_bytes = rest.as_bytes();
        if rest_bytes.starts_with(b"[Image ") {
            let mut check_idx = 7;
            let mut has_digits = false;
            while check_idx < rest_bytes.len() && rest_bytes[check_idx].is_ascii_digit() {
                has_digits = true;
                check_idx += 1;
            }
            if has_digits && check_idx < rest_bytes.len() && rest_bytes[check_idx] == b']' {
                let tag = &line_text[idx..idx + check_idx + 1];
                let width = display_width(tag) as u16;
                if let Some(one_based) = parse_image_tag_index(tag) {
                    let image_index = one_based.saturating_sub(1);
                    if image_index < app.pending_images.len() {
                        let absolute = line_start_byte + idx;
                        if absolute < input_text.len() {
                            let hit = Rect {
                                x: line_area.x.saturating_add(col),
                                y: line_area.y,
                                width: width.max(1),
                                height: 1,
                            };
                            app.register_hit(
                                hit,
                                20,
                                format!("preview_pending_image_{image_index}"),
                                HitAction::PreviewPendingImage(image_index),
                            );
                        }
                    }
                }
                col = col.saturating_add(width);
                idx += check_idx + 1;
                continue;
            }
        }
        if let Some(ch) = rest.chars().next() {
            col = col.saturating_add(display_width(&ch.to_string()) as u16);
            idx += ch.len_utf8();
        } else {
            break;
        }
    }
}

pub(crate) fn set_hover_from_action(app: &mut TuiApp, action: &HitAction) -> bool {
    let preview = match action {
        HitAction::PreviewPendingImage(index) => app
            .pending_images
            .get(*index)
            .map(|image| ImageHoverPreview::from_pending(*index, image)),
        HitAction::PreviewChatImage {
            message_index,
            image_index,
        } => app
            .messages
            .get(*message_index)
            .and_then(|message| message.images.get(*image_index))
            .map(ImageHoverPreview::from_chat),
        _ => return false,
    };

    // Rebuild graphics protocol only when the hovered image changes.
    let same = match (&app.image_hover, &preview) {
        (Some(prev), Some(next)) => {
            prev.index == next.index
                && prev.size_bytes == next.size_bytes
                && prev.data == next.data
        }
        _ => false,
    };

    app.image_hover = preview;
    if !same {
        app.image_hover_protocol = app.image_hover.as_ref().and_then(|hover| {
            terminal_graphics::session_graphics().encode_preview(&hover.data)
        });
    }
    true
}

pub(crate) fn clear_image_hover(app: &mut TuiApp) {
    app.image_hover = None;
    app.image_hover_protocol = None;
}
