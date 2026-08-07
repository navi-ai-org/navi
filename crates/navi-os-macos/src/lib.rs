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
    /// Max tree depth (0 = root only).
    pub max_depth: u32,
}

impl Default for MacInspectOptions {
    fn default() -> Self {
        Self {
            window: None,
            max_depth: 3,
        }
    }
}

// ── Platform-specific implementations ──────────────────────────────────────

#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod input;
#[cfg(target_os = "macos")]
mod inspect;
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
pub use inspect::inspect_element;
#[cfg(target_os = "macos")]
pub use permission::is_accessibility_trusted;
#[cfg(target_os = "macos")]
pub use target::{resolve_target_for_point, resolve_target_foreground};
#[cfg(target_os = "macos")]
pub use windows_list::enumerate_windows;

// ── Non-macOS stubs ────────────────────────────────────────────────────────

#[cfg(not(target_os = "macos"))]
pub fn capture_screen(_out_dir: &str) -> Result<MacScreenshot> {
    bail!("navi-os-macos: not compiled for this OS")
}

#[cfg(not(target_os = "macos"))]
pub fn enumerate_windows() -> Result<Vec<MacWindowInfo>> {
    bail!("navi-os-macos: not compiled for this OS")
}

#[cfg(not(target_os = "macos"))]
pub fn inspect_element(_opts: &MacInspectOptions) -> Result<MacElementTree> {
    bail!("navi-os-macos: not compiled for this OS")
}

#[cfg(not(target_os = "macos"))]
pub fn simulate_input(_actions: &[serde_json::Value]) -> Result<MacInputResult> {
    bail!("navi-os-macos: not compiled for this OS")
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
