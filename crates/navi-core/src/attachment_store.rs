//! Durable content-addressed storage for multimodal attachments.
//!
//! Live `view_image` tool results strip base64 from [`AgentEvent::ToolCompleted`]
//! so session JSON stays small. Bytes are stored under
//! `{data_dir}/attachments/{sha256}.{ext}` and reloaded on session restore even
//! when the original project file was deleted or moved.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

/// Subdirectory of NAVI data dir holding attachment blobs.
pub const ATTACHMENTS_DIR: &str = "attachments";

/// Max blob size accepted into the store (same as live view_image / TUI paste).
pub const MAX_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;

/// Store raw bytes content-addressed by SHA-256.
///
/// Returns the attachment id: `{sha256_hex}.{ext}` (filename under attachments/).
pub fn store_bytes(data_dir: &Path, bytes: &[u8], ext: &str) -> Result<String> {
    if bytes.is_empty() {
        bail!("cannot store empty attachment");
    }
    if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
        bail!(
            "attachment too large ({} bytes); max is {MAX_ATTACHMENT_BYTES}",
            bytes.len()
        );
    }
    let ext = sanitize_ext(ext);
    let hash = hex::encode(Sha256::digest(bytes));
    let id = format!("{hash}.{ext}");
    let dir = data_dir.join(ATTACHMENTS_DIR);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create attachment dir {}", dir.display()))?;
    let path = dir.join(&id);
    if !path.exists() {
        // Write via temp + rename for crash safety.
        let tmp = dir.join(format!(".{id}.tmp"));
        fs::write(&tmp, bytes)
            .with_context(|| format!("failed to write attachment {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| {
            format!(
                "failed to finalize attachment {} → {}",
                tmp.display(),
                path.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
    }
    Ok(id)
}

/// Resolve a stored attachment path: `{data_dir}/attachments/{id}`.
pub fn attachment_path(data_dir: &Path, attachment_id: &str) -> Option<PathBuf> {
    let id = sanitize_attachment_id(attachment_id)?;
    Some(data_dir.join(ATTACHMENTS_DIR).join(id))
}

/// Load raw bytes for a stored attachment id.
pub fn load_bytes(data_dir: &Path, attachment_id: &str) -> Result<Vec<u8>> {
    let path = attachment_path(data_dir, attachment_id)
        .with_context(|| format!("invalid attachment id {attachment_id:?}"))?;
    if !path.is_file() {
        bail!("attachment not found: {}", path.display());
    }
    let meta = fs::metadata(&path)
        .with_context(|| format!("failed to stat attachment {}", path.display()))?;
    if meta.len() > MAX_ATTACHMENT_BYTES {
        bail!(
            "stored attachment exceeds size limit ({} bytes)",
            meta.len()
        );
    }
    fs::read(&path).with_context(|| format!("failed to read attachment {}", path.display()))
}

fn sanitize_ext(ext: &str) -> &'static str {
    match ext
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "png",
        "jpg" | "jpeg" => "jpg",
        "gif" => "gif",
        "webp" => "webp",
        "bmp" => "bmp",
        "svg" => "svg",
        "ico" => "ico",
        "tiff" | "tif" => "tiff",
        _ => "bin",
    }
}

/// Reject path traversal / weird ids. Valid: hex sha256 + `.` + ext.
fn sanitize_attachment_id(id: &str) -> Option<&str> {
    let id = id.trim();
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return None;
    }
    let (hash, ext) = id.rsplit_once('.')?;
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    // Only allowlisted extensions (unknown map to bin via sanitize_ext for store).
    let ext_ok = matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico" | "tiff" | "tif" | "bin"
    );
    ext_ok.then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"hello-image-bytes";
        let id = store_bytes(dir.path(), bytes, "png").unwrap();
        assert!(id.ends_with(".png"));
        assert_eq!(id.len(), 64 + 1 + 3);
        let loaded = load_bytes(dir.path(), &id).unwrap();
        assert_eq!(loaded, bytes);
        // Second store is a no-op (same content-address).
        let id2 = store_bytes(dir.path(), bytes, "png").unwrap();
        assert_eq!(id, id2);
    }

    #[test]
    fn rejects_path_traversal_ids() {
        assert!(attachment_path(Path::new("/tmp"), "../etc/passwd").is_none());
        assert!(attachment_path(Path::new("/tmp"), "abc/def.png").is_none());
        assert!(attachment_path(Path::new("/tmp"), "not-a-hash.png").is_none());
    }

    #[test]
    fn missing_attachment_errors() {
        let dir = tempfile::tempdir().unwrap();
        let fake = format!("{:0>64}.png", "a");
        assert!(load_bytes(dir.path(), &fake).is_err());
    }
}
