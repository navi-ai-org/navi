use ratatui::layout::{Alignment, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::TuiApp;
use crate::render::markdown::{is_image_tag, parse_image_tag_index};
use crate::render::text::display_width;
use crate::state::ImageHoverPreview;
use crate::theme::*;
use crate::ui::interaction::HitAction;

/// Height of the legacy attachment strip (kept for layout callers that still reserve space).
#[allow(dead_code)]
pub(crate) const IMAGE_PREVIEW_HEIGHT: u16 = 0;

/// Composer strip is no longer used — images show as `[Image N]` chips in the input.
/// Kept as a no-op so existing layout call sites stay valid.
#[allow(dead_code)]
pub(crate) fn render_image_previews(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) -> Rect {
    let _ = (frame, app);
    Rect {
        height: 0,
        ..area
    }
}

/// Floating hover modal for an `[Image N]` chip (composer or chat).
pub(crate) fn render_image_hover_modal(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let Some(preview) = app.image_hover.as_ref() else {
        return;
    };
    if area.width < 24 || area.height < 6 {
        return;
    }

    let header = preview.header_line();
    let header_w = display_width(&header) as u16;
    let modal_width = header_w
        .saturating_add(6)
        .clamp(36, area.width.saturating_sub(4).max(24));
    // Compact card: header + thin body (placeholder frame for the image).
    let modal_height = 8u16.min(area.height.saturating_sub(2).max(5));
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
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ghost()).bg(panel()))
        .style(Style::default().bg(panel()).fg(text()))
        .title(Line::from(vec![
            Span::styled(" ", Style::default().bg(panel())),
            Span::styled(
                header,
                Style::default()
                    .fg(text())
                    .bg(panel())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default().bg(panel())),
        ]));
    let inner = block.inner(modal);
    frame.render_widget(block, modal);

    // Body: dim preview placeholder (true pixel rendering needs terminal image protocol).
    let dims = match (preview.width, preview.height) {
        (Some(w), Some(h)) => format!("{w}×{h}"),
        _ => "image".to_string(),
    };
    let body = format!(
        "\n  ▣  {} preview\n  hover target · {} · {}",
        preview.format_short(),
        dims,
        format_size(preview.size_bytes),
    );
    frame.render_widget(
        Paragraph::new(body)
            .style(Style::default().fg(muted()).bg(panel()))
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn format_size(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let n = bytes as f64;
    if n >= MB {
        format!("{:.1} MB", n / MB)
    } else if n >= KB {
        format!("{:.1} KB", n / KB)
    } else {
        format!("{bytes} B")
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
                        // Ensure the tag still maps into the full input (not a stale wrap).
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
    match action {
        HitAction::PreviewPendingImage(index) => {
            app.image_hover = app
                .pending_images
                .get(*index)
                .map(|image| ImageHoverPreview::from_pending(*index, image));
            true
        }
        HitAction::PreviewChatImage {
            message_index,
            image_index,
        } => {
            app.image_hover = app
                .messages
                .get(*message_index)
                .and_then(|message| message.images.get(*image_index))
                .map(ImageHoverPreview::from_chat);
            true
        }
        _ => false,
    }
}
