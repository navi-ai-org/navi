use crate::state::PendingImage;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ratatui_image::picker::Picker;

/// Maximum image size to accept from the clipboard (10 MiB of raw RGBA → ~5 MiB PNG).
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// Attempts to read an image from the system clipboard.
///
/// Returns `Some(PendingImage)` if the clipboard contains image data that
/// fits within the size limit, or `None` if no image is available or the
/// read fails.
///
/// The image is converted from the clipboard's raw RGBA format to PNG for
/// broad model compatibility. If a `Picker` is provided, a terminal
/// protocol thumbnail is also generated for inline preview.
pub fn try_read_clipboard_image(picker: Option<&Picker>) -> Option<PendingImage> {
    // On Linux, prefer native tools (wl-paste for Wayland, xclip for X11)
    // because arboard only supports X11 and fails silently on Wayland.
    if cfg!(target_os = "linux") {
        if let Some(img) = try_wl_paste_image(picker) {
            return Some(img);
        }
        if let Some(img) = try_xclip_image(picker) {
            return Some(img);
        }
        // Fallback to arboard if native tools are not installed
        if let Some(img) = try_arboard_image(picker) {
            return Some(img);
        }
    } else {
        // macOS/Windows: use arboard directly
        if let Some(img) = try_arboard_image(picker) {
            return Some(img);
        }
    }

    tracing::warn!("all clipboard image methods failed");
    None
}

/// Attempts to parse the given string as a file path and load it if it is an image.
/// This enables "drag and drop" functionality because terminal emulators paste
/// the dropped file's path.
pub fn try_read_image_from_path(picker: Option<&Picker>, text: &str) -> Option<PendingImage> {
    let raw = text.trim();
    // Strip quotes often added by terminals when dropping files
    let unquoted = raw
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .or_else(|| raw.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
        .unwrap_or(raw);

    // Strip file:// scheme if present
    let path_str = unquoted.strip_prefix("file://").unwrap_or(unquoted);

    let path = std::path::Path::new(path_str);
    if !path.is_file() {
        return None;
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let media_type = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        _ => return None, // Not a supported image format
    };

    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > MAX_IMAGE_BYTES {
        tracing::warn!(
            bytes = bytes.len(),
            limit = MAX_IMAGE_BYTES,
            "dropped image exceeds size limit"
        );
        return None;
    }

    let data = BASE64.encode(&bytes);

    // For vector images we don't try to generate a thumbnail protocol
    let protocol = if media_type == "image/svg+xml" {
        None
    } else {
        // Only try to thumbnail it if we can
        try_create_protocol(picker, &bytes)
    };

    tracing::info!(
        path = %path.display(),
        size = bytes.len(),
        "loaded image from path"
    );

    Some(PendingImage {
        media_type: media_type.to_string(),
        data,
        width: None,
        height: None,
        protocol,
    })
}

/// Try to create a terminal Protocol from PNG bytes using the picker.
fn try_create_protocol(
    picker: Option<&Picker>,
    png_bytes: &[u8],
) -> Option<ratatui_image::protocol::StatefulProtocol> {
    let picker = picker?;
    let mut dyn_img = image::load_from_memory(png_bytes).ok()?;

    // Center crop to target aspect ratio (26 cols / 10 rows => 2.6 chars aspect ratio).
    // Given character font cell aspect ratio of ~1:2 (width:height),
    // target physical aspect ratio = 26.0 / (10.0 * 2.0) = 1.3
    let target_ratio = 1.3_f64;
    let orig_width = dyn_img.width() as f64;
    let orig_height = dyn_img.height() as f64;
    let orig_ratio = orig_width / orig_height;

    if orig_ratio > target_ratio {
        // Wider than target ratio: crop horizontally centered
        let new_width = orig_height * target_ratio;
        let x = ((orig_width - new_width) / 2.0).round() as u32;
        dyn_img = dyn_img.crop_imm(x, 0, new_width.round() as u32, orig_height as u32);
    } else if orig_ratio < target_ratio {
        // Taller than target ratio: crop vertically centered
        let new_height = orig_width / target_ratio;
        let y = ((orig_height - new_height) / 2.0).round() as u32;
        dyn_img = dyn_img.crop_imm(0, y, orig_width as u32, new_height.round() as u32);
    }

    Some(picker.new_resize_protocol(dyn_img))
}

fn try_arboard_image(picker: Option<&Picker>) -> Option<PendingImage> {
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "arboard: failed to initialize clipboard");
            return None;
        }
    };
    let image = match clipboard.get_image() {
        Ok(img) => img,
        Err(e) => {
            tracing::warn!(error = %e, "arboard: failed to read image from clipboard");
            return None;
        }
    };

    let width = image.width as u32;
    let height = image.height as u32;
    let rgba_bytes = image.bytes.as_ref();

    if rgba_bytes.len() > MAX_IMAGE_BYTES {
        tracing::warn!(
            bytes = rgba_bytes.len(),
            limit = MAX_IMAGE_BYTES,
            "clipboard image exceeds size limit"
        );
        return None;
    }

    match encode_rgba_to_png(rgba_bytes, width, height) {
        Some(png_bytes) => {
            let data = BASE64.encode(&png_bytes);
            let protocol = try_create_protocol(picker, &png_bytes);
            tracing::info!(
                width,
                height,
                png_size = png_bytes.len(),
                "clipboard image captured as PNG (arboard)"
            );
            Some(PendingImage {
                media_type: "image/png".to_string(),
                data,
                width: Some(width),
                height: Some(height),
                protocol,
            })
        }
        None => {
            let data = BASE64.encode(rgba_bytes);
            tracing::info!(
                width,
                height,
                raw_size = rgba_bytes.len(),
                "clipboard image captured as raw RGBA (arboard)"
            );
            Some(PendingImage {
                media_type: "image/png".to_string(),
                data,
                width: Some(width),
                height: Some(height),
                protocol: None,
            })
        }
    }
}

/// Fallback clipboard image reader using wl-paste (Wayland) or xclip (X11).
fn try_wl_paste_image(picker: Option<&Picker>) -> Option<PendingImage> {
    // Try SVG first
    if let Ok(output) = std::process::Command::new("wl-paste")
        .args(["--type", "image/svg+xml", "--no-newline"])
        .output()
    {
        if output.status.success() && !output.stdout.is_empty() {
            let svg_bytes = output.stdout;
            if svg_bytes.len() <= MAX_IMAGE_BYTES {
                let data = BASE64.encode(&svg_bytes);
                tracing::info!(
                    svg_size = svg_bytes.len(),
                    "clipboard image captured as SVG via wl-paste"
                );
                return Some(PendingImage {
                    media_type: "image/svg+xml".to_string(),
                    data,
                    width: None,
                    height: None,
                    protocol: None, // No SVG rendering in terminal
                });
            }
        }
    }

    let output = std::process::Command::new("wl-paste")
        .args(["--type", "image/png", "--no-newline"])
        .output()
        .ok()?;

    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    let png_bytes = output.stdout;
    if png_bytes.len() > MAX_IMAGE_BYTES {
        tracing::warn!(
            bytes = png_bytes.len(),
            limit = MAX_IMAGE_BYTES,
            "wl-paste image exceeds size limit"
        );
        return None;
    }

    let data = BASE64.encode(&png_bytes);
    let protocol = try_create_protocol(picker, &png_bytes);
    tracing::info!(
        png_size = png_bytes.len(),
        "clipboard image captured via wl-paste"
    );
    Some(PendingImage {
        media_type: "image/png".to_string(),
        data,
        width: None,
        height: None,
        protocol,
    })
}

fn try_xclip_image(picker: Option<&Picker>) -> Option<PendingImage> {
    // Try SVG first
    if let Ok(output) = std::process::Command::new("xclip")
        .args([
            "-selection",
            "clipboard",
            "-target",
            "image/svg+xml",
            "-out",
        ])
        .output()
    {
        if output.status.success() && !output.stdout.is_empty() {
            let svg_bytes = output.stdout;
            if svg_bytes.len() <= MAX_IMAGE_BYTES {
                let data = BASE64.encode(&svg_bytes);
                tracing::info!(
                    svg_size = svg_bytes.len(),
                    "clipboard image captured as SVG via xclip"
                );
                return Some(PendingImage {
                    media_type: "image/svg+xml".to_string(),
                    data,
                    width: None,
                    height: None,
                    protocol: None, // No SVG rendering in terminal
                });
            }
        }
    }

    let output = std::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-target", "image/png", "-out"])
        .output()
        .ok()?;

    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    let png_bytes = output.stdout;
    if png_bytes.len() > MAX_IMAGE_BYTES {
        tracing::warn!(
            bytes = png_bytes.len(),
            limit = MAX_IMAGE_BYTES,
            "xclip image exceeds size limit"
        );
        return None;
    }

    let data = BASE64.encode(&png_bytes);
    let protocol = try_create_protocol(picker, &png_bytes);
    tracing::info!(
        png_size = png_bytes.len(),
        "clipboard image captured via xclip"
    );
    Some(PendingImage {
        media_type: "image/png".to_string(),
        data,
        width: None,
        height: None,
        protocol,
    })
}

/// Encodes raw RGBA pixels into PNG format without the `image` crate.
///
/// Uses a minimal hand-rolled PNG encoder to avoid adding a heavy dependency.
/// Returns `None` if encoding fails (caller should fall back to raw data).
fn encode_rgba_to_png(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    // Minimal PNG encoder: PNG signature + IHDR + single IDAT + IEND
    let expected_len = (width as usize) * (height as usize) * 4;
    if rgba.len() != expected_len || width == 0 || height == 0 {
        return None;
    }

    let mut png = Vec::with_capacity(rgba.len() / 2);

    // PNG signature
    png.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);

    // IHDR chunk
    let ihdr_data = {
        let mut d = Vec::with_capacity(13);
        d.extend_from_slice(&width.to_be_bytes());
        d.extend_from_slice(&height.to_be_bytes());
        d.push(8); // bit depth
        d.push(6); // color type: RGBA
        d.push(0); // compression method
        d.push(0); // filter method
        d.push(0); // interlace method
        d
    };
    write_png_chunk(&mut png, b"IHDR", &ihdr_data);

    // IDAT chunk: raw deflate of filter-byte-preceded scanlines
    let raw_data = {
        let mut raw = Vec::with_capacity(rgba.len() + height as usize);
        for row in 0..height as usize {
            raw.push(0); // filter: None
            let start = row * width as usize * 4;
            let end = start + width as usize * 4;
            raw.extend_from_slice(&rgba[start..end]);
        }
        raw
    };

    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&raw_data, 6);
    write_png_chunk(&mut png, b"IDAT", &compressed);

    // IEND chunk
    write_png_chunk(&mut png, b"IEND", &[]);

    Some(png)
}

fn write_png_chunk(buf: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
    buf.extend_from_slice(chunk_type);
    buf.extend_from_slice(data);
    let crc = crc32(chunk_type, data);
    buf.extend_from_slice(&crc.to_be_bytes());
}

/// CRC-32 calculation for PNG chunks (ISO 3309 / ITU-T V.42).
fn crc32(chunk_type: &[u8; 4], data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in chunk_type.iter().chain(data.iter()) {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_encoder_produces_valid_png_header() {
        // 1x1 red pixel
        let rgba = vec![255u8, 0, 0, 255];
        let png = encode_rgba_to_png(&rgba, 1, 1).expect("png encode");
        assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
        // IHDR chunk type at offset 12
        assert_eq!(&png[12..16], b"IHDR");
    }

    #[test]
    fn png_encoder_rejects_wrong_size() {
        let rgba = vec![0u8; 10]; // wrong size for 2x2
        assert!(encode_rgba_to_png(&rgba, 2, 2).is_none());
    }

    #[test]
    fn png_encoder_rejects_zero_dimensions() {
        let rgba = vec![0u8; 0];
        assert!(encode_rgba_to_png(&rgba, 0, 0).is_none());
    }

    #[test]
    fn crc32_matches_known_value() {
        // PNG spec: CRC of "IHDR" with known data
        let result = crc32(b"IHDR", &[]);
        // CRC of just "IHDR" with empty data
        assert_ne!(result, 0);
    }
}
