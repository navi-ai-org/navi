//! macOS OS automation backend for NAVI computer use (ADR 0016).
//!
//! Mirrors the `navi-os-windows` pattern: defines `Mac*` types that the
//! `navi-computer-use` facade adapter converts to platform-agnostic types.
//! This crate has NO dependency on the facade.
//!
//! ## APIs used
//!
//! | Function | macOS API | Permission |
//! |----------|-----------|------------|
//! | `capture_screen` | `CGDisplayCreateImage` | Screen Recording |
//! | `enumerate_windows` | `CGWindowListCopyWindowInfo` | none |
//! | `inspect_element` | `AXUIElement` | Accessibility |
//! | `simulate_input` | `CGEvent` | Input Monitoring / Accessibility |
//! | `is_accessibility_trusted` | `AXIsProcessTrusted()` | — |
//!
//! On non-macOS targets all functions return an error.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Error returned on platforms without a compiled backend.
pub const UNSUPPORTED_PLATFORM: &str = "navi-os-macos: not compiled for this OS";

// ── Data types (mirror facade types with Mac prefix) ───────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacScreenshot {
    /// Absolute path to the saved image file (PNG on macOS).
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacWindowInfo {
    /// Window number (CGWindowNumber) as unsigned integer.
    pub hwnd: u64,
    pub title: String,
    pub pid: u32,
    pub rect: MacRect,
    pub is_focused: bool,
    pub is_visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacInputResult {
    pub actions_performed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacTargetApp {
    pub pid: u32,
    /// Lowercase app name without `.app` suffix.
    pub exe_name: String,
    pub window_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacElementInfo {
    /// Stable element identifier (e.g. "w0.e12") assigned during tree walk.
    /// Use with `inspect_element` (drill-down) or `simulate_input` (click by ID).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_id: Option<String>,
    pub name: String,
    pub control_type: String,
    pub value: Option<String>,
    pub rect: Option<MacRect>,
    pub is_password: bool,
    pub children: Vec<MacElementInfo>,
    pub children_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacElementTree {
    pub root: MacElementInfo,
    pub supported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacInspectOptions {
    /// Window number (from enumerate_windows) or None for foreground.
    pub window: Option<u64>,
    /// Element ID to drill down from (e.g. "w0.e12"). When set, the backend
    /// walks the sub-tree starting from this element instead of the window
    /// root. If both `window` and `element_id` are set, `element_id` wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_id: Option<String>,
    /// Max tree depth (0 = root only).
    pub max_depth: u32,
    /// Use RawView (deeper, enters Electron/Chromium apps) instead of
    /// ControlView (logical UI tree). Default false.
    /// NOTE: macOS AXUIElement always exposes the full tree — there is no
    /// ControlView/RawView distinction — so this field is accepted for API
    /// compatibility but has no effect on macOS.
    #[serde(default)]
    pub raw_view: bool,
}

impl Default for MacInspectOptions {
    fn default() -> Self {
        Self {
            window: None,
            element_id: None,
            max_depth: 3,
            raw_view: false,
        }
    }
}

/// One window in a [`MacDesktopSnapshot`], with its shallow element tree.
///
/// Mirrors `navi_computer_use::WindowSnapshot` without the facade dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacWindowSnapshot {
    /// Stable window identifier (e.g. "w0", "w1").
    pub window_id: String,
    /// Platform-specific window handle (CGWindowNumber).
    pub hwnd: u64,
    pub title: String,
    pub pid: u32,
    pub rect: MacRect,
    pub is_focused: bool,
    /// Shallow element tree (depth 2: window → panels → controls).
    pub elements: Vec<MacElementInfo>,
}

/// Shallow snapshot of all visible windows and their top-level UI elements.
///
/// Mirrors `navi_computer_use::DesktopSnapshot` (minus the `platform` field,
/// which the facade adds) without the facade dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacDesktopSnapshot {
    pub windows: Vec<MacWindowSnapshot>,
}

/// Result of `open_application`. Mirrors `navi_computer_use::OpenAppResult`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacOpenAppResult {
    /// Whether the application was successfully launched.
    pub launched: bool,
    /// Process ID if available (0 if unknown — `open` doesn't return a PID).
    pub pid: u32,
    /// Human-readable description of what happened.
    pub message: String,
}

// ── Platform-specific implementations ──────────────────────────────────────

#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod input;
#[cfg(target_os = "macos")]
mod inspect;
#[cfg(target_os = "macos")]
mod open_app;
#[cfg(target_os = "macos")]
mod permission;
#[cfg(target_os = "macos")]
mod target;
#[cfg(target_os = "macos")]
mod windows_list;

// ── Public API ─────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
pub use capture::capture_screen;
#[cfg(target_os = "macos")]
pub use input::simulate_input;
#[cfg(target_os = "macos")]
pub use inspect::{inspect_desktop, inspect_element};
#[cfg(target_os = "macos")]
pub use open_app::open_application;
#[cfg(target_os = "macos")]
pub use permission::is_accessibility_trusted;
#[cfg(target_os = "macos")]
pub use target::{resolve_target_for_point, resolve_target_foreground};
#[cfg(target_os = "macos")]
pub use windows_list::enumerate_windows;

// ── Non-macOS stubs ────────────────────────────────────────────────────────

#[cfg(not(target_os = "macos"))]
pub fn capture_screen(_out_dir: &str) -> Result<MacScreenshot> {
    bail!(UNSUPPORTED_PLATFORM)
}

#[cfg(not(target_os = "macos"))]
pub fn enumerate_windows() -> Result<Vec<MacWindowInfo>> {
    bail!(UNSUPPORTED_PLATFORM)
}

#[cfg(not(target_os = "macos"))]
pub fn inspect_element(_opts: &MacInspectOptions) -> Result<MacElementTree> {
    bail!(UNSUPPORTED_PLATFORM)
}

#[cfg(not(target_os = "macos"))]
pub fn inspect_desktop() -> Result<MacDesktopSnapshot> {
    bail!(UNSUPPORTED_PLATFORM)
}

#[cfg(not(target_os = "macos"))]
pub fn open_application(_name: &str) -> Result<MacOpenAppResult> {
    bail!(UNSUPPORTED_PLATFORM)
}

#[cfg(not(target_os = "macos"))]
pub fn simulate_input(_actions: &[serde_json::Value]) -> Result<MacInputResult> {
    bail!(UNSUPPORTED_PLATFORM)
}

#[cfg(not(target_os = "macos"))]
pub fn resolve_target_for_point(_x: i32, _y: i32) -> Option<MacTargetApp> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn resolve_target_foreground() -> Option<MacTargetApp> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn is_accessibility_trusted() -> bool {
    false
}
