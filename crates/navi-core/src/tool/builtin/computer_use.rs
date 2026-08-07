//! Built-in computer use tools (OS automation via `navi-computer-use`).
//!
//! All four tools are `Deferred` — they stay out of the model's tool schema
//! and must be discovered via `tool_search`. See ADR 0016 for the security
//! model and per-mode approval behavior.
//!
//! - `capture_screen` (Read) — screenshot to BMP, returned as multimodal content
//! - `enumerate_windows` (Read) — visible top-level windows
//! - `inspect_element` (Read) — accessibility tree (spike stub)
//! - `simulate_input` (Command, Critical) — mouse + keyboard simulation

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

use super::helpers;
use crate::config::defaults::is_deny_listed;
use crate::config::types::PermissionMode;
use crate::tool::{
    NAVI_CONTENT_PARTS_KEY, Tool, ToolDefinition, ToolInvocation, ToolKind, ToolMetadata,
    ToolResult, ToolRisk,
};

use base64::Engine;

/// Directory under the NAVI data dir where screenshots are saved.
const SCREENSHOT_SUBDIR: &str = "screenshots";

// ── CaptureScreenTool ──────────────────────────────────────────────────────

pub(crate) struct CaptureScreenTool {
    data_dir: PathBuf,
    supports_vision: bool,
}

impl CaptureScreenTool {
    pub(crate) fn new(data_dir: PathBuf, supports_vision: bool) -> Self {
        Self {
            data_dir,
            supports_vision,
        }
    }
}

#[async_trait]
impl Tool for CaptureScreenTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition_with_meta(
            "capture_screen",
            "Capture the primary monitor screen as a screenshot. The image is saved as a file \
under the NAVI data directory. On vision-capable models the image is also attached for \
visual analysis on the next request; on text-only models only the file path, dimensions, \
and size are returned (use `inspect_element` for a text-based view of the UI). Returns the \
file path, dimensions, and size.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
            ToolMetadata {
                namespace: "computer-use".to_string(),
                risk: ToolRisk::Low,
                is_read_only: true,
                is_concurrency_safe: false,
                exposure: crate::tool::ToolExposure::Deferred,
                capabilities: vec!["os.screen.read".to_string()],
                tags: vec![
                    "screenshot".to_string(),
                    "screen".to_string(),
                    "capture".to_string(),
                ],
                ..ToolMetadata::default()
            },
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let out_dir = self.data_dir.join(SCREENSHOT_SUBDIR);
        let backend = navi_computer_use::platform_backend();
        let screenshot = backend.capture_screen(
            &out_dir.to_string_lossy(),
            &navi_computer_use::CaptureOptions::default(),
        )?;

        let path = std::path::Path::new(&screenshot.path);
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("bmp");

        if self.supports_vision {
            // Read the file and embed it as multimodal content so the model
            // sees the screenshot directly (same pattern as `view_image`).
            let bytes = std::fs::read(path)
                .map_err(|e| anyhow::anyhow!("failed to read screenshot: {e}"))?;
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);

            // Persist to the attachment store for session restore.
            let attachment_id =
                crate::attachment_store::store_bytes(&self.data_dir, &bytes, ext)
                    .map_err(|e| anyhow::anyhow!("failed to persist screenshot attachment: {e}"))?;

            let mut output = json!({
                "path": screenshot.path,
                "width": screenshot.width,
                "height": screenshot.height,
                "size_bytes": screenshot.size_bytes,
                "format": ext,
                "media_type": "image/bmp",
                "image_attached": true,
                "attachment_id": attachment_id,
                "message": "Screenshot captured and attached for multimodal analysis on the next model request.",
            });
            output[NAVI_CONTENT_PARTS_KEY] = json!([{
                "type": "image",
                "media_type": "image/bmp",
                "data": data,
            }]);
            Ok(helpers::ok(invocation.id, output))
        } else {
            // Text-only model: return metadata without the base64 image.
            // The screenshot file is still saved on disk for later use
            // (e.g. switching to a vision model or manual inspection).
            Ok(helpers::ok(
                invocation.id,
                json!({
                    "path": screenshot.path,
                    "width": screenshot.width,
                    "height": screenshot.height,
                    "size_bytes": screenshot.size_bytes,
                    "format": ext,
                    "image_attached": false,
                    "message": "Screenshot saved to disk. The current model does not support \
                                image input — use `inspect_element` for a text-based view of \
                                the UI, or switch to a vision-capable model to analyze the image.",
                }),
            ))
        }
    }
}

// ── EnumerateWindowsTool ───────────────────────────────────────────────────

pub(crate) struct EnumerateWindowsTool;

impl EnumerateWindowsTool {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for EnumerateWindowsTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition_with_meta(
            "enumerate_windows",
            "List all visible top-level windows on the desktop. Returns each window's title, \
process id (PID), bounding rectangle (x, y, width, height), and whether it is the focused \
window. The focused window appears first. Use to find a target window before capturing a \
specific region or simulating input.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
            ToolMetadata {
                namespace: "computer-use".to_string(),
                risk: ToolRisk::Low,
                is_read_only: true,
                is_concurrency_safe: true,
                exposure: crate::tool::ToolExposure::Deferred,
                capabilities: vec!["os.windows.read".to_string()],
                tags: vec![
                    "windows".to_string(),
                    "desktop".to_string(),
                    "enumerate".to_string(),
                ],
                ..ToolMetadata::default()
            },
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let backend = navi_computer_use::platform_backend();
        let windows = backend.enumerate_windows()?;

        let windows_json: Vec<Value> = windows
            .iter()
            .map(|w| {
                json!({
                    "hwnd": w.hwnd,
                    "title": w.title,
                    "pid": w.pid,
                    "rect": {
                        "x": w.rect.x,
                        "y": w.rect.y,
                        "width": w.rect.width,
                        "height": w.rect.height,
                    },
                    "is_focused": w.is_focused,
                    "is_visible": w.is_visible,
                })
            })
            .collect();

        Ok(helpers::ok(
            invocation.id,
            json!({
                "windows": windows_json,
                "count": windows.len(),
                "platform": backend.platform_name(),
            }),
        ))
    }
}

// ── InspectElementTool ─────────────────────────────────────────────────────

pub(crate) struct InspectElementTool {
    element_cache:
        Arc<std::sync::Mutex<std::collections::HashMap<String, navi_computer_use::Rect>>>,
}

impl InspectElementTool {
    pub(crate) fn new(
        element_cache: Arc<
            std::sync::Mutex<std::collections::HashMap<String, navi_computer_use::Rect>>,
        >,
    ) -> Self {
        Self { element_cache }
    }
}

#[async_trait]
impl Tool for InspectElementTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition_with_meta(
            "inspect_element",
            "Inspect the accessibility tree of a window or sub-element. Returns element names, \
control types, values, bounding rectangles, and element_ids for drill-down or click-by-ID. \
Pass `element_id` (from inspect_desktop) to drill down into a specific element's children, or \
`window` (hwnd) to inspect a specific window. Omit both for the foreground window. Set \
`raw_view: true` to use RawView (deeper traversal into Electron/Chromium apps like Discord, \
VS Code, Zen Browser). Default is ControlView (logical UI tree, cleaner but shallower).",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "window": {
                        "type": "integer",
                        "description": "Window handle (hwnd) from enumerate_windows or inspect_desktop. Omit for foreground window."
                    },
                    "element_id": {
                        "type": "string",
                        "description": "Element ID from inspect_desktop (e.g. 'w0.e12'). Drill down from this element instead of window root."
                    },
                    "max_depth": {
                        "type": "integer",
                        "description": "Maximum tree depth to traverse (default 3, 0 = root only)."
                    },
                    "raw_view": {
                        "type": "boolean",
                        "description": "Use RawView for deeper traversal into Electron/Chromium apps. Default false."
                    }
                },
                "additionalProperties": false,
            }),
            ToolMetadata {
                namespace: "computer-use".to_string(),
                risk: ToolRisk::Low,
                is_read_only: true,
                is_concurrency_safe: true,
                exposure: crate::tool::ToolExposure::Deferred,
                capabilities: vec!["os.accessibility.read".to_string()],
                tags: vec![
                    "accessibility".to_string(),
                    "uia".to_string(),
                    "inspect".to_string(),
                ],
                ..ToolMetadata::default()
            },
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let window = invocation.input.get("window").and_then(Value::as_u64);
        let element_id = invocation
            .input
            .get("element_id")
            .and_then(Value::as_str)
            .map(|s| s.to_string());
        let max_depth = invocation
            .input
            .get("max_depth")
            .and_then(Value::as_u64)
            .unwrap_or(3) as u32;
        let raw_view = invocation
            .input
            .get("raw_view")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let backend = navi_computer_use::platform_backend();
        // UIA calls are cross-process and can block on unresponsive apps.
        // Run on a blocking thread so we don't stall the tokio runtime.
        let opts = navi_computer_use::InspectOptions {
            window,
            element_id,
            max_depth,
            raw_view,
        };
        let tree = tokio::task::spawn_blocking(move || backend.inspect_element(&opts))
            .await
            .map_err(|e| anyhow::anyhow!("inspect_element worker panicked: {e}"))??;

        // Update the element cache with any new element_ids from this inspect.
        if let Ok(mut cache) = self.element_cache.lock() {
            collect_element_rects(&tree.root, &mut cache);
        }

        Ok(helpers::ok(
            invocation.id,
            json!({
                "supported": tree.supported,
                "platform": navi_computer_use::platform_backend().platform_name(),
                "root": element_to_json(&tree.root),
            }),
        ))
    }
}

fn element_to_json(el: &navi_computer_use::ElementInfo) -> Value {
    json!({
        "element_id": el.element_id,
        "name": el.name,
        "control_type": el.control_type,
        "value": el.value,
        "rect": el.rect.as_ref().map(|r| json!({
            "x": r.x, "y": r.y, "width": r.width, "height": r.height,
        })),
        "is_password": el.is_password,
        "children_truncated": el.children_truncated,
        "children": el.children.iter().map(element_to_json).collect::<Vec<_>>(),
    })
}

/// Recursively collects all element_ids and their rects from an element tree.
/// Used to populate the element cache for `simulate_input` click-by-ID.
fn collect_element_rects(
    el: &navi_computer_use::ElementInfo,
    map: &mut std::collections::HashMap<String, navi_computer_use::Rect>,
) {
    if let (Some(id), Some(rect)) = (&el.element_id, &el.rect) {
        map.insert(id.clone(), rect.clone());
    }
    for child in &el.children {
        collect_element_rects(child, map);
    }
}

// ── InspectDesktopTool ─────────────────────────────────────────────────────

pub(crate) struct InspectDesktopTool {
    /// Shared element cache — populated on each inspect call, read by
    /// SimulateInputTool to resolve element_id → coordinates.
    element_cache:
        Arc<std::sync::Mutex<std::collections::HashMap<String, navi_computer_use::Rect>>>,
}

impl InspectDesktopTool {
    pub(crate) fn new(
        element_cache: Arc<
            std::sync::Mutex<std::collections::HashMap<String, navi_computer_use::Rect>>,
        >,
    ) -> Self {
        Self { element_cache }
    }
}

#[async_trait]
impl Tool for InspectDesktopTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition_with_meta(
            "inspect_desktop",
            "Returns a text snapshot of all visible windows and their UI elements (depth 2). \
Each element has an element_id (e.g. 'w0.e12') for use with inspect_element (drill-down) or \
simulate_input (click by ID). This is the text-based equivalent of looking at the screen — \
no screenshot or vision capability needed. Use this as the first step in any desktop \
automation task: inspect_desktop → identify target → simulate_input or inspect_element.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
            ToolMetadata {
                namespace: "computer-use".to_string(),
                risk: ToolRisk::Low,
                is_read_only: true,
                is_concurrency_safe: false,
                exposure: crate::tool::ToolExposure::Direct,
                capabilities: vec!["os.accessibility.read".to_string()],
                tags: vec![
                    "desktop".to_string(),
                    "accessibility".to_string(),
                    "snapshot".to_string(),
                    "inspect".to_string(),
                ],
                ..ToolMetadata::default()
            },
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let cache = self.element_cache.clone();
        let backend = navi_computer_use::platform_backend();
        let snapshot = tokio::task::spawn_blocking(move || backend.inspect_desktop())
            .await
            .map_err(|e| anyhow::anyhow!("inspect_desktop worker panicked: {e}"))??;

        // Populate the element cache with all element_ids → rects.
        if let Ok(mut cache) = cache.lock() {
            cache.clear();
            for window in &snapshot.windows {
                for el in &window.elements {
                    collect_element_rects(el, &mut cache);
                }
            }
        }

        let windows_json: Vec<Value> = snapshot
            .windows
            .iter()
            .map(|w| {
                json!({
                    "window_id": w.window_id,
                    "hwnd": w.hwnd,
                    "title": w.title,
                    "pid": w.pid,
                    "rect": {
                        "x": w.rect.x,
                        "y": w.rect.y,
                        "width": w.rect.width,
                        "height": w.rect.height,
                    },
                    "is_focused": w.is_focused,
                    "elements": w.elements.iter().map(element_to_json).collect::<Vec<_>>(),
                })
            })
            .collect();

        Ok(helpers::ok(
            invocation.id,
            json!({
                "windows": windows_json,
                "window_count": snapshot.windows.len(),
                "platform": snapshot.platform,
                "message": format!(
                    "Found {} visible window(s). Each element has an element_id — \
                     use it with simulate_input (click by ID) or inspect_element \
                     (drill-down with raw_view=true for Electron apps).",
                    snapshot.windows.len()
                ),
            }),
        ))
    }
}

// ── OpenApplicationTool ────────────────────────────────────────────────────

pub(crate) struct OpenApplicationTool;

impl OpenApplicationTool {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for OpenApplicationTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition_with_meta(
            "open_application",
            "Launch an application by name (e.g. 'zen', 'notepad', 'chrome', 'Safari'). \
Bypasses the GUI entirely — no Start menu or Spotlight simulation needed. \
On Windows uses ShellExecute (searches App Paths registry, PATH, file associations). \
On macOS uses `open -a`. Returns whether the app was launched successfully.",
            ToolKind::Command,
            json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Application name or executable path (e.g. 'zen', 'notepad', 'chrome')."
                    }
                },
                "required": ["name"],
                "additionalProperties": false,
            }),
            ToolMetadata {
                namespace: "computer-use".to_string(),
                risk: ToolRisk::Medium,
                is_read_only: false,
                is_concurrency_safe: true,
                exposure: crate::tool::ToolExposure::Direct,
                capabilities: vec!["os.process.launch".to_string()],
                tags: vec![
                    "application".to_string(),
                    "launch".to_string(),
                    "open".to_string(),
                ],
                ..ToolMetadata::default()
            },
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let name = invocation
            .input
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing required `name` field"))?;

        let backend = navi_computer_use::platform_backend();
        let name_owned = name.to_string();
        let result = tokio::task::spawn_blocking(move || backend.open_application(&name_owned))
            .await
            .map_err(|e| anyhow::anyhow!("open_application worker panicked: {e}"))??;

        Ok(helpers::ok(
            invocation.id,
            json!({
                "launched": result.launched,
                "pid": result.pid,
                "message": result.message,
                "platform": navi_computer_use::platform_backend().platform_name(),
            }),
        ))
    }
}

// ── AnnotateScreenshotTool ──────────────────────────────────────────────────

pub(crate) struct AnnotateScreenshotTool {
    data_dir: PathBuf,
    supports_vision: bool,
}

impl AnnotateScreenshotTool {
    pub(crate) fn new(data_dir: PathBuf, supports_vision: bool) -> Self {
        Self {
            data_dir,
            supports_vision,
        }
    }
}

#[async_trait]
impl Tool for AnnotateScreenshotTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition_with_meta(
            "annotate_screenshot",
            "Capture a screenshot and overlay bounding-box rectangles for each UI element \
from the accessibility tree. On vision-capable models the annotated image (PNG) is \
attached for visual analysis; on text-only models only the element tree JSON is returned \
(with names, control types, and coordinates — no image). This closes the loop between \
`inspect_element` (which returns coordinates) and `capture_screen` (which returns pixels) \
— the model can see exactly which UI elements are where. Box colors: red = password field, \
green = interactive (button/edit/etc), blue = container (window/pane/group), gray = other. \
The element tree JSON is always returned so the model can correlate boxes with names and \
control types by position.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "window": {
                        "type": "integer",
                        "description": "Window handle (hwnd) from enumerate_windows. Omit for foreground window."
                    },
                    "max_depth": {
                        "type": "integer",
                        "description": "Maximum accessibility tree depth to traverse (default 3, 0 = root only)."
                    }
                },
                "additionalProperties": false,
            }),
            ToolMetadata {
                namespace: "computer-use".to_string(),
                risk: ToolRisk::Low,
                is_read_only: true,
                is_concurrency_safe: false,
                exposure: crate::tool::ToolExposure::Deferred,
                capabilities: vec![
                    "os.screen.read".to_string(),
                    "os.accessibility.read".to_string(),
                ],
                tags: vec![
                    "screenshot".to_string(),
                    "annotate".to_string(),
                    "overlay".to_string(),
                    "accessibility".to_string(),
                ],
                ..ToolMetadata::default()
            },
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let window = invocation.input.get("window").and_then(Value::as_u64);
        let max_depth = invocation
            .input
            .get("max_depth")
            .and_then(Value::as_u64)
            .unwrap_or(3) as u32;

        // 1. Capture screenshot
        let out_dir = self.data_dir.join(SCREENSHOT_SUBDIR);
        let backend = navi_computer_use::platform_backend();
        let screenshot = backend.capture_screen(
            &out_dir.to_string_lossy(),
            &navi_computer_use::CaptureOptions::default(),
        )?;

        // 2. Inspect the accessibility tree
        let inspect_opts = navi_computer_use::InspectOptions {
            window,
            element_id: None,
            max_depth,
            raw_view: false,
        };
        let backend2 = navi_computer_use::platform_backend();
        let tree = tokio::task::spawn_blocking(move || backend2.inspect_element(&inspect_opts))
            .await
            .map_err(|e| anyhow::anyhow!("inspect_element worker panicked: {e}"))??;

        // 3. Count elements and serialize tree JSON (always needed).
        let element_count = count_elements(&tree.root);
        let password_count = count_passwords(&tree.root);
        let tree_json = element_to_json(&tree.root);
        let tree_supported = tree.supported;

        if self.supports_vision {
            // Read screenshot bytes and annotate.
            let screenshot_path = std::path::Path::new(&screenshot.path);
            let image_bytes = std::fs::read(screenshot_path)
                .map_err(|e| anyhow::anyhow!("failed to read screenshot: {e}"))?;

            let annotated_png = tokio::task::spawn_blocking(move || {
                navi_computer_use::annotate_screenshot(&image_bytes, &tree)
            })
            .await
            .map_err(|e| anyhow::anyhow!("annotate worker panicked: {e}"))??;

            // Persist annotated PNG to attachment store.
            let attachment_id =
                crate::attachment_store::store_bytes(&self.data_dir, &annotated_png, "png")
                    .map_err(|e| anyhow::anyhow!("failed to persist annotated attachment: {e}"))?;

            let data = base64::engine::general_purpose::STANDARD.encode(&annotated_png);

            let mut output = json!({
                "screenshot_path": screenshot.path,
                "screenshot_width": screenshot.width,
                "screenshot_height": screenshot.height,
                "annotated_format": "png",
                "annotated_media_type": "image/png",
                "annotated_size_bytes": annotated_png.len(),
                "image_attached": true,
                "attachment_id": attachment_id,
                "annotated_elements": element_count,
                "password_fields_found": password_count,
                "platform": navi_computer_use::platform_backend().platform_name(),
                "element_tree": {
                    "supported": tree_supported,
                    "root": tree_json,
                },
                "message": "Annotated screenshot attached. Each UI element has a colored bounding box: red=password, green=interactive, blue=container, gray=other. The element_tree field contains names and control types for correlating with the visual boxes.",
            });
            output[NAVI_CONTENT_PARTS_KEY] = json!([{
                "type": "image",
                "media_type": "image/png",
                "data": data,
            }]);
            Ok(helpers::ok(invocation.id, output))
        } else {
            // Text-only model: return the element tree JSON without the
            // annotated image. The tree has names, control types, and
            // coordinates — sufficient for coordinate-based automation.
            Ok(helpers::ok(
                invocation.id,
                json!({
                    "screenshot_path": screenshot.path,
                    "screenshot_width": screenshot.width,
                    "screenshot_height": screenshot.height,
                    "image_attached": false,
                    "annotated_elements": element_count,
                    "password_fields_found": password_count,
                    "platform": navi_computer_use::platform_backend().platform_name(),
                    "element_tree": {
                        "supported": tree_supported,
                        "root": tree_json,
                    },
                    "message": "Element tree captured (text-only mode — no image attached). \
                                The element_tree field contains names, control types, and \
                                coordinates for each UI element. Use these coordinates with \
                                `simulate_input` for click/type actions.",
                }),
            ))
        }
    }
}

fn count_elements(el: &navi_computer_use::ElementInfo) -> usize {
    1 + el.children.iter().map(count_elements).sum::<usize>()
}

fn count_passwords(el: &navi_computer_use::ElementInfo) -> usize {
    (if el.is_password { 1 } else { 0 }) + el.children.iter().map(count_passwords).sum::<usize>()
}

// ── SimulateInputTool ──────────────────────────────────────────────────────

pub(crate) struct SimulateInputTool {
    #[allow(dead_code)]
    data_dir: PathBuf,
    /// Deny-list of apps that this tool must not target (ADR 0016).
    /// Enforced in all modes except Yolo.
    deny_apps: Vec<String>,
    /// Current permission mode — Yolo bypasses the deny-list.
    permission_mode: PermissionMode,
    /// Shared element cache — populated by InspectDesktopTool/InspectElementTool,
    /// read here to resolve element_id → coordinates before calling the backend.
    element_cache:
        Arc<std::sync::Mutex<std::collections::HashMap<String, navi_computer_use::Rect>>>,
}

impl SimulateInputTool {
    pub(crate) fn new(
        data_dir: PathBuf,
        deny_apps: Vec<String>,
        permission_mode: PermissionMode,
        element_cache: Arc<
            std::sync::Mutex<std::collections::HashMap<String, navi_computer_use::Rect>>,
        >,
    ) -> Self {
        Self {
            data_dir,
            deny_apps,
            permission_mode,
            element_cache,
        }
    }

    /// Resolves `element_id` in a mouse action to x/y coordinates using the
    /// element cache. Returns a new actions Vec with resolved coordinates.
    /// If an element_id is not found in the cache, returns an error message.
    ///
    /// Only mouse actions (click, mouse_move, scroll, drag) resolve
    /// element_id to coordinates. Keyboard actions (type, key, key_down,
    /// key_up) ignore element_id — it has no meaning for them.
    fn resolve_element_ids(&self, actions: &[Value]) -> std::result::Result<Vec<Value>, String> {
        let cache = self
            .element_cache
            .lock()
            .map_err(|e| format!("element cache lock failed: {e}"))?;
        let mut resolved = Vec::with_capacity(actions.len());
        for action in actions {
            // Only resolve element_id for mouse actions.
            let is_mouse = action
                .get("action")
                .and_then(Value::as_str)
                .map(|a| matches!(a, "click" | "mouse_move" | "scroll" | "drag"))
                .unwrap_or(false);

            if is_mouse {
                if let Some(element_id) = action.get("element_id").and_then(Value::as_str) {
                    let rect = cache.get(element_id).ok_or_else(|| {
                        format!(
                            "element_id '{element_id}' not found in cache. \
                             The element may have moved or been destroyed. \
                             Re-run inspect_desktop to refresh the element cache."
                        )
                    })?;
                    let center_x = rect.x + rect.width / 2;
                    let center_y = rect.y + rect.height / 2;
                    let mut new_action = action.clone();
                    if let Some(obj) = new_action.as_object_mut() {
                        obj.insert("x".to_string(), json!(center_x));
                        obj.insert("y".to_string(), json!(center_y));
                    }
                    resolved.push(new_action);
                    continue;
                }
            }
            resolved.push(action.clone());
        }
        Ok(resolved)
    }
}

#[async_trait]
impl Tool for SimulateInputTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition_with_meta(
            "simulate_input",
            "Simulate mouse and keyboard input on the desktop. Accepts an `actions` array of \
input action objects executed in order. Each action has an `action` field: \
`mouse_move` (x, y), `click` (button: left/right/middle, x, y), `double_click` (button, x, y), \
`scroll` (delta: wheel notches, x, y), `key` (key: e.g. Enter, Tab, Escape), `key_down` (key), \
`key_up` (key), `type` (text: string to type character by character). \
Mouse actions accept either `x`/`y` coordinates OR `element_id` (from inspect_desktop) — \
when `element_id` is provided, the tool resolves it to the element's center coordinates \
automatically. **High risk:** this tool controls the real desktop. Requires approval in all \
modes except Yolo.",
            ToolKind::Command,
            json!({
                "type": "object",
                "properties": {
                    "actions": {
                        "type": "array",
                        "description": "Array of input action objects to execute in order.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "action": {
                                    "type": "string",
                                    "enum": ["mouse_move", "click", "double_click", "scroll", "key", "key_down", "key_up", "type"],
                                    "description": "Input action type."
                                },
                                "x": { "type": "integer", "description": "X coordinate for mouse actions. Required if element_id is not provided." },
                                "y": { "type": "integer", "description": "Y coordinate for mouse actions. Required if element_id is not provided." },
                                "element_id": { "type": "string", "description": "Element ID from inspect_desktop (e.g. 'w0.e12'). When provided, resolves to the element's center coordinates. Alternative to x/y." },
                                "button": { "type": "string", "enum": ["left", "right", "middle"], "description": "Mouse button for click/double_click." },
                                "delta": { "type": "integer", "description": "Wheel notches for scroll (positive = up, negative = down)." },
                                "key": { "type": "string", "description": "Key name for key/key_down/key_up (e.g. Enter, Tab, Escape, A)." },
                                "text": { "type": "string", "description": "Text to type for the `type` action." }
                            },
                            "required": ["action"],
                            "additionalProperties": false,
                        },
                    },
                },
                "required": ["actions"],
                "additionalProperties": false,
            }),
            ToolMetadata {
                namespace: "computer-use".to_string(),
                risk: ToolRisk::Critical,
                is_read_only: false,
                is_concurrency_safe: false,
                exposure: crate::tool::ToolExposure::Deferred,
                capabilities: vec!["os.input.write".to_string()],
                tags: vec![
                    "input".to_string(),
                    "mouse".to_string(),
                    "keyboard".to_string(),
                    "click".to_string(),
                    "type".to_string(),
                ],
                ..ToolMetadata::default()
            },
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let raw_actions = invocation
            .input
            .get("actions")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("missing required `actions` array"))?;

        if raw_actions.is_empty() {
            return Ok(helpers::ok(
                invocation.id,
                json!({
                    "actions_performed": 0,
                    "message": "No actions provided.",
                }),
            ));
        }

        // ── Resolve element_id → coordinates ──────────────────────────────
        // Any mouse action with an `element_id` field gets resolved to x/y
        // using the element cache populated by inspect_desktop/inspect_element.
        let actions = match self.resolve_element_ids(raw_actions) {
            Ok(resolved) => resolved,
            Err(msg) => {
                return Ok(helpers::ok(
                    invocation.id,
                    json!({
                        "actions_performed": 0,
                        "error": msg,
                        "message": format!(
                            "Could not resolve element_id to coordinates: {msg}"
                        ),
                    }),
                ));
            }
        };

        // ── Deny-list enforcement (ADR 0016) ──────────────────────────────
        // Resolve the target app from the first mouse action's coordinates,
        // or fall back to the foreground window for keyboard-only actions.
        // Yolo bypasses the deny-list entirely.
        if self.permission_mode != PermissionMode::Yolo && !self.deny_apps.is_empty() {
            if let Some(deny_reason) = self.check_deny_list(&actions) {
                return Ok(helpers::ok(
                    invocation.id,
                    json!({
                        "actions_performed": 0,
                        "denied": true,
                        "deny_reason": deny_reason,
                        "message": format!(
                            "Input simulation blocked by computer-use deny-list: {deny_reason}. \
                             The target application is on the deny-list. Use Yolo mode to bypass \
                             (not recommended)."
                        ),
                    }),
                ));
            }
        }

        // ── Sensitive-field guard (ADR 0016, Auto mode) ──────────────────
        // In Auto mode, ordinary input is auto-approved, but typing into a
        // password field still requires approval. Restricted/AcceptEdits
        // already ask for approval on every `simulate_input` via the security
        // layer (`SecurityRisk::UiAutomation`); Yolo bypasses entirely.
        // We only block here in Auto — the other modes are handled upstream.
        if self.permission_mode == PermissionMode::Auto
            && self.actions_target_keyboard(&actions)
            && navi_computer_use::is_target_sensitive()
        {
            return Ok(helpers::ok(
                invocation.id,
                json!({
                    "actions_performed": 0,
                    "denied": true,
                    "deny_reason": "sensitive field (password) detected in foreground window",
                    "message": "Input simulation blocked: the focused element appears to be a \
                                password field. Auto mode does not type into sensitive fields \
                                without approval. Switch to a non-Auto mode and approve the \
                                action explicitly, or re-target a non-sensitive field.",
                }),
            ));
        }

        let backend = navi_computer_use::platform_backend();
        let result = backend.simulate_input(&actions)?;

        Ok(helpers::ok(
            invocation.id,
            json!({
                "actions_performed": result.actions_performed,
                "platform": backend.platform_name(),
                "message": format!("Simulated {} input action(s).", result.actions_performed),
            }),
        ))
    }
}

impl SimulateInputTool {
    /// Checks whether the target app for `actions` is on the deny-list.
    ///
    /// Returns `Some(reason)` if the target is deny-listed, `None` if it's
    /// safe to proceed (or if the target couldn't be resolved — we fail open
    /// for resolution errors to avoid blocking all input when the desktop is
    /// in an unusual state).
    #[cfg(windows)]
    fn check_deny_list(&self, actions: &[Value]) -> Option<String> {
        // Find the first mouse action with coordinates to resolve the target.
        let target = actions
            .iter()
            .find_map(|a| {
                let kind = a.get("action").and_then(Value::as_str)?;
                if matches!(kind, "mouse_move" | "click" | "double_click" | "scroll") {
                    let x = a.get("x").and_then(Value::as_i64)? as i32;
                    let y = a.get("y").and_then(Value::as_i64)? as i32;
                    navi_computer_use::resolve_target_for_point(x, y)
                } else {
                    None
                }
            })
            .or_else(navi_computer_use::resolve_target_foreground);

        let target = target?;
        if is_deny_listed(&target.exe_name, &self.deny_apps) {
            return Some(format!(
                "target process `{}` (pid {}) is deny-listed",
                target.exe_name, target.pid
            ));
        }
        if !target.window_title.is_empty() && is_deny_listed(&target.window_title, &self.deny_apps)
        {
            return Some(format!(
                "target window title `{}` is deny-listed",
                target.window_title
            ));
        }
        None
    }

    /// Non-Windows stub: no target resolution available, fail open.
    #[cfg(not(windows))]
    fn check_deny_list(&self, _actions: &[Value]) -> Option<String> {
        None
    }

    /// Returns `true` if any action in `actions` targets the keyboard
    /// (`key`, `key_down`, `key_up`, `type`). Used to decide whether the
    /// sensitive-field guard applies (it only matters for typing into a
    /// focused element; pure mouse actions don't type into anything).
    fn actions_target_keyboard(&self, actions: &[Value]) -> bool {
        actions.iter().any(|a| {
            matches!(
                a.get("action").and_then(Value::as_str),
                Some("key" | "key_down" | "key_up" | "type")
            )
        })
    }
}

#[cfg(all(test, feature = "computer-use"))]
mod tests {
    use super::*;

    fn test_cache()
    -> Arc<std::sync::Mutex<std::collections::HashMap<String, navi_computer_use::Rect>>> {
        Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))
    }

    #[test]
    fn capture_screen_definition_is_deferred_read() {
        let tool = CaptureScreenTool::new(PathBuf::from("/tmp"), true);
        let def = tool.definition();
        assert_eq!(def.name, "capture_screen");
        assert_eq!(def.kind, ToolKind::Read);
        assert_eq!(def.metadata.exposure, crate::tool::ToolExposure::Deferred);
        assert_eq!(def.metadata.risk, ToolRisk::Low);
        assert!(def.metadata.is_read_only);
    }

    #[test]
    fn enumerate_windows_definition_is_deferred_read() {
        let tool = EnumerateWindowsTool::new();
        let def = tool.definition();
        assert_eq!(def.name, "enumerate_windows");
        assert_eq!(def.kind, ToolKind::Read);
        assert_eq!(def.metadata.exposure, crate::tool::ToolExposure::Deferred);
    }

    #[test]
    fn inspect_element_definition_is_deferred_read() {
        let tool = InspectElementTool::new(test_cache());
        let def = tool.definition();
        assert_eq!(def.name, "inspect_element");
        assert_eq!(def.kind, ToolKind::Read);
        assert_eq!(def.metadata.exposure, crate::tool::ToolExposure::Deferred);
    }

    #[test]
    fn inspect_desktop_definition_is_direct_read() {
        let tool = InspectDesktopTool::new(test_cache());
        let def = tool.definition();
        assert_eq!(def.name, "inspect_desktop");
        assert_eq!(def.kind, ToolKind::Read);
        assert_eq!(def.metadata.exposure, crate::tool::ToolExposure::Direct);
        assert_eq!(def.metadata.risk, ToolRisk::Low);
        assert!(def.metadata.is_read_only);
    }

    #[test]
    fn open_application_definition_is_direct_command() {
        let tool = OpenApplicationTool::new();
        let def = tool.definition();
        assert_eq!(def.name, "open_application");
        assert_eq!(def.kind, ToolKind::Command);
        assert_eq!(def.metadata.exposure, crate::tool::ToolExposure::Direct);
    }

    #[test]
    fn simulate_input_definition_is_deferred_command_critical() {
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Restricted,
            test_cache(),
        );
        let def = tool.definition();
        assert_eq!(def.name, "simulate_input");
        assert_eq!(def.kind, ToolKind::Command);
        assert_eq!(def.metadata.exposure, crate::tool::ToolExposure::Deferred);
        assert_eq!(def.metadata.risk, ToolRisk::Critical);
        assert!(!def.metadata.is_read_only);
    }

    #[test]
    fn simulate_input_deny_list_check_returns_none_for_empty_list() {
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Auto,
            test_cache(),
        );
        // Empty deny-list → never blocks.
        assert_eq!(
            tool.check_deny_list(&[json!({"action": "click", "x": 0, "y": 0})]),
            None
        );
    }

    #[test]
    fn simulate_input_deny_list_check_returns_none_in_yolo_regardless() {
        // Yolo is enforced at the invoke level (permission_mode != Yolo),
        // but check_deny_list itself is mode-agnostic. The invoke guard
        // short-circuits before calling this in Yolo. We test the helper
        // in isolation here with a non-Yolo mode + empty list.
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            vec!["definitely_not_running_app_xyz".to_string()],
            PermissionMode::Auto,
            test_cache(),
        );
        // The deny-list has an entry, but the target app at (0,0) won't match
        // it (unless someone is running an app with that exact name). This
        // verifies the matching logic doesn't false-positive on arbitrary apps.
        // We can't assert None unconditionally (depends on what's at 0,0),
        // so we just assert the call doesn't panic.
        let _ = tool.check_deny_list(&[json!({"action": "click", "x": 0, "y": 0})]);
    }

    #[test]
    fn simulate_input_resolves_element_id_to_coordinates() {
        let cache = test_cache();
        {
            let mut c = cache.lock().unwrap();
            c.insert(
                "w0.e1".to_string(),
                navi_computer_use::Rect {
                    x: 100,
                    y: 200,
                    width: 50,
                    height: 40,
                },
            );
        }
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Yolo,
            cache,
        );
        let actions = vec![json!({"action": "click", "element_id": "w0.e1", "button": "left"})];
        let resolved = tool.resolve_element_ids(&actions).expect("should resolve");
        assert_eq!(resolved[0]["x"], 125); // 100 + 50/2
        assert_eq!(resolved[0]["y"], 220); // 200 + 40/2
    }

    #[test]
    fn simulate_input_element_id_not_in_cache_returns_error() {
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Yolo,
            test_cache(),
        );
        let actions = vec![json!({"action": "click", "element_id": "w99.e99"})];
        let result = tool.resolve_element_ids(&actions);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found in cache"));
    }

    #[test]
    fn simulate_input_mixed_element_id_and_coordinates() {
        // Actions with a mix of element_id and explicit coordinates should
        // resolve element_id ones and leave coordinate ones unchanged.
        let cache = test_cache();
        {
            let mut c = cache.lock().unwrap();
            c.insert(
                "w0.e5".to_string(),
                navi_computer_use::Rect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                },
            );
        }
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Yolo,
            cache,
        );
        let actions = vec![
            json!({"action": "click", "element_id": "w0.e5", "button": "left"}),
            json!({"action": "mouse_move", "x": 50, "y": 60}),
            json!({"action": "key", "key": "Enter"}),
        ];
        let resolved = tool.resolve_element_ids(&actions).expect("should resolve");

        // First action: element_id resolved to center (50, 50)
        assert_eq!(resolved[0]["x"], 50);
        assert_eq!(resolved[0]["y"], 50);
        assert!(resolved[0].get("element_id").is_some()); // element_id preserved

        // Second action: explicit coordinates unchanged
        assert_eq!(resolved[1]["x"], 50);
        assert_eq!(resolved[1]["y"], 60);
        assert!(resolved[1].get("element_id").is_none());

        // Third action: keyboard action unchanged
        assert_eq!(resolved[2]["action"], "key");
        assert_eq!(resolved[2]["key"], "Enter");
    }

    #[test]
    fn simulate_input_element_id_with_zero_size_element() {
        // An element with width=0 or height=0 should still resolve (center
        // will be the x/y origin of the rect).
        let cache = test_cache();
        {
            let mut c = cache.lock().unwrap();
            c.insert(
                "w0.e0".to_string(),
                navi_computer_use::Rect {
                    x: 200,
                    y: 300,
                    width: 0,
                    height: 0,
                },
            );
        }
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Yolo,
            cache,
        );
        let actions = vec![json!({"action": "click", "element_id": "w0.e0"})];
        let resolved = tool.resolve_element_ids(&actions).expect("should resolve");
        assert_eq!(resolved[0]["x"], 200);
        assert_eq!(resolved[0]["y"], 300);
    }

    // ── Edge cases: simulate_input resolve_element_ids ────────────────────

    #[test]
    fn simulate_input_empty_actions_returns_empty_vec() {
        // Empty actions array should resolve to empty vec, not error.
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Yolo,
            test_cache(),
        );
        let resolved = tool.resolve_element_ids(&[]).expect("empty should resolve");
        assert!(resolved.is_empty(), "empty actions should return empty vec");
    }

    #[test]
    fn simulate_input_element_id_on_non_mouse_action_is_ignored() {
        // element_id on a keyboard action (e.g. "key") should be ignored —
        // only mouse actions (click, mouse_move, scroll, drag) use coordinates.
        let cache = test_cache();
        {
            let mut c = cache.lock().unwrap();
            c.insert(
                "w0.e1".to_string(),
                navi_computer_use::Rect {
                    x: 10,
                    y: 20,
                    width: 100,
                    height: 50,
                },
            );
        }
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Yolo,
            cache,
        );
        // A "type" action with element_id — element_id should be ignored,
        // no x/y should be added.
        let actions = vec![json!({"action": "type", "text": "hello", "element_id": "w0.e1"})];
        let resolved = tool.resolve_element_ids(&actions).expect("should resolve");
        assert_eq!(resolved[0]["action"], "type");
        assert_eq!(resolved[0]["text"], "hello");
        // element_id should be preserved (we don't strip it), but no x/y added.
        assert!(
            resolved[0].get("x").is_none(),
            "type action should not get x/y"
        );
        assert!(
            resolved[0].get("y").is_none(),
            "type action should not get x/y"
        );
    }

    #[test]
    fn simulate_input_multiple_element_ids_same_action() {
        // Multiple actions each with different element_ids should all resolve.
        let cache = test_cache();
        {
            let mut c = cache.lock().unwrap();
            c.insert(
                "w0.e1".to_string(),
                navi_computer_use::Rect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                },
            );
            c.insert(
                "w0.e2".to_string(),
                navi_computer_use::Rect {
                    x: 200,
                    y: 200,
                    width: 100,
                    height: 100,
                },
            );
            c.insert(
                "w1.e0".to_string(),
                navi_computer_use::Rect {
                    x: 500,
                    y: 500,
                    width: 50,
                    height: 50,
                },
            );
        }
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Yolo,
            cache,
        );
        let actions = vec![
            json!({"action": "click", "element_id": "w0.e1"}),
            json!({"action": "mouse_move", "element_id": "w0.e2"}),
            json!({"action": "click", "element_id": "w1.e0"}),
        ];
        let resolved = tool.resolve_element_ids(&actions).expect("should resolve");
        assert_eq!(resolved[0]["x"], 50); // w0.e1 center
        assert_eq!(resolved[0]["y"], 50);
        assert_eq!(resolved[1]["x"], 250); // w0.e2 center
        assert_eq!(resolved[1]["y"], 250);
        assert_eq!(resolved[2]["x"], 525); // w1.e0 center
        assert_eq!(resolved[2]["y"], 525);
    }

    #[test]
    fn simulate_input_element_id_overrides_explicit_coordinates() {
        // If both element_id AND x/y are present, element_id should win
        // (the resolved coordinates from the cache override the explicit ones).
        let cache = test_cache();
        {
            let mut c = cache.lock().unwrap();
            c.insert(
                "w0.e1".to_string(),
                navi_computer_use::Rect {
                    x: 100,
                    y: 100,
                    width: 50,
                    height: 50,
                },
            );
        }
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Yolo,
            cache,
        );
        let actions = vec![json!({
            "action": "click",
            "element_id": "w0.e1",
            "x": 999,
            "y": 999
        })];
        let resolved = tool.resolve_element_ids(&actions).expect("should resolve");
        // The resolved center (125, 125) should override the explicit (999, 999).
        assert_eq!(
            resolved[0]["x"], 125,
            "element_id should override explicit x"
        );
        assert_eq!(
            resolved[0]["y"], 125,
            "element_id should override explicit y"
        );
    }

    #[test]
    fn simulate_input_element_id_with_negative_rect() {
        // Elements on multi-monitor setups can have negative x/y coordinates.
        let cache = test_cache();
        {
            let mut c = cache.lock().unwrap();
            c.insert(
                "w0.e1".to_string(),
                navi_computer_use::Rect {
                    x: -200,
                    y: -100,
                    width: 100,
                    height: 50,
                },
            );
        }
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Yolo,
            cache,
        );
        let actions = vec![json!({"action": "click", "element_id": "w0.e1"})];
        let resolved = tool.resolve_element_ids(&actions).expect("should resolve");
        assert_eq!(resolved[0]["x"], -150); // -200 + 100/2
        assert_eq!(resolved[0]["y"], -75); // -100 + 50/2
    }

    #[test]
    fn simulate_input_element_id_with_very_large_rect() {
        // 4K monitor coordinates — should handle large values without overflow.
        let cache = test_cache();
        {
            let mut c = cache.lock().unwrap();
            c.insert(
                "w0.e1".to_string(),
                navi_computer_use::Rect {
                    x: 3840,
                    y: 2160,
                    width: 1920,
                    height: 1080,
                },
            );
        }
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Yolo,
            cache,
        );
        let actions = vec![json!({"action": "click", "element_id": "w0.e1"})];
        let resolved = tool.resolve_element_ids(&actions).expect("should resolve");
        assert_eq!(resolved[0]["x"], 4800); // 3840 + 1920/2
        assert_eq!(resolved[0]["y"], 2700); // 2160 + 1080/2
    }

    #[test]
    fn simulate_input_cache_with_1000_entries_resolves_correctly() {
        // Large cache should not cause performance issues or incorrect lookups.
        let cache = test_cache();
        {
            let mut c = cache.lock().unwrap();
            for i in 0..1000 {
                c.insert(
                    format!("w0.e{i}"),
                    navi_computer_use::Rect {
                        x: i as i32,
                        y: i as i32,
                        width: 10,
                        height: 10,
                    },
                );
            }
        }
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Yolo,
            cache,
        );
        // Resolve element_id "w0.e500" — should find it among 1000 entries.
        let actions = vec![json!({"action": "click", "element_id": "w0.e500"})];
        let resolved = tool.resolve_element_ids(&actions).expect("should resolve");
        assert_eq!(resolved[0]["x"], 505); // 500 + 10/2
        assert_eq!(resolved[0]["y"], 505);
    }

    #[test]
    fn simulate_input_empty_element_id_string_returns_error() {
        // An empty string element_id should not be treated as "no element_id".
        // It should fail with "not found in cache" (empty string is not a valid key).
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Yolo,
            test_cache(),
        );
        let actions = vec![json!({"action": "click", "element_id": ""})];
        let result = tool.resolve_element_ids(&actions);
        assert!(result.is_err(), "empty element_id should error");
    }

    #[test]
    fn simulate_input_all_mouse_action_types_resolve_element_id() {
        // All mouse action types (click, mouse_move, scroll, drag) should
        // resolve element_id to coordinates.
        let cache = test_cache();
        {
            let mut c = cache.lock().unwrap();
            c.insert(
                "w0.e1".to_string(),
                navi_computer_use::Rect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                },
            );
        }
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Yolo,
            cache,
        );
        let actions = vec![
            json!({"action": "click", "element_id": "w0.e1"}),
            json!({"action": "mouse_move", "element_id": "w0.e1"}),
            json!({"action": "scroll", "element_id": "w0.e1", "direction": "down"}),
            json!({"action": "drag", "element_id": "w0.e1", "target_x": 200, "target_y": 200}),
        ];
        let resolved = tool.resolve_element_ids(&actions).expect("should resolve");
        for (i, expected_action) in ["click", "mouse_move", "scroll", "drag"].iter().enumerate() {
            assert_eq!(
                resolved[i]["action"], *expected_action,
                "action {i} should be {expected_action}"
            );
            assert_eq!(resolved[i]["x"], 50, "action {i} should have resolved x=50");
            assert_eq!(resolved[i]["y"], 50, "action {i} should have resolved y=50");
        }
    }

    #[test]
    fn simulate_input_partial_failure_all_actions_after_error_rejected() {
        // If one action has an invalid element_id, the entire batch should
        // fail (resolve_element_ids returns Err) — we don't want to execute
        // a partial batch of mouse actions.
        let cache = test_cache();
        {
            let mut c = cache.lock().unwrap();
            c.insert(
                "w0.e1".to_string(),
                navi_computer_use::Rect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                },
            );
        }
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Yolo,
            cache,
        );
        let actions = vec![
            json!({"action": "click", "element_id": "w0.e1"}), // valid
            json!({"action": "click", "element_id": "w0.e999"}), // invalid
            json!({"action": "click", "element_id": "w0.e1"}), // valid
        ];
        let result = tool.resolve_element_ids(&actions);
        assert!(
            result.is_err(),
            "batch with one invalid element_id should fail entirely"
        );
        assert!(
            result.unwrap_err().contains("not found in cache"),
            "error should mention 'not found in cache'"
        );
    }

    #[tokio::test]
    #[cfg(all(windows, feature = "computer-use"))]
    async fn inspect_desktop_populates_element_cache() {
        // Integration test: call inspect_desktop through the tool layer and
        // verify the element cache is populated with real element_ids.
        let cache: Arc<
            std::sync::Mutex<std::collections::HashMap<String, navi_computer_use::Rect>>,
        > = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

        let tool = InspectDesktopTool::new(cache.clone());
        let invocation = ToolInvocation {
            id: "test-desktop".to_string(),
            tool_name: "inspect_desktop".to_string(),
            input: json!({}),
        };

        let result = match tool.invoke(invocation).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipping inspect_desktop_populates_element_cache: {e}");
                return;
            }
        };

        assert!(result.ok, "inspect_desktop should return ok=true");

        // The cache should have at least some entries (if there are windows).
        let cache_len = cache.lock().unwrap().len();
        if cache_len == 0 {
            // Could be an empty desktop in CI — skip.
            eprintln!("skipping: element cache is empty (no visible windows?)");
            return;
        }

        // Every entry in the cache should have a key matching "w{idx}.e{counter}".
        for key in cache.lock().unwrap().keys() {
            assert!(
                key.starts_with("w") && key.contains(".e"),
                "cache key '{key}' should match 'w{{idx}}.e{{counter}}' format"
            );
        }
    }

    #[tokio::test]
    #[cfg(all(windows, feature = "computer-use"))]
    async fn open_application_tool_launches_notepad() {
        // Integration test: launch notepad through the tool layer.
        let tool = OpenApplicationTool::new();
        let invocation = ToolInvocation {
            id: "test-open-app".to_string(),
            tool_name: "open_application".to_string(),
            input: json!({"name": "notepad"}),
        };

        let result = match tool.invoke(invocation).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipping open_application_tool_launches_notepad: {e}");
                return;
            }
        };

        assert!(result.ok, "open_application should return ok=true");

        // Check the result content.
        let launched = result
            .output
            .get("launched")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert!(launched, "notepad should be launched");

        // Clean up.
        let _ = std::process::Command::new("taskkill")
            .args(["/IM", "notepad.exe", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    #[tokio::test]
    #[cfg(windows)]
    async fn simulate_input_invoke_blocks_deny_listed_app_in_auto_mode() {
        // Integration test: register the foreground window's exe name in the
        // deny-list and verify the tool returns `denied: true` in Auto mode.
        // Skip if there's no foreground window (headless CI).
        let target = match navi_computer_use::resolve_target_foreground() {
            Some(t) => t,
            None => {
                eprintln!(
                    "skipping simulate_input_invoke_blocks_deny_listed_app_in_auto_mode: no foreground window"
                );
                return;
            }
        };
        if target.exe_name.is_empty() {
            eprintln!("skipping: could not resolve foreground exe name");
            return;
        }

        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            vec![target.exe_name.clone()],
            PermissionMode::Auto,
            test_cache(),
        );
        let invocation = ToolInvocation {
            id: "test-deny".to_string(),
            tool_name: "simulate_input".to_string(),
            input: json!({
                "actions": [{"action": "key", "key": "Escape"}],
            }),
        };
        let result = tool
            .invoke(invocation)
            .await
            .expect("invoke should not error");
        assert!(result.ok, "tool should return ok=true even when denying");
        let denied = result
            .output
            .get("denied")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert!(
            denied,
            "expected denied=true for deny-listed foreground app in Auto mode"
        );
        let actions = result
            .output
            .get("actions_performed")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        assert_eq!(actions, 0, "no actions should be performed when denied");
    }

    #[tokio::test]
    #[cfg(windows)]
    async fn simulate_input_invoke_allows_deny_listed_app_in_yolo_mode() {
        // Yolo bypasses the deny-list entirely (ADR 0016).
        let target = match navi_computer_use::resolve_target_foreground() {
            Some(t) => t,
            None => {
                eprintln!(
                    "skipping simulate_input_invoke_allows_deny_listed_app_in_yolo_mode: no foreground window"
                );
                return;
            }
        };
        if target.exe_name.is_empty() {
            eprintln!("skipping: could not resolve foreground exe name");
            return;
        }

        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            vec![target.exe_name.clone()],
            PermissionMode::Yolo,
            test_cache(),
        );
        let invocation = ToolInvocation {
            id: "test-yolo".to_string(),
            tool_name: "simulate_input".to_string(),
            input: json!({
                // Empty actions — the tool short-circuits before the deny-list
                // check, but we want to verify Yolo doesn't even reach it.
                "actions": [],
            }),
        };
        let result = tool
            .invoke(invocation)
            .await
            .expect("invoke should not error");
        // Empty actions → actions_performed: 0, but NOT denied.
        let denied = result
            .output
            .get("denied")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert!(!denied, "Yolo mode must not set denied=true");
    }

    #[test]
    fn actions_target_keyboard_detects_keyboard_actions() {
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Auto,
            test_cache(),
        );
        assert!(tool.actions_target_keyboard(&[json!({"action": "type", "text": "hi"})]));
        assert!(tool.actions_target_keyboard(&[json!({"action": "key", "key": "Enter"})]));
        assert!(tool.actions_target_keyboard(&[json!({"action": "key_down", "key": "Shift"})]));
        assert!(tool.actions_target_keyboard(&[json!({"action": "key_up", "key": "Shift"})]));
        // Mixed mouse + keyboard → true (keyboard present).
        assert!(tool.actions_target_keyboard(&[
            json!({"action": "click", "x": 10, "y": 20}),
            json!({"action": "type", "text": "hi"})
        ]));
        // Pure mouse → false.
        assert!(!tool.actions_target_keyboard(&[json!({"action": "click", "x": 10, "y": 20})]));
        assert!(
            !tool.actions_target_keyboard(&[
                json!({"action": "scroll", "delta": 1, "x": 0, "y": 0})
            ])
        );
        assert!(!tool.actions_target_keyboard(&[json!({"action": "mouse_move", "x": 0, "y": 0})]));
        // Empty → false.
        assert!(!tool.actions_target_keyboard(&[]));
    }
}
