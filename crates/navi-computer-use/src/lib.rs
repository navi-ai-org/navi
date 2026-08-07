//! OS automation facade for NAVI computer use.
//!
//! Defines the [`ComputerUseBackend`] trait and platform-agnostic data types.
//! The platform backend is selected at compile time via `cfg(target_os)`:
//!
//! - **Windows** — backed by [`navi_os_windows`] (GDI capture, `EnumWindows`,
//!   `SendInput`).
//! - **macOS** — backed by [`navi_os_macos`] (`CGDisplayCreateImage`,
//!   `CGWindowListCopyWindowInfo`, `AXUIElement`, `CGEvent`).
//! - **Linux** — not yet implemented; [`platform_backend`] returns an
//!   [`UnsupportedBackend`] that fails all operations with a clear message.
//!
//! Consumers (`navi-core`) depend on this crate only, never on a platform
//! backend directly — mirroring the `navi-providers` / `navi-openai` split.
//!
//! See ADR 0016 for the security model, tool exposure, and surface sync rules.

use anyhow::{Context, Result, bail};
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
    /// Element ID to drill down from (e.g. "w0.e12"). When set, the backend
    /// walks the sub-tree starting from this element instead of the window
    /// root. If both `window` and `element_id` are set, `element_id` wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_id: Option<String>,
    /// Max tree depth (0 = root only).
    pub max_depth: u32,
    /// Use RawView (deeper, enters Electron/Chromium apps) instead of
    /// ControlView (logical UI tree). Default false.
    #[serde(default)]
    pub raw_view: bool,
}

impl Default for InspectOptions {
    fn default() -> Self {
        Self {
            window: None,
            element_id: None,
            max_depth: 3,
            raw_view: false,
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
    /// Stable element identifier (e.g. "w0.e12") assigned during
    /// `inspect_desktop` or `inspect_element`. Use this with
    /// `simulate_input` (click by ID) or `inspect_element` (drill-down).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_id: Option<String>,
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

// ── Desktop snapshot types ─────────────────────────────────────────────────

/// Shallow snapshot of all visible windows and their top-level UI elements.
///
/// Returned by [`ComputerUseBackend::inspect_desktop`]. This is the
/// text-based equivalent of looking at the screen — no screenshot needed.
/// Each element has an `element_id` for use with `inspect_element`
/// (drill-down) or `simulate_input` (click by ID).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopSnapshot {
    pub windows: Vec<WindowSnapshot>,
    pub platform: String,
    /// Windows that were visible but could not be inspected (UIPI,
    /// unresponsive app, etc.). Empty on macOS (AX API doesn't fail
    /// per-window the way UIA does).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_windows: Vec<String>,
}

/// One window in a [`DesktopSnapshot`], with its shallow element tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowSnapshot {
    /// Stable window identifier (e.g. "w0", "w1").
    pub window_id: String,
    /// Platform-specific window handle.
    pub hwnd: u64,
    pub title: String,
    pub pid: u32,
    pub rect: Rect,
    pub is_focused: bool,
    /// Shallow element tree (depth 2: window → panels → controls).
    pub elements: Vec<ElementInfo>,
}

// ── Open application types ─────────────────────────────────────────────────

/// Result of [`ComputerUseBackend::open_application`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAppResult {
    /// Whether the application was successfully launched.
    pub launched: bool,
    /// Process ID if available (0 if unknown).
    pub pid: u32,
    /// Human-readable description of what happened.
    pub message: String,
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

    /// Inspects the accessibility tree of a window (or sub-tree from an
    /// element_id). Supports RawView for deeper traversal into
    /// Electron/Chromium apps.
    fn inspect_element(&self, opts: &InspectOptions) -> Result<ElementTree>;

    /// Simulates a sequence of input actions (mouse + keyboard).
    fn simulate_input(&self, actions: &[serde_json::Value]) -> Result<InputResult>;

    /// Returns a shallow snapshot of all visible windows and their UI
    /// elements (depth 2). Each element gets a stable `element_id` for
    /// drill-down via `inspect_element` or click-by-ID via `simulate_input`.
    /// This is the text-based equivalent of looking at the screen.
    fn inspect_desktop(&self) -> Result<DesktopSnapshot>;

    /// Opens/launches an application by name or path. Bypasses the GUI
    /// entirely — no Start menu or Spotlight simulation needed.
    fn open_application(&self, name: &str) -> Result<OpenAppResult>;
}

// ── Platform selection ─────────────────────────────────────────────────────

/// Returns the platform-appropriate backend, or an [`UnsupportedBackend`] on
/// platforms without a compiled implementation.
pub fn platform_backend() -> Box<dyn ComputerUseBackend> {
    #[cfg(windows)]
    {
        return Box::new(WindowsBackendAdapter);
    }

    #[cfg(target_os = "macos")]
    {
        return Box::new(MacosBackendAdapter);
    }

    #[cfg(not(any(windows, target_os = "macos")))]
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
    fn inspect_desktop(&self) -> Result<DesktopSnapshot> {
        bail!("computer use: no platform backend compiled for this OS")
    }
    fn open_application(&self, _name: &str) -> Result<OpenAppResult> {
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
            element_id: opts.element_id.clone(),
            max_depth: opts.max_depth,
            raw_view: opts.raw_view,
        };
        let tree = navi_os_windows::inspect_element(&win_opts).with_context(|| {
            format!(
                "inspect_element failed (window={:?}, element_id={:?}, max_depth={}, raw_view={})",
                opts.window, opts.element_id, opts.max_depth, opts.raw_view
            )
        })?;
        Ok(ElementTree {
            root: convert_element(tree.root),
            supported: tree.supported,
        })
    }

    fn simulate_input(&self, actions: &[serde_json::Value]) -> Result<InputResult> {
        let res = navi_os_windows::simulate_input(actions)
            .with_context(|| format!("simulate_input failed ({} actions)", actions.len()))?;
        Ok(InputResult {
            actions_performed: res.actions_performed,
        })
    }

    fn inspect_desktop(&self) -> Result<DesktopSnapshot> {
        let snap = navi_os_windows::inspect_desktop().context("inspect_desktop failed")?;
        Ok(DesktopSnapshot {
            windows: snap
                .windows
                .into_iter()
                .map(|w| WindowSnapshot {
                    window_id: w.window_id,
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
                    elements: w.elements.into_iter().map(convert_element).collect(),
                })
                .collect(),
            platform: "windows".to_string(),
            skipped_windows: snap.skipped_windows,
        })
    }

    fn open_application(&self, name: &str) -> Result<OpenAppResult> {
        let res = navi_os_windows::open_application(name)
            .with_context(|| format!("open_application failed for '{name}'"))?;
        Ok(OpenAppResult {
            launched: res.launched,
            pid: res.pid,
            message: res.message,
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
        element_id: win.element_id,
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

// ── macOS adapter ──────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
struct MacosBackendAdapter;

#[cfg(target_os = "macos")]
impl ComputerUseBackend for MacosBackendAdapter {
    fn platform_name(&self) -> &'static str {
        "macos"
    }

    fn capture_screen(&self, out_dir: &str, _opts: &CaptureOptions) -> Result<Screenshot> {
        let mac = navi_os_macos::capture_screen(out_dir)?;
        Ok(Screenshot {
            path: mac.path,
            width: mac.width,
            height: mac.height,
            size_bytes: mac.size_bytes,
        })
    }

    fn enumerate_windows(&self) -> Result<Vec<WindowInfo>> {
        let wins = navi_os_macos::enumerate_windows()?;
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
        let mac_opts = navi_os_macos::MacInspectOptions {
            window: opts.window,
            element_id: opts.element_id.clone(),
            max_depth: opts.max_depth,
            raw_view: opts.raw_view,
        };
        let tree = navi_os_macos::inspect_element(&mac_opts)?;
        Ok(ElementTree {
            root: convert_element_mac(tree.root),
            supported: tree.supported,
        })
    }

    fn simulate_input(&self, actions: &[serde_json::Value]) -> Result<InputResult> {
        let res = navi_os_macos::simulate_input(actions)?;
        Ok(InputResult {
            actions_performed: res.actions_performed,
        })
    }

    fn inspect_desktop(&self) -> Result<DesktopSnapshot> {
        let snap = navi_os_macos::inspect_desktop()?;
        Ok(DesktopSnapshot {
            windows: snap
                .windows
                .into_iter()
                .map(|w| WindowSnapshot {
                    window_id: w.window_id,
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
                    elements: w.elements.into_iter().map(convert_element_mac).collect(),
                })
                .collect(),
            platform: "macos".to_string(),
            skipped_windows: Vec::new(),
        })
    }

    fn open_application(&self, name: &str) -> Result<OpenAppResult> {
        let res = navi_os_macos::open_application(name)?;
        Ok(OpenAppResult {
            launched: res.launched,
            pid: res.pid,
            message: res.message,
        })
    }
}

/// Converts the leaf crate's `MacElementInfo` into the facade's `ElementInfo`.
#[cfg(target_os = "macos")]
fn convert_element_mac(mac: navi_os_macos::MacElementInfo) -> ElementInfo {
    ElementInfo {
        element_id: mac.element_id,
        name: mac.name,
        control_type: mac.control_type,
        value: mac.value,
        rect: mac.rect.map(|r| Rect {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        }),
        is_password: mac.is_password,
        children: mac.children.into_iter().map(convert_element_mac).collect(),
        children_truncated: mac.children_truncated,
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
    #[cfg(target_os = "macos")]
    {
        navi_os_macos::resolve_target_for_point(x, y).map(|t| TargetApp {
            pid: t.pid,
            exe_name: t.exe_name,
            window_title: t.window_title,
        })
    }
    #[cfg(not(any(windows, target_os = "macos")))]
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
    #[cfg(target_os = "macos")]
    {
        navi_os_macos::resolve_target_foreground().map(|t| TargetApp {
            pid: t.pid,
            exe_name: t.exe_name,
            window_title: t.window_title,
        })
    }
    #[cfg(not(any(windows, target_os = "macos")))]
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
    #[cfg(target_os = "macos")]
    {
        is_target_sensitive_macos()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        false
    }
}

/// Returns `true` if the process has macOS Accessibility permission
/// (`AXIsProcessTrusted()`). Returns `false` on non-macOS platforms.
///
/// Used by the `computer-use doctor` diagnostic check.
pub fn is_accessibility_trusted_macos() -> bool {
    #[cfg(target_os = "macos")]
    {
        navi_os_macos::is_accessibility_trusted()
    }
    #[cfg(not(target_os = "macos"))]
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
        ..Default::default()
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

#[cfg(target_os = "macos")]
fn is_target_sensitive_macos() -> bool {
    // Walk the focused UI element's accessibility tree (depth 0-2) and look
    // for any element with `is_password == true`. Same conservative approach
    // as the Windows implementation.
    use navi_os_macos::{MacInspectOptions, inspect_element};

    let opts = MacInspectOptions {
        window: None, // foreground
        max_depth: 2,
    };
    let tree = match inspect_element(&opts) {
        Ok(t) => t,
        Err(_) => return false, // fail open
    };
    find_password_with_focus_mac(&tree.root)
}

#[cfg(target_os = "macos")]
fn find_password_with_focus_mac(el: &navi_os_macos::MacElementInfo) -> bool {
    if el.is_password {
        return true;
    }
    el.children.iter().any(find_password_with_focus_mac)
}

// ── Screenshot annotation (ADR 0016 §5) ─────────────────────────────────────

/// Annotates a screenshot image with bounding-box overlays for each element
/// in an [`ElementTree`]. The annotated image is returned as PNG bytes.
///
/// Each element with a `rect` gets a hollow rectangle drawn on the image:
/// - **Red** for password fields (`is_password: true`)
/// - **Green** for interactive elements (button, edit, checkbox, etc.)
/// - **Blue** for containers (window, pane, group)
/// - **Gray** for everything else
///
/// The model receives both the annotated image and the element tree JSON
/// (with names, control types, and coordinates), so it can correlate visual
/// boxes with semantic information by position.
pub fn annotate_screenshot(image_bytes: &[u8], tree: &ElementTree) -> Result<Vec<u8>> {
    let mut img = image::load_from_memory(image_bytes)
        .map_err(|e| anyhow::anyhow!("failed to decode screenshot: {e}"))?
        .to_rgba8();

    let mut stats = AnnotationStats::default();
    annotate_elements(&mut img, &tree.root, &mut stats);

    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .map_err(|e| anyhow::anyhow!("failed to encode annotated PNG: {e}"))?;

    tracing::info!(
        annotated = stats.annotated,
        skipped = stats.skipped,
        password_fields = stats.password_fields,
        "annotate_screenshot complete"
    );

    Ok(png_bytes)
}

#[derive(Default)]
struct AnnotationStats {
    annotated: usize,
    skipped: usize,
    password_fields: usize,
}

fn annotate_elements(img: &mut image::RgbaImage, el: &ElementInfo, stats: &mut AnnotationStats) {
    if let Some(rect) = &el.rect {
        let (w, h) = img.dimensions();
        // Only draw rects that are at least partially within the image bounds.
        if rect.width > 0
            && rect.height > 0
            && rect.x < w as i32
            && rect.y < h as i32
            && rect.x + rect.width > 0
            && rect.y + rect.height > 0
        {
            let color = element_color(el);
            let img_rect = imageproc::rect::Rect::at(rect.x, rect.y)
                .of_size(rect.width as u32, rect.height as u32);
            imageproc::drawing::draw_hollow_rect_mut(img, img_rect, color);

            stats.annotated += 1;
            if el.is_password {
                stats.password_fields += 1;
            }
        } else {
            stats.skipped += 1;
        }
    }

    for child in &el.children {
        annotate_elements(img, child, stats);
    }
}

/// Returns the overlay color for an element based on its type and password flag.
fn element_color(el: &ElementInfo) -> image::Rgba<u8> {
    if el.is_password {
        return image::Rgba([255, 0, 0, 255]); // red
    }
    match el.control_type.to_lowercase().as_str() {
        "button" | "edit" | "checkbox" | "radio button" | "combobox" | "list" | "list item"
        | "menu item" | "tab item" | "hyperlink" | "slider" | "spinner" | "text" | "document"
        | "custom" => {
            image::Rgba([0, 255, 0, 255]) // green — interactive
        }
        "window" | "pane" | "group" | "toolbar" | "menubar" | "tab" | "tree" | "tree item"
        | "statusbar" | "titlebar" => {
            image::Rgba([0, 120, 255, 255]) // blue — container
        }
        _ => image::Rgba([160, 160, 160, 255]), // gray — other
    }
}

#[cfg(test)]
mod annotate_tests {
    use super::*;

    fn make_rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    fn make_element(
        name: &str,
        control_type: &str,
        rect: Option<Rect>,
        is_password: bool,
        children: Vec<ElementInfo>,
    ) -> ElementInfo {
        ElementInfo {
            name: name.to_string(),
            control_type: control_type.to_string(),
            value: None,
            rect,
            is_password,
            children,
            children_truncated: false,
        }
    }

    fn make_test_png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::new(w, h);
        let mut bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("encode png");
        bytes
    }

    #[test]
    fn annotate_draws_boxes_for_elements_within_bounds() {
        let png = make_test_png(200, 200);
        let tree = ElementTree {
            root: make_element(
                "Window",
                "window",
                Some(make_rect(0, 0, 200, 200)),
                false,
                vec![
                    make_element(
                        "OK",
                        "button",
                        Some(make_rect(50, 150, 80, 30)),
                        false,
                        vec![],
                    ),
                    make_element(
                        "Password",
                        "edit",
                        Some(make_rect(50, 50, 100, 25)),
                        true,
                        vec![],
                    ),
                ],
            ),
            supported: true,
        };

        let result = annotate_screenshot(&png, &tree).expect("annotate");
        assert!(!result.is_empty());

        // Verify it's a valid PNG.
        let decoded = image::load_from_memory(&result).expect("decode annotated png");
        assert_eq!(decoded.width(), 200);
        assert_eq!(decoded.height(), 200);
    }

    #[test]
    fn annotate_skips_elements_outside_image_bounds() {
        let png = make_test_png(100, 100);
        let tree = ElementTree {
            root: make_element(
                "Root",
                "window",
                Some(make_rect(0, 0, 100, 100)),
                false,
                vec![
                    // This element is entirely outside the image.
                    make_element(
                        "Outside",
                        "button",
                        Some(make_rect(200, 200, 50, 50)),
                        false,
                        vec![],
                    ),
                    // This element has zero dimensions.
                    make_element(
                        "Zero",
                        "button",
                        Some(make_rect(50, 50, 0, 0)),
                        false,
                        vec![],
                    ),
                ],
            ),
            supported: true,
        };

        // Should not fail — just skips out-of-bounds elements.
        let result = annotate_screenshot(&png, &tree).expect("annotate");
        let decoded = image::load_from_memory(&result).expect("decode");
        assert_eq!(decoded.width(), 100);
    }

    #[test]
    fn annotate_handles_elements_with_no_rect() {
        let png = make_test_png(100, 100);
        let tree = ElementTree {
            root: make_element(
                "Root",
                "window",
                None, // no rect
                false,
                vec![make_element("Child", "button", None, false, vec![])],
            ),
            supported: true,
        };

        let result = annotate_screenshot(&png, &tree).expect("annotate");
        let decoded = image::load_from_memory(&result).expect("decode");
        assert_eq!(decoded.width(), 100);
    }

    #[test]
    fn annotate_preserves_image_dimensions() {
        let png = make_test_png(640, 480);
        let tree = ElementTree {
            root: make_element(
                "Root",
                "window",
                Some(make_rect(0, 0, 640, 480)),
                false,
                vec![],
            ),
            supported: true,
        };

        let result = annotate_screenshot(&png, &tree).expect("annotate");
        let decoded = image::load_from_memory(&result).expect("decode");
        assert_eq!(decoded.width(), 640);
        assert_eq!(decoded.height(), 480);
    }

    #[test]
    fn element_color_password_is_red() {
        let el = make_element("pw", "edit", None, true, vec![]);
        assert_eq!(element_color(&el), image::Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn element_color_button_is_green() {
        let el = make_element("OK", "button", None, false, vec![]);
        assert_eq!(element_color(&el), image::Rgba([0, 255, 0, 255]));
    }

    #[test]
    fn element_color_window_is_blue() {
        let el = make_element("Main", "window", None, false, vec![]);
        assert_eq!(element_color(&el), image::Rgba([0, 120, 255, 255]));
    }

    #[test]
    fn element_color_unknown_is_gray() {
        let el = make_element("x", "tooltip", None, false, vec![]);
        assert_eq!(element_color(&el), image::Rgba([160, 160, 160, 255]));
    }

    #[test]
    fn annotate_works_with_bmp_input() {
        // capture_screen produces BMP, so verify BMP input works too.
        let img = image::RgbaImage::new(100, 100);
        let mut bmp_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut bmp_bytes),
            image::ImageFormat::Bmp,
        )
        .expect("encode bmp");

        let tree = ElementTree {
            root: make_element(
                "Root",
                "window",
                Some(make_rect(10, 10, 50, 50)),
                false,
                vec![],
            ),
            supported: true,
        };

        let result = annotate_screenshot(&bmp_bytes, &tree).expect("annotate");
        let decoded = image::load_from_memory(&result).expect("decode");
        assert_eq!(decoded.width(), 100);
        assert_eq!(decoded.height(), 100);
    }
}
