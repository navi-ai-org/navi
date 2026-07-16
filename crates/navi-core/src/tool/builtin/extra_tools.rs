use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::helpers;
use crate::tool::registry::ToolRegistry;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolMetadata, ToolResult};

// ── CurrentTimeTool ────────────────────────────────────────────────────────

pub(crate) struct CurrentTimeTool;

impl CurrentTimeTool {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for CurrentTimeTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "current_time",
            "Get the current UTC date/time and Unix timestamp. Use this to determine the current date and time.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch");
        let unix_secs = now.as_secs();

        // Format as ISO 8601 UTC.
        let secs = unix_secs % 86400;
        let days = unix_secs / 86400;
        // Simple Gregorian date calculation from days since epoch.
        let (year, month, day) = days_to_date(days);
        let hours = secs / 3600;
        let minutes = (secs % 3600) / 60;
        let seconds = secs % 60;

        let utc_iso = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            year, month, day, hours, minutes, seconds
        );

        Ok(helpers::ok(
            invocation.id,
            json!({
                "utc_iso": utc_iso,
                "unix_timestamp_seconds": unix_secs,
                "timezone": "UTC",
            }),
        ))
    }
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_date(days: u64) -> (u64, u64, u64) {
    // Algorithm from Howard Hinnant.
    let z = days + 719468;
    let era = z / 146097;
    let doe = z % 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + 3;
    let (y, m) = if m > 12 { (y + 1, m - 12) } else { (y, m) };
    (y, m, d)
}

// ── SleepTool ──────────────────────────────────────────────────────────────

pub(crate) struct SleepTool;

impl SleepTool {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SleepTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "sleep",
            "Sleep (delay) for a specified number of seconds. Use this to wait before retrying an operation or to introduce a deliberate pause.",
            ToolKind::Command,
            json!({
                "type": "object",
                "properties": {
                    "seconds": {
                        "type": "integer",
                        "description": "Number of seconds to sleep.",
                        "minimum": 1,
                        "maximum": 300
                    }
                },
                "required": ["seconds"],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let seconds = helpers::optional_u64(&invocation.input, "seconds")
            .unwrap_or(1)
            .max(1)
            .min(300);

        tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;

        Ok(helpers::ok(
            invocation.id,
            json!({
                "slept_seconds": seconds,
            }),
        ))
    }
}

// ── ContextRemainingTool ───────────────────────────────────────────────────

pub(crate) struct ContextRemainingTool {
    #[allow(dead_code)]
    project_root: PathBuf,
}

impl ContextRemainingTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }
}

#[async_trait]
impl Tool for ContextRemainingTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "get_context_remaining",
            "Calculate remaining context tokens based on the context window and used tokens reported in the system prompt. The model should pass the context_window and used_tokens values from the 'Context' line in the system prompt header (e.g. 'Context: 45k / 200k (22%)').",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "context_window": {
                        "type": "integer",
                        "description": "Total context window in tokens (from system prompt context info)."
                    },
                    "used_tokens": {
                        "type": "integer",
                        "description": "Tokens used so far (from system prompt context info)."
                    }
                },
                "required": ["context_window", "used_tokens"],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let context_window = helpers::optional_u64(&invocation.input, "context_window")
            .context("missing required integer `context_window`")?;
        let used_tokens = helpers::optional_u64(&invocation.input, "used_tokens")
            .context("missing required integer `used_tokens`")?;

        let remaining = context_window.saturating_sub(used_tokens);
        let usage_pct = if context_window > 0 {
            (used_tokens as f64 / context_window as f64) * 100.0
        } else {
            0.0
        };

        Ok(helpers::ok(
            invocation.id,
            json!({
                "context_window": context_window,
                "used_tokens": used_tokens,
                "remaining_tokens": remaining,
                "usage_percent": format!("{:.1}%", usage_pct),
                "status": if remaining < context_window / 10 {
                    "CRITICAL — very little context remaining"
                } else if remaining < context_window / 4 {
                    "WARNING — context running low"
                } else {
                    "OK — sufficient context remaining"
                },
            }),
        ))
    }
}

// ── RequestUserInputTool ───────────────────────────────────────────────────

pub(crate) struct RequestUserInputTool;

impl RequestUserInputTool {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for RequestUserInputTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "request_user_input",
            "Request free-form text input from the user. Use this when you need additional information, clarification, or user-provided content that doesn't fit a multiple-choice question. The user will be prompted to provide input through the client.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Short title for what information is needed."
                    },
                    "description": {
                        "type": "string",
                        "description": "Detailed description of what the model needs from the user."
                    },
                    "required": {
                        "type": "boolean",
                        "description": "Whether the user must provide input before continuing."
                    }
                },
                "required": ["title", "description"],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        // This tool requires an interactive client to resolve.
        // The TUI/SDK handles user input resolution through the same
        // mechanism as QuestionTool.
        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: false,
            output: helpers::tool_error(
                "interactive_input_unavailable",
                "request_user_input requires an interactive client",
                true,
                Some(
                    "Run this turn from the TUI or another client that supports user input resolution.",
                ),
                None,
            ),
        })
    }
}

// ── ViewImageTool ──────────────────────────────────────────────────────────

/// Max bytes for images attached as multimodal content (same limit as TUI paste).
const MAX_VIEW_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

pub(crate) struct ViewImageTool {
    project_root: PathBuf,
    /// NAVI data dir — durable attachment blobs live under `attachments/`.
    data_dir: PathBuf,
    name: &'static str,
}

impl ViewImageTool {
    pub(crate) fn new(project_root: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            project_root,
            data_dir,
            name: "view_image",
        }
    }

    pub(crate) fn inspect_image(project_root: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            project_root,
            data_dir,
            name: "inspect_image",
        }
    }
}

fn media_type_for_image_ext(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "tiff" | "tif" => "image/tiff",
        _ => return None,
    })
}

#[async_trait]
impl Tool for ViewImageTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            self.name,
            "Load an image from the project and attach it for visual analysis by the chat model. \
On vision-capable models the image bytes are sent directly in the next API request. \
Returns path, format, size, and confirmation that the image was attached.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Project-relative (or absolute) path to the image file."
                    }
                },
                "required": ["path"],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let raw_path = helpers::required_string(&invocation.input, "path")?.to_string();
        let path = Path::new(&raw_path);
        let full_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.project_root.join(path)
        };

        if !full_path.exists() {
            anyhow::bail!("image file not found: {}", raw_path);
        }

        let ext = full_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        let Some(media_type) = media_type_for_image_ext(&ext) else {
            anyhow::bail!(
                "unsupported image format '.{ext}'. Supported formats: png, jpg, jpeg, gif, webp, bmp, svg, ico, tiff, tif"
            );
        };

        let metadata = full_path
            .metadata()
            .map_err(|e| anyhow::anyhow!("failed to read image metadata: {e}"))?;
        let size_bytes = metadata.len();
        if size_bytes > MAX_VIEW_IMAGE_BYTES {
            anyhow::bail!(
                "image too large ({size_bytes} bytes); max is {MAX_VIEW_IMAGE_BYTES} bytes"
            );
        }

        let bytes = std::fs::read(&full_path)
            .map_err(|e| anyhow::anyhow!("failed to read image file: {e}"))?;
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);

        // Durable copy so session restore works after the project file is gone.
        let attachment_id = crate::attachment_store::store_bytes(&self.data_dir, &bytes, &ext)
            .map_err(|e| anyhow::anyhow!("failed to persist image attachment: {e}"))?;

        // `_navi_content_parts` is stripped by the turn loop before observations
        // and ToolCompleted events; only the model request receives the image.
        // `attachment_id` remains so restore can reload from `{data_dir}/attachments/`.
        let mut output = json!({
            "path": raw_path,
            "format": ext,
            "media_type": media_type,
            "size_bytes": size_bytes,
            "image_attached": true,
            "attachment_id": attachment_id,
            "message": "Image attached for multimodal analysis on the next model request.",
        });
        output[crate::tool::NAVI_CONTENT_PARTS_KEY] = json!([{
            "type": "image",
            "media_type": media_type,
            "data": data,
        }]);
        Ok(helpers::ok(invocation.id, output))
    }
}

// ── NewContextWindowTool ────────────────────────────────────────────────────

pub(crate) struct NewContextWindowTool;

impl NewContextWindowTool {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for NewContextWindowTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "new_context_window",
            "Request a fresh context window by summarizing the conversation so far. \
Provide a summary of what has been accomplished, current state, and next steps.",
            ToolKind::Command,
            json!({
                "type": "object",
                "properties": {
                    "summary": {
                        "type": "string",
                        "description": "Brief summary of the conversation so far. This will be used as the new system context after compaction."
                    }
                },
                "required": ["summary"],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let summary = helpers::required_string(&invocation.input, "summary")?.to_string();

        Ok(helpers::ok(
            invocation.id,
            json!({
                "new_context_requested": true,
                "summary": summary,
                "message": "A new context window will be opened using the provided summary as context. The previous conversation history will be replaced."
            }),
        ))
    }
}

// ── ToolSearchTool ──────────────────────────────────────────────────────────

pub(crate) struct ToolSearchTool {
    registry: Arc<ToolRegistry>,
}

impl ToolSearchTool {
    pub(crate) fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition_with_meta(
            "tool_search",
            "Search the full tool registry for tools matching a query. Returns tool definitions (name, description, schema, tags) for tools not shown by default. Use this to discover deferred or hidden tools when you need a capability not visible in the current tool list.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query — matches against tool names, descriptions, tags, and capabilities."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum results to return (default 10, max 50)."
                    }
                },
                "required": ["query"],
                "additionalProperties": false,
            }),
            ToolMetadata::reader("system", &["search", "discovery", "tools"])
                .with_capability(&["tool.discovery"]),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let query = helpers::required_string(&invocation.input, "query")?.to_string();
        let max_results = helpers::optional_u64(&invocation.input, "max_results")
            .unwrap_or(10)
            .min(50) as usize;

        let results = self.registry.search(&query, max_results);

        // Only return name, description, tags and a simplified schema — not full metadata
        let simplified: Vec<Value> = results
            .iter()
            .map(|def| {
                json!({
                    "name": def.name,
                    "description": def.description,
                    "kind": def.kind,
                    "tags": def.metadata.tags,
                    "capabilities": def.metadata.capabilities,
                    "input_schema": def.input_schema,
                })
            })
            .collect();

        Ok(helpers::ok(
            invocation.id,
            json!({
                "query": query,
                "results": simplified,
                "total": results.len(),
                "hint": if results.is_empty() {
                    "No tools found. Try a different query."
                } else {
                    "Use the tool name from the results to call the tool directly."
                },
            }),
        ))
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolDefinition;
    use std::fs;

    // ── CurrentTimeTool ──────────────────────────────────────────────────

    #[test]
    fn current_time_definition_has_correct_name() {
        let tool = CurrentTimeTool::new();
        let def: ToolDefinition = tool.definition();
        assert_eq!(def.name, "current_time");
        assert!(matches!(def.kind, ToolKind::Read));
    }

    #[tokio::test]
    async fn current_time_invoke_returns_utc_iso() {
        let tool = CurrentTimeTool::new();
        let result = tool
            .invoke(ToolInvocation {
                id: "t1".into(),
                tool_name: "current_time".into(),
                input: json!({}),
            })
            .await
            .unwrap();

        assert!(result.ok);
        let iso = result.output["utc_iso"].as_str().unwrap();
        assert!(
            iso.ends_with('Z'),
            "expected UTC time ending in Z, got {iso}"
        );
        assert!(iso.contains('T'), "expected ISO 8601 format, got {iso}");
        let unix = result.output["unix_timestamp_seconds"].as_u64().unwrap();
        assert!(unix > 1_700_000_000, "unix timestamp seems too old: {unix}");
        assert_eq!(result.output["timezone"], "UTC");
    }

    #[tokio::test]
    async fn current_time_invoke_utc_iso_format() {
        let tool = CurrentTimeTool::new();
        let result = tool
            .invoke(ToolInvocation {
                id: "t2".into(),
                tool_name: "current_time".into(),
                input: json!({}),
            })
            .await
            .unwrap();

        // Basic ISO 8601 format check: YYYY-MM-DDTHH:MM:SSZ
        let iso = result.output["utc_iso"].as_str().unwrap();
        assert_eq!(iso.len(), 20, "ISO 8601 UTC should be 20 chars: {iso}");
        assert_eq!(&iso[4..5], "-");
        assert_eq!(&iso[7..8], "-");
        assert_eq!(&iso[10..11], "T");
        assert_eq!(&iso[13..14], ":");
        assert_eq!(&iso[16..17], ":");
        assert_eq!(&iso[19..20], "Z");
    }

    #[tokio::test]
    async fn current_time_days_to_date_simple() {
        // Unix epoch (1970-01-01)
        let (y, m, d) = days_to_date(0);
        assert_eq!(y, 1970);
        assert_eq!(m, 1);
        assert_eq!(d, 1);
    }

    // ── SleepTool ────────────────────────────────────────────────────────

    #[test]
    fn sleep_definition_has_correct_name() {
        let tool = SleepTool::new();
        let def: ToolDefinition = tool.definition();
        assert_eq!(def.name, "sleep");
        assert!(matches!(def.kind, ToolKind::Command));
        assert!(
            def.input_schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "seconds")
        );
    }

    #[tokio::test]
    async fn sleep_invoke_one_second() {
        let tool = SleepTool::new();
        let start = std::time::Instant::now();
        let result = tool
            .invoke(ToolInvocation {
                id: "t3".into(),
                tool_name: "sleep".into(),
                input: json!({ "seconds": 1 }),
            })
            .await
            .unwrap();

        let elapsed = start.elapsed();
        assert!(result.ok);
        assert!(elapsed.as_secs() >= 1);
        assert_eq!(result.output["slept_seconds"], 1);
    }

    #[tokio::test]
    async fn sleep_invoke_zero_clamps_to_one() {
        let tool = SleepTool::new();
        let result = tool
            .invoke(ToolInvocation {
                id: "t4".into(),
                tool_name: "sleep".into(),
                input: json!({ "seconds": 0 }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["slept_seconds"], 1);
    }

    // ── ContextRemainingTool ─────────────────────────────────────────────

    #[test]
    fn context_remaining_definition_has_correct_name() {
        let tool = ContextRemainingTool::new(PathBuf::from("/tmp"));
        let def: ToolDefinition = tool.definition();
        assert_eq!(def.name, "get_context_remaining");
        assert!(matches!(def.kind, ToolKind::Read));
    }

    #[tokio::test]
    async fn context_remaining_invoke_basic() {
        let tool = ContextRemainingTool::new(PathBuf::from("/tmp"));
        let result = tool
            .invoke(ToolInvocation {
                id: "t6".into(),
                tool_name: "get_context_remaining".into(),
                input: json!({ "context_window": 200000, "used_tokens": 45000 }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["context_window"], 200000);
        assert_eq!(result.output["used_tokens"], 45000);
        assert_eq!(result.output["remaining_tokens"], 155000);
        assert_eq!(result.output["usage_percent"], "22.5%");
        assert!(result.output["status"].as_str().unwrap().contains("OK"));
    }

    #[tokio::test]
    async fn context_remaining_critical_threshold() {
        let tool = ContextRemainingTool::new(PathBuf::from("/tmp"));
        let result = tool
            .invoke(ToolInvocation {
                id: "t7".into(),
                tool_name: "get_context_remaining".into(),
                input: json!({ "context_window": 200000, "used_tokens": 190000 }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["remaining_tokens"], 10000);
        assert!(
            result.output["status"]
                .as_str()
                .unwrap()
                .contains("CRITICAL")
        );
    }

    #[tokio::test]
    async fn context_remaining_warning_threshold() {
        let tool = ContextRemainingTool::new(PathBuf::from("/tmp"));
        let result = tool
            .invoke(ToolInvocation {
                id: "t8".into(),
                tool_name: "get_context_remaining".into(),
                input: json!({ "context_window": 200000, "used_tokens": 160000 }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["remaining_tokens"], 40000);
        assert!(
            result.output["status"]
                .as_str()
                .unwrap()
                .contains("WARNING")
        );
    }

    #[tokio::test]
    async fn context_remaining_handles_full_context() {
        let tool = ContextRemainingTool::new(PathBuf::from("/tmp"));
        let result = tool
            .invoke(ToolInvocation {
                id: "t9".into(),
                tool_name: "get_context_remaining".into(),
                input: json!({ "context_window": 1000, "used_tokens": 1000 }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["remaining_tokens"], 0);
        assert_eq!(result.output["usage_percent"], "100.0%");
    }

    #[tokio::test]
    async fn context_remaining_zero_window_no_panic() {
        let tool = ContextRemainingTool::new(PathBuf::from("/tmp"));
        let result = tool
            .invoke(ToolInvocation {
                id: "t10".into(),
                tool_name: "get_context_remaining".into(),
                input: json!({ "context_window": 0, "used_tokens": 0 }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["remaining_tokens"], 0);
        assert_eq!(result.output["usage_percent"], "0.0%");
    }

    // ── RequestUserInputTool ─────────────────────────────────────────────

    #[test]
    fn request_user_input_definition_has_correct_name() {
        let tool = RequestUserInputTool::new();
        let def: ToolDefinition = tool.definition();
        assert_eq!(def.name, "request_user_input");
        assert!(matches!(def.kind, ToolKind::Read));
        assert!(
            def.input_schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("title"))
        );
        assert!(
            def.input_schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("description"))
        );
    }

    #[tokio::test]
    async fn request_user_input_returns_interactive_error() {
        let tool = RequestUserInputTool::new();
        let result = tool
            .invoke(ToolInvocation {
                id: "t11".into(),
                tool_name: "request_user_input".into(),
                input: json!({
                    "title": "Need API key",
                    "description": "Please provide your API key to continue.",
                    "required": true,
                }),
            })
            .await
            .unwrap();

        assert!(!result.ok);
        assert_eq!(result.output["error_code"], "interactive_input_unavailable");
    }

    #[tokio::test]
    async fn request_user_input_required_false_also_returns_error() {
        let tool = RequestUserInputTool::new();
        let result = tool
            .invoke(ToolInvocation {
                id: "t12".into(),
                tool_name: "request_user_input".into(),
                input: json!({
                    "title": "Optional info",
                    "description": "If you have any additional details.",
                    "required": false,
                }),
            })
            .await
            .unwrap();

        assert!(!result.ok);
    }

    // ── ViewImageTool ────────────────────────────────────────────────────

    #[test]
    fn view_image_definition_has_correct_name() {
        let tool = ViewImageTool::new(PathBuf::from("/tmp"), PathBuf::from("/tmp/navi-data"));
        let def: ToolDefinition = tool.definition();
        assert_eq!(def.name, "view_image");
        assert!(matches!(def.kind, ToolKind::Read));
        assert!(
            def.input_schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "path")
        );
    }

    #[tokio::test]
    async fn view_image_returns_metadata_for_png() {
        let dir = tempfile::tempdir().unwrap();
        let img_path = dir.path().join("test.png");
        // Create a minimal valid PNG file.
        let minimal_png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0x60, 0x60, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE5, 0x27, 0xDE, 0xFC,
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        fs::write(&img_path, minimal_png).unwrap();

        let data_dir = tempfile::tempdir().unwrap();
        let tool = ViewImageTool::new(dir.path().to_path_buf(), data_dir.path().to_path_buf());
        let mut result = tool
            .invoke(ToolInvocation {
                id: "t13".into(),
                tool_name: "view_image".into(),
                input: json!({ "path": "test.png" }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["format"], "png");
        assert_eq!(result.output["size_bytes"], minimal_png.len() as u64);
        assert_eq!(result.output["path"], "test.png");
        assert_eq!(result.output["image_attached"], true);
        assert_eq!(result.output["media_type"], "image/png");
        let attachment_id = result.output["attachment_id"].as_str().unwrap();
        assert!(!attachment_id.is_empty());
        assert!(
            crate::attachment_store::load_bytes(data_dir.path(), attachment_id).is_ok(),
            "image must be persisted for restore after source deletion"
        );

        let parts = crate::tool::take_tool_content_parts(&mut result);
        assert_eq!(parts.len(), 1);
        assert!(parts[0].is_image());
        assert_eq!(parts[0].media_type(), Some("image/png"));
        assert!(parts[0].data().is_some_and(|d| !d.is_empty()));
        // Base64 payload must not remain in the public output after take.
        assert!(result.output.get(crate::tool::NAVI_CONTENT_PARTS_KEY).is_none());
    }

    #[tokio::test]
    async fn view_image_rejects_unsupported_format() {
        let dir = tempfile::tempdir().unwrap();
        let img_path = dir.path().join("test.txt");
        fs::write(&img_path, "not an image").unwrap();

        let data_dir = tempfile::tempdir().unwrap();
        let tool = ViewImageTool::new(dir.path().to_path_buf(), data_dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "t14".into(),
                tool_name: "view_image".into(),
                input: json!({ "path": "test.txt" }),
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn view_image_handles_missing_file() {
        let data_dir = tempfile::tempdir().unwrap();
        let tool = ViewImageTool::new(PathBuf::from("/tmp"), data_dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "t15".into(),
                tool_name: "view_image".into(),
                input: json!({ "path": "nonexistent.png" }),
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn view_image_resolves_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let img_path = dir.path().join("abs.png");
        fs::write(&img_path, "fake png content").unwrap();

        let tool = ViewImageTool::new(
            PathBuf::from("/nonexistent"),
            data_dir.path().to_path_buf(),
        );
        let result = tool
            .invoke(ToolInvocation {
                id: "t16".into(),
                tool_name: "view_image".into(),
                input: json!({ "path": img_path.to_string_lossy() }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["format"], "png");
    }

    #[tokio::test]
    async fn view_image_supports_multiple_formats() {
        for ext in &["jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico"] {
            let dir = tempfile::tempdir().unwrap();
            let data_dir = tempfile::tempdir().unwrap();
            let img_path = dir.path().join(format!("test.{ext}"));
            fs::write(&img_path, format!("fake {ext} content")).unwrap();

            let tool = ViewImageTool::new(dir.path().to_path_buf(), data_dir.path().to_path_buf());
            let result = tool
                .invoke(ToolInvocation {
                    id: format!("t17-{ext}"),
                    tool_name: "view_image".into(),
                    input: json!({ "path": format!("test.{ext}") }),
                })
                .await
                .unwrap();

            assert!(result.ok, "failed for .{ext}");
            assert_eq!(result.output["format"], *ext, "wrong format for .{ext}");
        }
    }

    #[tokio::test]
    async fn view_image_restore_works_after_source_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let img_path = dir.path().join("gone.png");
        let minimal_png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0x60, 0x60, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE5, 0x27, 0xDE, 0xFC,
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        fs::write(&img_path, minimal_png).unwrap();
        let tool = ViewImageTool::new(dir.path().to_path_buf(), data_dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "t18".into(),
                tool_name: "view_image".into(),
                input: json!({ "path": "gone.png" }),
            })
            .await
            .unwrap();
        fs::remove_file(&img_path).unwrap();

        let parts = crate::session_replay::rehydrate_tool_content_parts(
            "view_image",
            &result,
            Some(dir.path()),
            Some(data_dir.path()),
        );
        assert_eq!(parts.len(), 1);
        assert!(parts[0].is_image());
        assert!(!parts[0].data().unwrap_or("").is_empty());
    }

    // ── NewContextWindowTool ─────────────────────────────────────────────

    #[test]
    fn new_context_window_definition_has_correct_name() {
        let tool = NewContextWindowTool::new();
        let def: ToolDefinition = tool.definition();
        assert_eq!(def.name, "new_context_window");
        assert!(matches!(def.kind, ToolKind::Command));
        assert!(
            def.input_schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "summary")
        );
    }

    #[tokio::test]
    async fn new_context_window_invoke_basic() {
        let tool = NewContextWindowTool::new();
        let result = tool
            .invoke(ToolInvocation {
                id: "t18".into(),
                tool_name: "new_context_window".into(),
                input: json!({
                    "summary": "Implemented the authentication module and fixed the login bug."
                }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert!(result.output["new_context_requested"].as_bool().unwrap());
        assert_eq!(
            result.output["summary"],
            "Implemented the authentication module and fixed the login bug."
        );
        assert!(
            result.output["message"]
                .as_str()
                .unwrap()
                .contains("new context window")
        );
    }

    #[tokio::test]
    async fn new_context_window_invoke_empty_summary_fails() {
        let tool = NewContextWindowTool::new();
        let result = tool
            .invoke(ToolInvocation {
                id: "t19".into(),
                tool_name: "new_context_window".into(),
                input: json!({ "summary": "" }),
            })
            .await;

        assert!(result.is_err());
    }
}
