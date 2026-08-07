//! Screen capture via `CGDisplayCreateImage`.
//!
//! Captures the primary display to a PNG file. Requires Screen Recording
//! permission (System Settings → Privacy & Security → Screen Recording).

use anyhow::{Result, anyhow};
use std::io::Cursor;
use std::path::Path;

use crate::{MacRect, MacScreenshot};

/// Captures the primary monitor screen and saves it as a PNG file.
pub fn capture_screen(out_dir: &str) -> Result<MacScreenshot> {
    use core_graphics::display::CGDisplay;
    use core_graphics::image::CGImageRef;
    use image::ImageEncoder;

    let display = CGDisplay::main();
    let cg_image = display
        .image()
        .ok_or_else(|| anyhow!("CGDisplayCreateImage failed"))?;

    let width = cg_image.width();
    let height = cg_image.height();

    // Convert CGImage to RGBA pixel buffer.
    let bytes_per_row = cg_image.bytes_per_row();
    let bits_per_pixel = cg_image.bits_per_pixel();
    let raw_data = cg_image.data();

    // CGImage data may have padding at the end of each row. We need to
    // copy it into a tightly-packed buffer for the image crate.
    let expected_bpp = (bits_per_pixel + 7) / 8;
    let mut packed = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height as usize {
        let row_start = y * bytes_per_row;
        for x in 0..width as usize {
            let offset = row_start + x * expected_bpp;
            if offset + 4 <= raw_data.len() {
                // CGImage uses BGRA on macOS; convert to RGBA for the image crate.
                let b = raw_data[offset];
                let g = raw_data[offset + 1];
                let r = raw_data[offset + 2];
                let a = if expected_bpp == 4 {
                    raw_data[offset + 3]
                } else {
                    255
                };
                packed.extend_from_slice(&[r, g, b, a]);
            } else {
                packed.extend_from_slice(&[0, 0, 0, 255]);
            }
        }
    }

    // Encode as PNG.
    let mut png_bytes = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut png_bytes);
    encoder
        .write_image(&packed, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| anyhow!("PNG encode failed: {e}"))?;

    // Write to file.
    let dir = Path::new(out_dir);
    std::fs::create_dir_all(dir).map_err(|e| anyhow!("failed to create screenshot dir: {e}"))?;

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let filename = format!("screenshot_{timestamp}.png");
    let path = dir.join(&filename);
    std::fs::write(&path, &png_bytes).map_err(|e| anyhow!("failed to write screenshot: {e}"))?;

    let size_bytes = png_bytes.len() as u64;
    let path_str = path.to_string_lossy().to_string();

    tracing::info!(
        width, height, size_bytes, path = %path_str,
        "macOS screen capture complete"
    );

    Ok(MacScreenshot {
        path: path_str,
        width,
        height,
        size_bytes,
    })
}

/// Returns the bounding rect of the main display (screen coordinates).
#[allow(dead_code)]
pub fn main_display_rect() -> Result<MacRect> {
    use core_graphics::display::CGDisplay;
    let display = CGDisplay::main();
    let bounds = display.bounds();
    Ok(MacRect {
        x: bounds.origin.x as i32,
        y: bounds.origin.y as i32,
        width: bounds.size.width as i32,
        height: bounds.size.height as i32,
    })
}
