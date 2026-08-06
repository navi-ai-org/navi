//! Windows OS automation backend for NAVI computer use.
//!
//! Pure Win32 FFI layer — no trait dependencies. Exports free functions and
//! plain data structs. The [`navi_computer_use`] facade wraps these into the
//! [`ComputerUseBackend`] trait.
//!
//! Implements:
//! - Screen capture via GDI `BitBlt` + `GetDIBits` (BMP file output).
//! - Window enumeration via `EnumWindows`.
//! - Input simulation via `SendInput`.
//! - Element inspection is stubbed in this spike (requires UIA COM — see ADR 0016).
//!
//! On non-Windows targets this crate compiles but all operations return
//! [`UNSUPPORTED_PLATFORM`].

#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

#[cfg(not(windows))]
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Error returned on platforms without a compiled backend.
pub const UNSUPPORTED_PLATFORM: &str = "navi-os-windows: not running on Windows";

// ── Plain data structs (no trait, no facade dependency) ─────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WinScreenshot {
    /// Absolute path to the saved BMP file.
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WinRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WinWindowInfo {
    /// Window handle as an unsigned integer.
    pub hwnd: u64,
    pub title: String,
    pub pid: u32,
    pub rect: WinRect,
    pub is_focused: bool,
    pub is_visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WinInputResult {
    pub actions_performed: usize,
}

/// Resolved target app for the computer-use deny-list (ADR 0016).
///
/// `exe_name` is lowercase without `.exe` (e.g. `"1password"`).
/// `window_title` is the raw window title (may be empty for background windows).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WinTargetApp {
    pub pid: u32,
    pub exe_name: String,
    pub window_title: String,
}

/// Plain data mirror of `navi_computer_use::ElementInfo`.
///
/// Kept local to this leaf crate so it has no dependency on the facade —
/// the `WindowsBackendAdapter` converts `WinElementInfo` → `ElementInfo`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WinElementInfo {
    pub name: String,
    pub control_type: String,
    pub value: Option<String>,
    pub rect: Option<WinRect>,
    pub is_password: bool,
    pub children: Vec<WinElementInfo>,
    /// `true` if this node's children were truncated to avoid huge trees
    /// (Electron/VS Code can have thousands of nodes).
    #[serde(default, skip_serializing_if = "is_false")]
    pub children_truncated: bool,
}

fn is_false(b: &bool) -> bool {
    !b
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WinElementTree {
    pub root: WinElementInfo,
    pub supported: bool,
}

/// Options for `inspect_element`. Mirrors `navi_computer_use::InspectOptions`
/// without the facade dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WinInspectOptions {
    /// Window handle to inspect (None = foreground).
    pub window: Option<u64>,
    /// Max tree depth (0 = root only).
    pub max_depth: u32,
}

impl Default for WinInspectOptions {
    fn default() -> Self {
        Self {
            window: None,
            max_depth: 3,
        }
    }
}

// ── Non-Windows stubs ───────────────────────────────────────────────────────

#[cfg(not(windows))]
pub fn capture_screen(_out_dir: &str) -> Result<WinScreenshot> {
    bail!(UNSUPPORTED_PLATFORM)
}

#[cfg(not(windows))]
pub fn enumerate_windows() -> Result<Vec<WinWindowInfo>> {
    bail!(UNSUPPORTED_PLATFORM)
}

#[cfg(not(windows))]
pub fn inspect_element(_opts: &WinInspectOptions) -> Result<WinElementTree> {
    bail!(UNSUPPORTED_PLATFORM)
}

#[cfg(not(windows))]
pub fn simulate_input(_actions: &[serde_json::Value]) -> Result<WinInputResult> {
    bail!(UNSUPPORTED_PLATFORM)
}

// ── Windows implementation ─────────────────────────────────────────────────

#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod com;
#[cfg(windows)]
mod input;
#[cfg(windows)]
mod inspect;
#[cfg(windows)]
mod target;
#[cfg(windows)]
mod windows_list;

#[cfg(windows)]
pub use capture::capture_screen;
#[cfg(windows)]
pub use com::ensure_com_initialized;
#[cfg(windows)]
pub use input::simulate_input;
#[cfg(windows)]
pub use inspect::inspect_element;
#[cfg(windows)]
pub use target::{resolve_target_for_point, resolve_target_foreground};
#[cfg(windows)]
pub use windows_list::enumerate_windows;
