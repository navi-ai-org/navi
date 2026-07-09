//! Terminal graphics for image hover previews (Kitty / Sixel / iTerm2).
//!
//! Detection runs once after entering the alternate screen. Halfblocks-only
//! terminals are treated as *unsupported* for image viewing — the UI keeps a
//! compact text metadata card instead.

use std::io::Cursor;
use std::sync::OnceLock;

use base64::Engine;
use image::ImageReader;
use ratatui::layout::Size;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;
use tracing::{debug, warn};

/// Graphics capabilities detected for the current terminal session.
#[derive(Debug)]
pub struct TerminalGraphics {
    picker: Option<Picker>,
    protocol: ProtocolType,
}

/// Decoded preview ready for a large lightbox (scales to the modal body).
pub struct EncodedPreview {
    pub protocol: StatefulProtocol,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

impl Default for TerminalGraphics {
    fn default() -> Self {
        Self {
            picker: None,
            protocol: ProtocolType::Halfblocks,
        }
    }
}

impl TerminalGraphics {
    /// Query the terminal for graphics protocol support.
    ///
    /// Must run after alternate-screen entry and before reading events
    /// (see `Picker::from_query_stdio` docs).
    pub fn detect() -> Self {
        match Picker::from_query_stdio() {
            Ok(picker) => {
                let protocol = picker.protocol_type();
                let supports = matches!(
                    protocol,
                    ProtocolType::Kitty | ProtocolType::Sixel | ProtocolType::Iterm2
                );
                let fs = picker.font_size();
                debug!(
                    ?protocol,
                    supports,
                    font_w = fs.width,
                    font_h = fs.height,
                    "terminal graphics capability"
                );
                Self {
                    picker: supports.then_some(picker),
                    protocol,
                }
            }
            Err(err) => {
                warn!(error = %err, "terminal graphics probe failed; text-only image hover");
                Self::default()
            }
        }
    }

    /// True when the terminal can show real pixel images (not halfblocks).
    pub fn supports_image_preview(&self) -> bool {
        self.picker.is_some()
            && matches!(
                self.protocol,
                ProtocolType::Kitty | ProtocolType::Sixel | ProtocolType::Iterm2
            )
    }

    pub fn protocol_label(&self) -> &'static str {
        match self.protocol {
            ProtocolType::Kitty => "kitty",
            ProtocolType::Sixel => "sixel",
            ProtocolType::Iterm2 => "iterm2",
            ProtocolType::Halfblocks => "none",
        }
    }

    /// Decode base64 into a stateful protocol that can scale into any modal body.
    pub fn encode_preview(&self, data_b64: &str) -> Option<EncodedPreview> {
        let picker = self.picker.as_ref()?;
        let bytes = decode_b64(data_b64)?;
        let reader = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        let dyn_img = reader.decode().ok()?;
        let pixel_width = dyn_img.width();
        let pixel_height = dyn_img.height();
        let protocol = picker.new_resize_protocol(dyn_img);
        Some(EncodedPreview {
            protocol,
            pixel_width,
            pixel_height,
        })
    }
}

fn decode_b64(data_b64: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(data_b64.as_bytes())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(data_b64.as_bytes()))
        .ok()
}

/// Process-wide probe result from the live TUI session (set once in `run`).
static SESSION_GRAPHICS: OnceLock<TerminalGraphics> = OnceLock::new();

pub fn install_session_graphics(graphics: TerminalGraphics) {
    let _ = SESSION_GRAPHICS.set(graphics);
}

pub fn session_graphics() -> &'static TerminalGraphics {
    SESSION_GRAPHICS.get_or_init(TerminalGraphics::default)
}

/// Decode pixel dimensions from base64 image data (for labels without graphics).
pub fn peek_image_dimensions(data_b64: &str) -> Option<(u32, u32)> {
    let bytes = decode_b64(data_b64)?;
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let dyn_img = reader.decode().ok()?;
    Some((dyn_img.width(), dyn_img.height()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_preview_rejects_garbage_without_graphics() {
        let gfx = TerminalGraphics::default();
        assert!(!gfx.supports_image_preview());
        assert!(gfx.encode_preview("not-valid-base64!!!").is_none());
    }

    #[test]
    fn peek_dimensions_reads_png() {
        use image::{ImageBuffer, ImageEncoder, Rgb};
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(12, 8, Rgb([10, 20, 30]));
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(img.as_raw(), 12, 8, image::ExtendedColorType::Rgb8)
            .expect("encode png");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        assert_eq!(peek_image_dimensions(&b64), Some((12, 8)));
    }

    #[test]
    fn lightbox_size_is_large_fraction_of_viewport() {
        let area = Size::new(100, 40);
        let (w, h) = lightbox_cells(area);
        assert!(w >= 80, "width {w}");
        assert!(h >= 28, "height {h}");
        assert!(w <= area.width);
        assert!(h <= area.height);
    }
}

/// Large lightbox size relative to the available content area.
pub fn lightbox_cells(area: Size) -> (u16, u16) {
    // ~92% width, ~80% height — big preview like the reference chrome, with
    // a little room left so the composer/footer stay visible underneath.
    let w = ((area.width as u32 * 92) / 100)
        .max(48)
        .min(area.width.saturating_sub(2).max(48) as u32) as u16;
    let h = ((area.height as u32 * 80) / 100)
        .max(16)
        .min(area.height.saturating_sub(3).max(16) as u32) as u16;
    (w, h)
}
