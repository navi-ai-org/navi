use crate::state::PendingImage;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
#[cfg(target_os = "linux")]
use std::process::Command;

/// Maximum image size to accept from clipboard/path sources.
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// Attempts to read an image from the system clipboard.
///
/// - Linux: native clipboard tools (`wl-paste` / `xclip`).
/// - Windows: `CF_DIBV5` then `CF_DIB` via the Win32 clipboard API, converted
///   to PNG. Windows Terminal does not support inline image protocols
///   (Kitty/Sixel/iTerm2), but the image is still attached to the message as
///   base64 for the model, so paste is useful without inline rendering.
pub fn try_read_clipboard_image() -> Option<PendingImage> {
    #[cfg(target_os = "linux")]
    {
        let session = ClipboardSession::detect()?;
        match session {
            ClipboardSession::Wayland => try_wl_paste_image(),
            ClipboardSession::X11 => try_xclip_image(),
        }
    }
    #[cfg(target_os = "windows")]
    {
        try_win_clipboard_image()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

/// Reads plain text from the system clipboard.
///
/// - Linux: `wl-paste` / `xclip`.
/// - Windows: Win32 clipboard (`CF_UNICODETEXT`).
pub fn try_read_clipboard_text() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let session = ClipboardSession::detect()?;
        let output = match session {
            ClipboardSession::Wayland => Command::new("wl-paste")
                .args(["--type", "text/plain", "--no-newline"])
                .output()
                .ok()?,
            ClipboardSession::X11 => Command::new("xclip")
                .args(["-selection", "clipboard", "-o"])
                .output()
                .ok()?,
        };
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }
    #[cfg(target_os = "windows")]
    {
        use clipboard_win::get_clipboard_string;
        match get_clipboard_string() {
            Ok(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            Err(_) => None,
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

/// Attempts to parse the given string as a file path and load it if it is an image.
/// This enables "drag and drop" functionality because terminal emulators paste
/// the dropped file's path.
pub fn try_read_image_from_path(text: &str) -> Option<PendingImage> {
    let raw = text.trim();
    let unquoted = raw
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .or_else(|| raw.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
        .unwrap_or(raw);

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
        _ => return None,
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

    tracing::info!(
        path = %path.display(),
        size = bytes.len(),
        "loaded image from path"
    );

    Some(pending_image(media_type, &bytes))
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
enum ClipboardSession {
    Wayland,
    X11,
}

#[cfg(target_os = "linux")]
impl ClipboardSession {
    fn detect() -> Option<Self> {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            return Some(Self::Wayland);
        }
        if std::env::var_os("DISPLAY").is_some() {
            return Some(Self::X11);
        }
        tracing::warn!("neither WAYLAND_DISPLAY nor DISPLAY is set; cannot pick clipboard tool");
        None
    }
}

#[cfg(target_os = "linux")]
fn try_wl_paste_image() -> Option<PendingImage> {
    try_clipboard_command(
        "wl-paste",
        &["--type", "image/svg+xml", "--no-newline"],
        "image/svg+xml",
    )
    .or_else(|| {
        try_clipboard_command(
            "wl-paste",
            &["--type", "image/png", "--no-newline"],
            "image/png",
        )
    })
}

#[cfg(target_os = "linux")]
fn try_xclip_image() -> Option<PendingImage> {
    try_clipboard_command(
        "xclip",
        &[
            "-selection",
            "clipboard",
            "-target",
            "image/svg+xml",
            "-out",
        ],
        "image/svg+xml",
    )
    .or_else(|| {
        try_clipboard_command(
            "xclip",
            &["-selection", "clipboard", "-target", "image/png", "-out"],
            "image/png",
        )
    })
}

#[cfg(target_os = "linux")]
fn try_clipboard_command(program: &str, args: &[&str], media_type: &str) -> Option<PendingImage> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    let bytes = output.stdout;
    if bytes.len() > MAX_IMAGE_BYTES {
        tracing::warn!(
            program,
            bytes = bytes.len(),
            limit = MAX_IMAGE_BYTES,
            "clipboard image exceeds size limit"
        );
        return None;
    }

    tracing::info!(
        program,
        media_type,
        size = bytes.len(),
        "clipboard image captured"
    );

    Some(pending_image(media_type, &bytes))
}

fn pending_image(media_type: &str, bytes: &[u8]) -> PendingImage {
    let data = BASE64.encode(bytes);
    let (width, height) = crate::view::terminal_graphics::peek_image_dimensions(&data)
        .map(|(w, h)| (Some(w), Some(h)))
        .unwrap_or((None, None));
    PendingImage {
        media_type: media_type.to_string(),
        data,
        width,
        height,
    }
}

// ─── Windows clipboard image (CF_DIB / CF_DIBV5 → PNG) ──────────────────────

#[cfg(target_os = "windows")]
fn try_win_clipboard_image() -> Option<PendingImage> {
    use clipboard_win::{formats, get};

    // CF_DIBV5 carries richer color-space metadata; CF_DIB is the common
    // fallback. Both are a BITMAPINFO/BITMAPV5HEADER followed by palette + bits.
    let dib: Vec<u8> = get::<Vec<u8>, _>(formats::RawData(formats::CF_DIBV5))
        .or_else(|_| get::<Vec<u8>, _>(formats::RawData(formats::CF_DIB)))
        .ok()?;

    if dib.is_empty() {
        return None;
    }
    if dib.len() > MAX_IMAGE_BYTES {
        tracing::warn!(
            bytes = dib.len(),
            limit = MAX_IMAGE_BYTES,
            "clipboard image exceeds size limit"
        );
        return None;
    }

    let png = dib_to_png(&dib)?;
    tracing::info!(
        dib_bytes = dib.len(),
        png_bytes = png.len(),
        "clipboard image captured (CF_DIB → PNG)"
    );
    Some(pending_image("image/png", &png))
}

/// Convert a Win32 `CF_DIB` payload (BITMAPINFO + palette + pixels) into PNG.
///
/// `CF_DIB` is a BMP without the 14-byte `BITMAPFILEHEADER`. We synthesize the
/// header (magic `BM`, file size, pixel-data offset derived from the DIB header
/// size + palette) so the `image` crate can decode it, then re-encode as PNG.
#[cfg(target_os = "windows")]
fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    use image::ImageDecoder as _;

    // BITMAPINFOHEADER starts at offset 0; biSize (u32 LE) is the header size
    // (40 for BITMAPINFOHEADER, 108/124 for V4/V5). biClrUsed is at +32.
    if dib.len() < 40 {
        tracing::warn!(
            len = dib.len(),
            "CF_DIB payload too small for BITMAPINFOHEADER"
        );
        return None;
    }
    let header_size = u32::from_le_bytes([dib[0], dib[1], dib[2], dib[3]]) as usize;
    if header_size < 40 || header_size > dib.len() {
        tracing::warn!(header_size, "CF_DIB header size out of range");
        return None;
    }
    let clr_used = u32::from_le_bytes([dib[32], dib[33], dib[34], dib[35]]) as usize;
    // Palette size: biClrUsed entries × 4 bytes; if zero, infer from bit depth.
    let bit_count = u16::from_le_bytes([dib[14], dib[15]]) as usize;
    let palette_entries = if clr_used != 0 {
        clr_used
    } else if bit_count <= 8 {
        1usize << bit_count
    } else {
        0
    };
    let palette_size = palette_entries * 4;
    let pixel_offset = 14 + header_size + palette_size;

    // BITMAPFILEHEADER: 14 bytes.
    let mut bmp = Vec::with_capacity(14 + dib.len());
    bmp.extend_from_slice(b"BM"); // bfType
    let file_size = (14 + dib.len()) as u32;
    bmp.extend_from_slice(&file_size.to_le_bytes()); // bfSize
    bmp.extend_from_slice(&[0, 0, 0, 0]); // bfReserved1 + bfReserved2
    bmp.extend_from_slice(&(pixel_offset as u32).to_le_bytes()); // bfOffBits
    bmp.extend_from_slice(dib);

    let decoder = match image::codecs::bmp::BmpDecoder::new(std::io::Cursor::new(&bmp)) {
        Ok(d) => d,
        Err(err) => {
            tracing::warn!(error = %err, "failed to decode CF_DIB as BMP");
            return None;
        }
    };
    let (w, h) = decoder.dimensions();
    let mut rgba = vec![0u8; (w as usize) * (h as usize) * 4];
    if let Err(err) = decoder.read_image(&mut rgba) {
        tracing::warn!(error = %err, "failed to read CF_DIB pixels");
        return None;
    }
    // BMP is bottom-up by default; flip to top-down for PNG.
    let row = w as usize * 4;
    let mut top_down = Vec::with_capacity(rgba.len());
    for y in (0..h as usize).rev() {
        top_down.extend_from_slice(&rgba[y * row..(y + 1) * row]);
    }
    let img = image::RgbaImage::from_raw(w, h, top_down)?;
    let mut png = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut png);
    if let Err(err) =
        image::ImageEncoder::write_image(encoder, &img, w, h, image::ExtendedColorType::Rgba8)
    {
        tracing::warn!(error = %err, "failed to encode PNG from CF_DIB");
        return None;
    }
    Some(png)
}
