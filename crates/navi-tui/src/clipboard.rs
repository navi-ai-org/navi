use crate::state::PendingImage;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use std::process::Command;

/// Maximum image size to accept from clipboard/path sources.
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// Attempts to read an image from the system clipboard.
///
/// Linux uses native clipboard tools only:
/// - Wayland: `wl-paste`
/// - X11: `xclip`
pub fn try_read_clipboard_image() -> Option<PendingImage> {
    if !cfg!(target_os = "linux") {
        tracing::warn!("clipboard image capture requires native Linux clipboard tools");
        return None;
    }

    let session = ClipboardSession::detect()?;
    match session {
        ClipboardSession::Wayland => try_wl_paste_image(),
        ClipboardSession::X11 => try_xclip_image(),
    }
}

/// Reads plain text from the system clipboard (Wayland `wl-paste` / X11 `xclip`).
pub fn try_read_clipboard_text() -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
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

    Some(PendingImage {
        media_type: media_type.to_string(),
        data: BASE64.encode(&bytes),
        width: None,
        height: None,
    })
}

#[derive(Debug, Clone, Copy)]
enum ClipboardSession {
    Wayland,
    X11,
}

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

    Some(PendingImage {
        media_type: media_type.to_string(),
        data: BASE64.encode(&bytes),
        width: None,
        height: None,
    })
}
