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
                debug!(?protocol, supports, "terminal graphics capability");
                Self {
                    // Only keep the picker when we will actually render images.
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

    #[allow(dead_code)]
    pub fn protocol_label(&self) -> &'static str {
        match self.protocol {
            ProtocolType::Kitty => "kitty",
            ProtocolType::Sixel => "sixel",
            ProtocolType::Iterm2 => "iterm2",
            ProtocolType::Halfblocks => "none",
        }
    }

    /// Decode base64 image bytes into a stateful protocol for `StatefulImage`.
    pub fn encode_preview(&self, data_b64: &str) -> Option<StatefulProtocol> {
        let picker = self.picker.as_ref()?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data_b64.as_bytes())
            .or_else(|_| {
                base64::engine::general_purpose::STANDARD_NO_PAD.decode(data_b64.as_bytes())
            })
            .ok()?;
        let reader = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        let dyn_img = reader.decode().ok()?;
        Some(picker.new_resize_protocol(dyn_img))
    }

    /// Cell size for a pixel image using the probed font metrics when available.
    pub fn estimate_cells(&self, image_w: Option<u32>, image_h: Option<u32>, max: Size) -> Size {
        let (cell_w, cell_h) = self
            .picker
            .as_ref()
            .map(|p| {
                let fs = p.font_size();
                (fs.width.max(1) as u32, fs.height.max(1) as u32)
            })
            .unwrap_or((10, 20));
        estimate_cell_size_with_font(image_w, image_h, max, cell_w, cell_h)
    }
}

/// Process-wide probe result from the live TUI session (set once in `run`).
static SESSION_GRAPHICS: OnceLock<TerminalGraphics> = OnceLock::new();

pub fn install_session_graphics(graphics: TerminalGraphics) {
    let _ = SESSION_GRAPHICS.set(graphics);
}

pub fn session_graphics() -> &'static TerminalGraphics {
    SESSION_GRAPHICS.get_or_init(TerminalGraphics::default)
}

/// Fixed cell size used when estimating modal dimensions for a pixel image.
#[cfg(test)]
fn estimate_cell_size(image_w: Option<u32>, image_h: Option<u32>, max: Size) -> Size {
    estimate_cell_size_with_font(image_w, image_h, max, 10, 20)
}

fn estimate_cell_size_with_font(
    image_w: Option<u32>,
    image_h: Option<u32>,
    max: Size,
    cell_w: u32,
    cell_h: u32,
) -> Size {
    let (pw, ph) = match (image_w, image_h) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
        _ => return Size::new(max.width.min(48), max.height.min(14)),
    };
    let cols = pw.div_ceil(cell_w.max(1)) as u16;
    let rows = ph.div_ceil(cell_h.max(1)) as u16;
    Size::new(
        cols.clamp(20, max.width.max(20)),
        rows.clamp(6, max.height.max(6)),
    )
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
    fn estimate_cells_clamps_to_max() {
        let size = estimate_cell_size(Some(4000), Some(3000), Size::new(40, 12));
        assert!(size.width <= 40);
        assert!(size.height <= 12);
    }
}
