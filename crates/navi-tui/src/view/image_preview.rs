use std::time::{Duration, Instant};

use ratatui::layout::{Alignment, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui_image::{Resize, StatefulImage};

use crate::app::TuiApp;
use crate::render::layout::opaque_fill;
use crate::render::markdown::{is_image_tag, parse_image_tag_index};
use crate::render::text::display_width;
use crate::state::ImageHoverPreview;
use crate::theme::*;
use crate::ui::interaction::HitAction;
use crate::view::terminal_graphics::{self, lightbox_cells};

/// Grace period after the cursor leaves the chip / lightbox before closing.
/// Covers the empty gap between an `[Image N]` chip and the centered modal so
/// the user can move onto the image without the preview vanishing mid-path.
pub(crate) const IMAGE_HOVER_CLOSE_GRACE: Duration = Duration::from_millis(180);

/// Floating hover modal for an `[Image N]` chip (composer or chat).
///
/// - **Kitty / Sixel / iTerm2:** large lightbox (~90%×80% of the content area)
///   with metadata title and the image scaled to fill the body.
/// - **Other terminals:** compact text-only metadata card.
///
/// Registers a high-z [`HitAction::ImageLightboxKeep`] over the modal so the
/// cursor can rest on the image without chat hits underneath clearing hover.
pub(crate) fn render_image_hover_modal(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let Some(preview) = app.image_hover.clone() else {
        app.image_hover_modal_rect = None;
        return;
    };
    if area.width < 20 || area.height < 3 {
        app.image_hover_modal_rect = None;
        return;
    }

    let gfx = terminal_graphics::session_graphics();
    let has_graphics = app.image_hover_protocol.is_some() && gfx.supports_image_preview();
    let header = if has_graphics {
        format!("{} · {}", preview.header_line(), gfx.protocol_label())
    } else {
        preview.header_line()
    };
    let fill = Style::default().bg(panel()).fg(text());

    let (modal_width, modal_height) = if has_graphics {
        lightbox_cells(ratatui::layout::Size::new(area.width, area.height))
    } else {
        let header_w = (display_width(&header) as u16).saturating_add(6);
        let w = header_w.clamp(28, area.width.saturating_sub(4).max(28));
        (w, 3.min(area.height))
    };

    let x = area
        .x
        .saturating_add(area.width.saturating_sub(modal_width) / 2);
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(modal_height) / 4); // slightly upper-center
    let modal = Rect {
        x,
        y,
        width: modal_width,
        height: modal_height,
    };
    app.image_hover_modal_rect = Some(modal);
    // z=100: above chat lines / chips so content under the modal cannot steal hover.
    app.register_hit(
        modal,
        100,
        "image_lightbox_keep",
        HitAction::ImageLightboxKeep,
    );

    // Solid underlay — Kitty skips cells; without this, chat bleeds through.
    opaque_fill(frame, modal, fill);

    let title = Line::from(vec![
        Span::styled(" ", fill),
        Span::styled(
            header,
            Style::default()
                .fg(text())
                .bg(panel())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", fill),
    ])
    .alignment(Alignment::Center);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ghost()).bg(panel()))
        .style(fill)
        .title(title)
        .title_alignment(Alignment::Center);
    let inner = block.inner(modal);
    frame.render_widget(block, modal);
    opaque_fill(frame, inner, fill);

    if let Some(protocol) = app.image_hover_protocol.as_mut() {
        // Scale image to fill the large body (Fit keeps aspect ratio).
        let image = StatefulImage::default().resize(Resize::Fit(None));
        frame.render_stateful_widget(image, inner, protocol);
    } else if inner.height > 0 {
        frame.render_widget(
            Paragraph::new("").style(fill).alignment(Alignment::Center),
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
                            // Above FocusComposer (z=100) so image chips remain clickable.
                            app.register_hit(
                                hit,
                                110,
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

/// Open or refresh the hover preview for an image chip action.
/// Returns `true` when the visible hover state changed (needs redraw).
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

    let same = match (&app.image_hover, &preview) {
        (Some(prev), Some(next)) => {
            prev.index == next.index && prev.size_bytes == next.size_bytes && prev.data == next.data
        }
        _ => false,
    };

    // Cursor is on a chip: cancel any pending leave-close.
    cancel_image_hover_close(app);
    app.image_hover = preview;
    if !same {
        app.image_hover_protocol = None;
        if let Some(hover) = app.image_hover.as_mut() {
            if (hover.width.is_none() || hover.height.is_none())
                && let Some((w, h)) = terminal_graphics::peek_image_dimensions(&hover.data)
            {
                hover.width = Some(w);
                hover.height = Some(h);
            }
            let gfx = terminal_graphics::session_graphics();
            if gfx.supports_image_preview()
                && let Some(encoded) = gfx.encode_preview(&hover.data)
            {
                hover.width = Some(encoded.pixel_width);
                hover.height = Some(encoded.pixel_height);
                app.image_hover_protocol = Some(encoded.protocol);
            }
        }
        return true;
    }
    false
}

/// Cursor is still inside the sticky zone (chip or lightbox body).
pub(crate) fn keep_image_hover(app: &mut TuiApp) {
    cancel_image_hover_close(app);
}

/// Cursor left chip + lightbox: schedule close after [`IMAGE_HOVER_CLOSE_GRACE`].
/// Returns `true` when a new deadline was armed (caller may not need a redraw).
pub(crate) fn schedule_image_hover_close(app: &mut TuiApp) -> bool {
    if app.image_hover.is_none() {
        return false;
    }
    if app.image_hover_close_deadline.is_some() {
        return false;
    }
    app.image_hover_close_deadline = Some(Instant::now() + IMAGE_HOVER_CLOSE_GRACE);
    false
}

pub(crate) fn cancel_image_hover_close(app: &mut TuiApp) {
    app.image_hover_close_deadline = None;
}

/// Apply a pending leave-close if the grace period elapsed. Returns true if closed.
pub(crate) fn poll_image_hover_close(app: &mut TuiApp) -> bool {
    let Some(deadline) = app.image_hover_close_deadline else {
        return false;
    };
    if Instant::now() < deadline {
        return false;
    }
    clear_image_hover(app);
    true
}

pub(crate) fn clear_image_hover(app: &mut TuiApp) {
    app.image_hover = None;
    app.image_hover_protocol = None;
    app.image_hover_modal_rect = None;
    app.image_hover_close_deadline = None;
}

/// Whether `action` is an image chip that should open the lightbox on hover.
pub(crate) fn is_image_chip_action(action: &HitAction) -> bool {
    matches!(
        action,
        HitAction::PreviewPendingImage(_) | HitAction::PreviewChatImage { .. }
    )
}
