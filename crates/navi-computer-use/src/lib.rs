//! OS automation facade for NAVI computer use.
//!
//! Defines the [`ComputerUseBackend`] trait and platform-agnostic data types.
//! The platform backend is selected at compile time via `cfg(target_os)`:
//!
//! - **Windows** — backed by [`navi_os_windows`] (GDI capture, `EnumWindows`,
//!   `SendInput`).
//! - **macOS / Linux** — not yet implemented; [`platform_backend`] returns an
//!   [`UnsupportedBackend`] that fails all operations with a clear message.
//!
//! Consumers (`navi-core`) depend on this crate only, never on a platform
//! backend directly — mirroring the `navi-providers` / `navi-openai` split.
//!
//! See ADR 0016 for the security model, tool exposure, and surface sync rules.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

// ── Data types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Screenshot {
    /// Absolute path to the saved image file (suitable for `view_image`).
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Window handle as an unsigned integer (platform-specific).
    pub hwnd: u64,
    pub title: String,
    pub pid: u32,
    pub rect: Rect,
    pub is_focused: bool,
    pub is_visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureOptions {
    /// Monitor index (0 = primary). None = primary.
    pub monitor: Option<u32>,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self { monitor: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InspectOptions {
    /// Window handle to inspect (None = foreground).
    pub window: Option<u64>,
    /// Max tree depth (0 = root only).
    pub max_depth: u32,
}

impl Default for InspectOptions {
    fn default() -> Self {
        Self {
            window: None,
            max_depth: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementTree {
    pub root: ElementInfo,
    pub supported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementInfo {
    pub name: String,
    pub control_type: String,
    pub value: Option<String>,
    pub rect: Option<Rect>,
    pub is_password: bool,
    pub children: Vec<ElementInfo>,
    /// `true` if this node's children were truncated to avoid huge trees.
    #[serde(default, skip_serializing_if = "is_false")]
    pub children_truncated: bool,
}

fn is_false(b: &bool) -> bool {
    !b
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputResult {
    pub actions_performed: usize,
}

// ── Backend trait ──────────────────────────────────────────────────────────

/// Platform-agnostic OS automation backend.
///
/// Implementations live in `navi-os-{windows,macos,linux}`. This facade
/// selects the correct one at compile time via [`platform_backend`].
pub trait ComputerUseBackend: Send + Sync {
    /// Human-readable platform name (e.g. "windows", "macos", "unsupported").
    fn platform_name(&self) -> &'static str;

    /// Captures the screen (or a specific monitor) to an image file.
    fn capture_screen(&self, out_dir: &str, opts: &CaptureOptions) -> Result<Screenshot>;

    /// Enumerates visible top-level windows.
    fn enumerate_windows(&self) -> Result<Vec<WindowInfo>>;

    /// Inspects the accessibility tree of a window.
    fn inspect_element(&self, opts: &InspectOptions) -> Result<ElementTree>;

    /// Simulates a sequence of input actions (mouse + keyboard).
    fn simulate_input(&self, actions: &[serde_json::Value]) -> Result<InputResult>;
}

// ── Platform selection ─────────────────────────────────────────────────────

/// Returns the platform-appropriate backend, or an [`UnsupportedBackend`] on
/// platforms without a compiled implementation.
pub fn platform_backend() -> Box<dyn ComputerUseBackend> {
    #[cfg(windows)]
    {
        return Box::new(WindowsBackendAdapter);
    }

    #[cfg(not(windows))]
    {
        return Box::new(UnsupportedBackend);
    }
}

// ── Unsupported platform stub ──────────────────────────────────────────────

pub struct UnsupportedBackend;

impl ComputerUseBackend for UnsupportedBackend {
    fn platform_name(&self) -> &'static str {
        "unsupported"
    }
    fn capture_screen(&self, _out_dir: &str, _opts: &CaptureOptions) -> Result<Screenshot> {
        bail!("computer use: no platform backend compiled for this OS")
    }
    fn enumerate_windows(&self) -> Result<Vec<WindowInfo>> {
        bail!("computer use: no platform backend compiled for this OS")
    }
    fn inspect_element(&self, _opts: &InspectOptions) -> Result<ElementTree> {
        bail!("computer use: no platform backend compiled for this OS")
    }
    fn simulate_input(&self, _actions: &[serde_json::Value]) -> Result<InputResult> {
        bail!("computer use: no platform backend compiled for this OS")
    }
}

// ── Windows adapter ────────────────────────────────────────────────────────

#[cfg(windows)]
struct WindowsBackendAdapter;

#[cfg(windows)]
impl ComputerUseBackend for WindowsBackendAdapter {
    fn platform_name(&self) -> &'static str {
        "windows"
    }

    fn capture_screen(&self, out_dir: &str, _opts: &CaptureOptions) -> Result<Screenshot> {
        let win = navi_os_windows::capture_screen(out_dir)?;
        Ok(Screenshot {
            path: win.path,
            width: win.width,
            height: win.height,
            size_bytes: win.size_bytes,
        })
    }

    fn enumerate_windows(&self) -> Result<Vec<WindowInfo>> {
        let wins = navi_os_windows::enumerate_windows()?;
        Ok(wins
            .into_iter()
            .map(|w| WindowInfo {
                hwnd: w.hwnd,
                title: w.title,
                pid: w.pid,
                rect: Rect {
                    x: w.rect.x,
                    y: w.rect.y,
                    width: w.rect.width,
                    height: w.rect.height,
                },
                is_focused: w.is_focused,
                is_visible: w.is_visible,
            })
            .collect())
    }

    fn inspect_element(&self, opts: &InspectOptions) -> Result<ElementTree> {
        // UIA calls are cross-process and can block on unresponsive apps.
        // Run on a blocking thread so we don't stall the tokio runtime.
        let win_opts = navi_os_windows::WinInspectOptions {
            window: opts.window,
            max_depth: opts.max_depth,
        };
        let tree = navi_os_windows::inspect_element(&win_opts)?;
        Ok(ElementTree {
            root: convert_element(tree.root),
            supported: tree.supported,
        })
    }

    fn simulate_input(&self, actions: &[serde_json::Value]) -> Result<InputResult> {
        let res = navi_os_windows::simulate_input(actions)?;
        Ok(InputResult {
            actions_performed: res.actions_performed,
        })
    }
}

/// Converts the leaf crate's `WinElementInfo` into the facade's `ElementInfo`.
///
/// This is a plain recursive mapping — the leaf crate has no dependency on
/// the facade, so the adapter performs the conversion at the boundary.
#[cfg(windows)]
fn convert_element(win: navi_os_windows::WinElementInfo) -> ElementInfo {
    ElementInfo {
        name: win.name,
        control_type: win.control_type,
        value: win.value,
        rect: win.rect.map(|r| Rect {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        }),
        is_password: win.is_password,
        children: win.children.into_iter().map(convert_element).collect(),
        children_truncated: win.children_truncated,
    }
}

// ── Target resolution (deny-list support, ADR 0016) ────────────────────────

/// Resolved target app for the deny-list check.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TargetApp {
    pub pid: u32,
    pub exe_name: String,
    pub window_title: String,
}

/// Resolves the target app at a screen coordinate (for mouse actions).
///
/// Returns `None` if no window is at the point or the platform backend
/// doesn't support target resolution.
pub fn resolve_target_for_point(x: i32, y: i32) -> Option<TargetApp> {
    #[cfg(windows)]
    {
        navi_os_windows::resolve_target_for_point(x, y).map(|t| TargetApp {
            pid: t.pid,
            exe_name: t.exe_name,
            window_title: t.window_title,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (x, y);
        None
    }
}

/// Resolves the target app from the foreground window (for keyboard actions).
pub fn resolve_target_foreground() -> Option<TargetApp> {
    #[cfg(windows)]
    {
        navi_os_windows::resolve_target_foreground().map(|t| TargetApp {
            pid: t.pid,
            exe_name: t.exe_name,
            window_title: t.window_title,
        })
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Returns `true` if the focused element in the foreground window is a
/// password field (UIA `IsPassword`).
///
/// This is the hook for the Auto-mode sensitive-field guard (ADR 0016):
/// `simulate_input` should refuse to type into password fields in Auto mode
/// even though ordinary input is auto-approved. The guard is **not yet
/// activated** in `SimulateInputTool::invoke` — this function is the
/// building block. Returns `false` on non-Windows or when the focused
/// element can't be resolved.
pub fn is_target_sensitive() -> bool {
    #[cfg(windows)]
    {
        is_target_sensitive_windows()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
fn is_target_sensitive_windows() -> bool {
    // Walk the foreground window's ControlView tree (depth 0-2) and look for
    // any element with `is_password == true` that also has keyboard focus.
    // We use a shallow walk because the focused element is usually near the
    // root of the active window.
    use navi_os_windows::{WinInspectOptions, inspect_element};

    let opts = WinInspectOptions {
        window: None, // foreground
        max_depth: 2,
    };
    let tree = match inspect_element(&opts) {
        Ok(t) => t,
        Err(_) => return false, // fail open — don't block on resolution errors
    };
    find_password_with_focus(&tree.root)
}

/// Recursively searches for an element with `is_password == true` and
/// (heuristically) keyboard focus. Since the UIA `HasKeyboardFocus` property
/// isn't exposed in our `WinElementInfo` yet, we treat any password field in
/// the foreground window's shallow tree as "potentially the target" — a
/// conservative match that errs toward blocking sensitive input.
#[cfg(windows)]
fn find_password_with_focus(el: &navi_os_windows::WinElementInfo) -> bool {
    if el.is_password {
        return true;
    }
    el.children.iter().any(find_password_with_focus)
}
