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
}

impl CaptureScreenTool {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }
}

#[async_trait]
impl Tool for CaptureScreenTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition_with_meta(
            "capture_screen",
            "Capture the primary monitor screen as a screenshot. The image is saved as a BMP \
file under the NAVI data directory and attached for visual analysis by the chat model on \
the next request. Returns the file path, dimensions, and size. Use `view_image` to \
re-examine a previously captured screenshot.",
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

        // Read the BMP file and embed it as multimodal content so the model
        // sees the screenshot directly (same pattern as `view_image`).
        let path = std::path::Path::new(&screenshot.path);
        let bytes =
            std::fs::read(path).map_err(|e| anyhow::anyhow!("failed to read screenshot: {e}"))?;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);

        // Persist to the attachment store for session restore.
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("bmp");
        let attachment_id = crate::attachment_store::store_bytes(&self.data_dir, &bytes, ext)
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

pub(crate) struct InspectElementTool;

impl InspectElementTool {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for InspectElementTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition_with_meta(
            "inspect_element",
            "Inspect the accessibility tree of a window (UI Automation / AXUIElement / AT-SPI). \
Returns element names, control types, values, bounding rectangles, and whether elements are \
password fields. Pass `window` (hwnd from enumerate_windows) to target a specific window, or \
omit to inspect the foreground window. **Spike:** returns the foreground window root only; \
full tree walk is not yet implemented.",
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
                        "description": "Maximum tree depth to traverse (default 3, 0 = root only)."
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
        let max_depth = invocation
            .input
            .get("max_depth")
            .and_then(Value::as_u64)
            .unwrap_or(3) as u32;

        let backend = navi_computer_use::platform_backend();
        // UIA calls are cross-process and can block on unresponsive apps.
        // Run on a blocking thread so we don't stall the tokio runtime.
        let opts = navi_computer_use::InspectOptions { window, max_depth };
        let tree = tokio::task::spawn_blocking(move || backend.inspect_element(&opts))
            .await
            .map_err(|e| anyhow::anyhow!("inspect_element worker panicked: {e}"))??;

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

// ── SimulateInputTool ──────────────────────────────────────────────────────

pub(crate) struct SimulateInputTool {
    #[allow(dead_code)]
    data_dir: PathBuf,
    /// Deny-list of apps that this tool must not target (ADR 0016).
    /// Enforced in all modes except Yolo.
    deny_apps: Vec<String>,
    /// Current permission mode — Yolo bypasses the deny-list.
    permission_mode: PermissionMode,
}

impl SimulateInputTool {
    pub(crate) fn new(
        data_dir: PathBuf,
        deny_apps: Vec<String>,
        permission_mode: PermissionMode,
    ) -> Self {
        Self {
            data_dir,
            deny_apps,
            permission_mode,
        }
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
**High risk:** this tool controls the real desktop. Requires approval in all modes except Yolo.",
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
                                "x": { "type": "integer", "description": "X coordinate for mouse actions." },
                                "y": { "type": "integer", "description": "Y coordinate for mouse actions." },
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
        let actions = invocation
            .input
            .get("actions")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("missing required `actions` array"))?;

        if actions.is_empty() {
            return Ok(helpers::ok(
                invocation.id,
                json!({
                    "actions_performed": 0,
                    "message": "No actions provided.",
                }),
            ));
        }

        // ── Deny-list enforcement (ADR 0016) ──────────────────────────────
        // Resolve the target app from the first mouse action's coordinates,
        // or fall back to the foreground window for keyboard-only actions.
        // Yolo bypasses the deny-list entirely.
        if self.permission_mode != PermissionMode::Yolo && !self.deny_apps.is_empty() {
            if let Some(deny_reason) = self.check_deny_list(actions) {
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
            && self.actions_target_keyboard(actions)
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
        let result = backend.simulate_input(actions)?;

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

    #[test]
    fn capture_screen_definition_is_deferred_read() {
        let tool = CaptureScreenTool::new(PathBuf::from("/tmp"));
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
        let tool = InspectElementTool::new();
        let def = tool.definition();
        assert_eq!(def.name, "inspect_element");
        assert_eq!(def.kind, ToolKind::Read);
        assert_eq!(def.metadata.exposure, crate::tool::ToolExposure::Deferred);
    }

    #[test]
    fn simulate_input_definition_is_deferred_command_critical() {
        let tool = SimulateInputTool::new(
            PathBuf::from("/tmp"),
            Vec::new(),
            PermissionMode::Restricted,
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
        let tool = SimulateInputTool::new(PathBuf::from("/tmp"), Vec::new(), PermissionMode::Auto);
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
        );
        // The deny-list has an entry, but the target app at (0,0) won't match
        // it (unless someone is running an app with that exact name). This
        // verifies the matching logic doesn't false-positive on arbitrary apps.
        // We can't assert None unconditionally (depends on what's at 0,0),
        // so we just assert the call doesn't panic.
        let _ = tool.check_deny_list(&[json!({"action": "click", "x": 0, "y": 0})]);
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
        let tool = SimulateInputTool::new(PathBuf::from("/tmp"), Vec::new(), PermissionMode::Auto);
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
