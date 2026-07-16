//! Rebuild model conversation history from persisted [`AgentEvent`] streams.
//!
//! Live turns keep multimodal tool payloads (e.g. `view_image`) on in-memory
//! [`ModelMessage::content_parts`]. Those bytes are intentionally stripped from
//! [`AgentEvent::ToolCompleted`] so session JSON stays small. On restore this
//! module re-attaches images by:
//! 1. re-reading the project path when still present, or
//! 2. loading a durable content-addressed blob from
//!    `{data_dir}/attachments/{attachment_id}` written at view time.

use crate::event::AgentEvent;
use crate::model::{ContentPart, ModelMessage};
use crate::tool::{ToolInvocation, ToolResult, take_tool_content_parts};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Same limit as TUI paste / live `view_image`.
const MAX_REHYDRATE_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

/// Rebuild provider-facing conversation messages from a session event log.
///
/// - User messages keep stored `content_parts` (pasted images).
/// - Tool results for `view_image` / `inspect_image` reload image bytes from
///   the project path or from `{data_dir}/attachments/{attachment_id}`.
/// - `project_root` resolves project-relative image paths.
/// - `data_dir` is NAVI's durable app data directory.
pub fn model_messages_from_agent_events(
    events: &[AgentEvent],
    project_root: Option<&Path>,
    data_dir: Option<&Path>,
) -> Vec<ModelMessage> {
    let mut messages = Vec::new();
    let mut pending_tool_calls: Vec<ToolInvocation> = Vec::new();
    let mut tool_names: HashMap<String, String> = HashMap::new();

    for event in events {
        match event {
            AgentEvent::UserTaskSubmitted {
                text,
                content_parts,
                submitted_at: _,
            } => {
                flush_pending_tool_calls(&mut messages, &mut pending_tool_calls);
                if content_parts.is_empty() {
                    messages.push(ModelMessage::user(text.clone()));
                } else {
                    messages.push(ModelMessage::user_multimodal(
                        text.clone(),
                        content_parts.clone(),
                    ));
                }
            }
            AgentEvent::ModelOutput { text, thinking } => {
                flush_pending_tool_calls(&mut messages, &mut pending_tool_calls);
                messages.push(ModelMessage::assistant_with_thinking(
                    text.clone(),
                    thinking.clone(),
                ));
            }
            AgentEvent::ToolRequested(invocation) => {
                tool_names.insert(invocation.id.clone(), invocation.tool_name.clone());
                pending_tool_calls.push(invocation.clone());
            }
            AgentEvent::ToolCompleted(result) => {
                flush_pending_tool_calls(&mut messages, &mut pending_tool_calls);
                let tool_name = tool_names
                    .get(&result.invocation_id)
                    .cloned()
                    .unwrap_or_else(|| "tool".to_string());
                let content = tool_result_text(result);
                let content_parts =
                    rehydrate_tool_content_parts(&tool_name, result, project_root, data_dir);
                messages.push(ModelMessage::tool_result_with_parts(
                    result.invocation_id.clone(),
                    tool_name,
                    content,
                    content_parts,
                ));
            }
            _ => {}
        }
    }
    // Orphan tool calls (interrupted turn) still surface as assistant requests.
    flush_pending_tool_calls(&mut messages, &mut pending_tool_calls);
    messages
}

fn flush_pending_tool_calls(messages: &mut Vec<ModelMessage>, pending: &mut Vec<ToolInvocation>) {
    if pending.is_empty() {
        return;
    }
    let calls = std::mem::take(pending);
    messages.push(ModelMessage::assistant_tool_calls_with_context(
        calls,
        String::new(),
        None,
    ));
}

fn tool_result_text(result: &ToolResult) -> String {
    result
        .output
        .as_str()
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            // Prefer compact JSON without internal multimodal payload keys.
            let mut output = result.output.clone();
            if let Some(obj) = output.as_object_mut() {
                obj.remove(crate::tool::NAVI_CONTENT_PARTS_KEY);
            }
            output.to_string()
        })
}

/// Re-attach multimodal parts for a persisted tool result.
///
/// Order:
/// 1. leftover `_navi_content_parts` (if present)
/// 2. project path (when the source file still exists)
/// 3. durable `{data_dir}/attachments/{attachment_id}` blob
pub fn rehydrate_tool_content_parts(
    tool_name: &str,
    result: &ToolResult,
    project_root: Option<&Path>,
    data_dir: Option<&Path>,
) -> Vec<ContentPart> {
    // Clone so we can run take_tool_content_parts without mutating the event log.
    let mut scratch = result.clone();
    let embedded = take_tool_content_parts(&mut scratch);
    if !embedded.is_empty() {
        return embedded;
    }

    if !matches!(tool_name, "view_image" | "inspect_image") {
        return Vec::new();
    }
    if !result.ok {
        return Vec::new();
    }
    let image_attached = result
        .output
        .get("image_attached")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !image_attached {
        return Vec::new();
    }

    let media_type = result
        .output
        .get("media_type")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            result
                .output
                .get("path")
                .and_then(|v| v.as_str())
                .and_then(media_type_from_path)
        })
        .unwrap_or_else(|| "application/octet-stream".to_string());

    // Prefer original path when still present (cheap, no data_dir needed).
    if let Some(path) = result.output.get("path").and_then(|v| v.as_str())
        && !path.is_empty()
    {
        let full_path = resolve_image_path(path, project_root);
        if let Some(part) = load_image_part(&full_path, &media_type) {
            return vec![part];
        }
    }

    // Fall back to durable content-addressed attachment.
    if let (Some(data_dir), Some(attachment_id)) = (
        data_dir,
        result.output.get("attachment_id").and_then(|v| v.as_str()),
    ) {
        match crate::attachment_store::load_bytes(data_dir, attachment_id) {
            Ok(bytes) if !bytes.is_empty() => {
                use base64::Engine;
                let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                return vec![ContentPart::Image { media_type, data }];
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(
                    %attachment_id,
                    error = %err,
                    tool = tool_name,
                    "could not load durable view_image attachment on session restore"
                );
            }
        }
    }

    tracing::warn!(
        tool = tool_name,
        path = ?result.output.get("path"),
        attachment_id = ?result.output.get("attachment_id"),
        "could not rehydrate view_image attachment on session restore"
    );
    Vec::new()
}

fn resolve_image_path(path: &str, project_root: Option<&Path>) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    if let Some(root) = project_root {
        return root.join(p);
    }
    p.to_path_buf()
}

fn media_type_from_path(path: &str) -> Option<String> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())?;
    Some(
        match ext.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "bmp" => "image/bmp",
            "svg" => "image/svg+xml",
            "ico" => "image/x-icon",
            "tiff" | "tif" => "image/tiff",
            _ => return None,
        }
        .to_string(),
    )
}

fn load_image_part(path: &Path, media_type: &str) -> Option<ContentPart> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_REHYDRATE_IMAGE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(ContentPart::Image {
        media_type: media_type.to_string(),
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolInvocation;
    use serde_json::json;
    use std::fs;

    fn minimal_png() -> Vec<u8> {
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0x60, 0x60, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE5, 0x27, 0xDE, 0xFC,
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ]
    }

    #[test]
    fn rebuilds_user_multimodal_and_view_image_tool_parts() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let img_path = dir.path().join("shot.png");
        fs::write(&img_path, minimal_png()).unwrap();
        let attachment_id =
            crate::attachment_store::store_bytes(data_dir.path(), &minimal_png(), "png").unwrap();

        let events = vec![
            AgentEvent::UserTaskSubmitted {
                text: "look".into(),
                content_parts: vec![ContentPart::Image {
                    media_type: "image/png".into(),
                    data: "userpaste".into(),
                }],
                submitted_at: None,
            },
            AgentEvent::ToolRequested(ToolInvocation {
                id: "c1".into(),
                tool_name: "view_image".into(),
                input: json!({ "path": "shot.png" }),
            }),
            AgentEvent::ToolCompleted(ToolResult {
                invocation_id: "c1".into(),
                ok: true,
                output: json!({
                    "path": "shot.png",
                    "format": "png",
                    "media_type": "image/png",
                    "size_bytes": minimal_png().len(),
                    "image_attached": true,
                    "attachment_id": attachment_id,
                    "message": "Image attached for multimodal analysis on the next model request.",
                }),
            }),
            AgentEvent::ModelOutput {
                text: "I see a pixel.".into(),
                thinking: None,
            },
        ];

        let messages =
            model_messages_from_agent_events(&events, Some(dir.path()), Some(data_dir.path()));
        assert_eq!(messages.len(), 4);
        assert!(messages[0].content_parts.iter().any(|p| p.is_image()));
        assert_eq!(messages[1].tool_calls.len(), 1);
        assert_eq!(messages[1].tool_calls[0].tool_name, "view_image");
        assert_eq!(messages[2].role, crate::model::ModelRole::Tool);
        assert!(
            messages[2].content_parts.iter().any(|p| p.is_image()),
            "view_image must rehydrate image bytes on restore"
        );
        assert!(
            messages[2]
                .content_parts
                .iter()
                .filter_map(|p| p.data())
                .any(|d| d != "userpaste" && !d.is_empty()),
            "rehydrated data should come from disk, not user paste"
        );
        assert_eq!(messages[3].content, "I see a pixel.");
    }

    #[test]
    fn rehydrates_from_attachment_store_when_source_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let attachment_id =
            crate::attachment_store::store_bytes(data_dir.path(), &minimal_png(), "png").unwrap();

        let result = ToolResult {
            invocation_id: "c1".into(),
            ok: true,
            output: json!({
                "path": "deleted.png",
                "media_type": "image/png",
                "image_attached": true,
                "attachment_id": attachment_id,
            }),
        };
        // Source path does not exist.
        let parts = rehydrate_tool_content_parts(
            "view_image",
            &result,
            Some(dir.path()),
            Some(data_dir.path()),
        );
        assert_eq!(parts.len(), 1);
        assert!(parts[0].is_image());
        assert_eq!(parts[0].media_type(), Some("image/png"));
        assert!(!parts[0].data().unwrap_or("").is_empty());
    }

    #[test]
    fn view_image_without_file_or_store_does_not_panic() {
        let events = vec![
            AgentEvent::ToolRequested(ToolInvocation {
                id: "c1".into(),
                tool_name: "view_image".into(),
                input: json!({ "path": "missing.png" }),
            }),
            AgentEvent::ToolCompleted(ToolResult {
                invocation_id: "c1".into(),
                ok: true,
                output: json!({
                    "path": "missing.png",
                    "image_attached": true,
                    "media_type": "image/png",
                }),
            }),
        ];
        let messages = model_messages_from_agent_events(&events, None, None);
        assert_eq!(messages.len(), 2);
        assert!(messages[1].content_parts.is_empty());
    }

    #[test]
    fn batches_parallel_tool_requests() {
        let events = vec![
            AgentEvent::ToolRequested(ToolInvocation {
                id: "a".into(),
                tool_name: "read_file".into(),
                input: json!({ "path": "a.rs" }),
            }),
            AgentEvent::ToolRequested(ToolInvocation {
                id: "b".into(),
                tool_name: "read_file".into(),
                input: json!({ "path": "b.rs" }),
            }),
            AgentEvent::ToolCompleted(ToolResult {
                invocation_id: "a".into(),
                ok: true,
                output: json!({ "content": "a" }),
            }),
            AgentEvent::ToolCompleted(ToolResult {
                invocation_id: "b".into(),
                ok: true,
                output: json!({ "content": "b" }),
            }),
        ];
        let messages = model_messages_from_agent_events(&events, None, None);
        // one assistant with 2 calls + two tool results
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].tool_calls.len(), 2);
    }
}
