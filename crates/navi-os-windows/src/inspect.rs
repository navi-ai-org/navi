//! Element inspection via UI Automation (UIA).
//!
//! Walks the **ControlView** tree (logical UI, not raw) from a target window
//! and returns `WinElementInfo` nodes with name, control type, value, bounding
//! rectangle, and password-field detection.
//!
//! COM is initialized lazily via [`super::ensure_com_initialized`] (MTA, once
//! per process). UIA calls are cross-process and can block; the facade wraps
//! this in `spawn_blocking` so the tokio runtime is not stalled.

use super::{
    WinDesktopSnapshot, WinElementInfo, WinElementTree, WinInspectOptions, WinRect,
    WinWindowSnapshot, enumerate_windows,
};
use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTreeWalker,
    IUIAutomationValuePattern, UIA_ValuePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use windows::core::BSTR;

/// Maximum children per node before truncating. Electron/VS Code can have
/// thousands of nodes at a single level; this keeps the payload bounded.
const MAX_CHILDREN_PER_NODE: usize = 200;

/// Maximum number of windows [`inspect_desktop`] inspects, to keep the
/// response payload bounded.
const DESKTOP_MAX_WINDOWS: usize = 20;

/// Inspects the accessibility tree of a window.
///
/// `opts.window` selects the target window by HWND (as `u64`); `None` uses
/// the foreground window. `opts.max_depth` bounds the recursion (0 = root
/// only). `opts.raw_view` selects the RawView walker (all nodes) instead of
/// the ControlView walker (logical tree); use it for Electron/Chromium apps
/// where the ControlView tree is sparse.
///
/// Each visited element is assigned a stable `element_id` of the form
/// `w0.e{counter}` (single-window inspection always uses window index 0).
pub fn inspect_element(opts: &WinInspectOptions) -> Result<WinElementTree> {
    super::ensure_com_initialized()?;

    unsafe {
        // 1. Create the IUIAutomation instance.
        let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_ALL)
            .context("CoCreateInstance(CUIAutomation) failed")?;

        // 2. Resolve the root element from the target window.
        let hwnd = match opts.window {
            Some(h) if h != 0 => HWND(h as *mut _),
            _ => GetForegroundWindow(),
        };
        if hwnd.0.is_null() {
            bail!("no target window: foreground window is null");
        }

        let root = automation
            .ElementFromHandle(hwnd)
            .context("IUIAutomation::ElementFromHandle failed (UIPI may block non-elevated clients from elevated windows)")?;

        // 3. Select the walker: RawView exposes every node (including
        //    decorative/invisible ones that flood Chromium/Electron trees),
        //    while ControlView skips them for a cleaner logical tree.
        let walker = if opts.raw_view {
            automation
                .RawViewWalker()
                .context("IUIAutomation::RawViewWalker failed")?
        } else {
            automation
                .ControlViewWalker()
                .context("IUIAutomation::ControlViewWalker failed")?
        };

        // 4. Walk recursively, building WinElementInfo nodes. Each visited
        //    element gets a stable id "w0.e{counter}".
        let mut counter = 0usize;
        let root_info = walk_tree(&walker, &root, 0, opts.max_depth, 0, &mut counter);

        Ok(WinElementTree {
            root: root_info,
            supported: true,
        })
    }
}

/// Returns a shallow snapshot of all visible windows and their top-level UI
/// elements (depth 2: window -> panels -> controls).
///
/// Each element is assigned a stable `element_id` of the form
/// `w{window_index}.e{counter}` (counter resets per window) for use with
/// [`inspect_element`] (drill-down) or `simulate_input` (click by ID).
///
/// Windows that fail inspection are skipped rather than failing the whole
/// call. At most [`DESKTOP_MAX_WINDOWS`] windows are inspected to keep the
/// payload bounded.
pub fn inspect_desktop() -> Result<WinDesktopSnapshot> {
    super::ensure_com_initialized()?;

    let wins = enumerate_windows().context("inspect_desktop: enumerate_windows failed")?;

    unsafe {
        // One IUIAutomation instance shared across all windows.
        let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_ALL)
            .context("CoCreateInstance(CUIAutomation) failed")?;

        let walker = automation
            .ControlViewWalker()
            .context("IUIAutomation::ControlViewWalker failed")?;

        let mut snapshots = Vec::new();
        for (idx, win) in wins.iter().take(DESKTOP_MAX_WINDOWS).enumerate() {
            let hwnd = HWND(win.hwnd as *mut _);
            if hwnd.0.is_null() {
                continue;
            }

            // Skip windows whose root element cannot be resolved (UIPI,
            // unresponsive app, etc.) — don't fail the whole snapshot.
            let root = match automation.ElementFromHandle(hwnd) {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!(
                        window = %win.title,
                        hwnd = win.hwnd,
                        error = %e,
                        "inspect_desktop: skipping window, ElementFromHandle failed"
                    );
                    continue;
                }
            };

            // Depth 2: window root -> its direct children -> their children.
            // The counter resets per window so ids are scoped as
            // w{idx}.e{counter}.
            let mut counter = 0usize;
            let root_info = walk_tree(&walker, &root, 0, 2, idx, &mut counter);

            snapshots.push(WinWindowSnapshot {
                window_id: format!("w{idx}"),
                hwnd: win.hwnd,
                title: win.title.clone(),
                pid: win.pid,
                rect: WinRect {
                    x: win.rect.x,
                    y: win.rect.y,
                    width: win.rect.width,
                    height: win.rect.height,
                },
                is_focused: win.is_focused,
                elements: root_info.children,
            });
        }

        Ok(WinDesktopSnapshot { windows: snapshots })
    }
}

/// Recursively walks the UIA tree from `element`, building a `WinElementInfo`.
///
/// Stops at `max_depth`. Each level truncates children to
/// `MAX_CHILDREN_PER_NODE` and sets `children_truncated` if exceeded.
///
/// `window_idx` and `counter` are used to assign stable element ids of the
/// form `w{window_idx}.e{counter}`. The counter is incremented for every
/// visited element (pre-order: the parent is numbered before its children).
unsafe fn walk_tree(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    depth: u32,
    max_depth: u32,
    window_idx: usize,
    counter: &mut usize,
) -> WinElementInfo {
    unsafe {
        let mut info = read_element_info(element);
        info.element_id = Some(format!("w{window_idx}.e{counter}"));
        *counter += 1;

        // Stop recursing at max_depth.
        if depth >= max_depth {
            return info;
        }

        // Get first child, then walk siblings.
        let mut children = Vec::new();
        let mut truncated = false;

        if let Ok(first_child) = walker.GetFirstChildElement(element) {
            let mut current = first_child;
            let mut count = 0usize;
            loop {
                if count >= MAX_CHILDREN_PER_NODE {
                    truncated = true;
                    break;
                }
                let child_info =
                    walk_tree(walker, &current, depth + 1, max_depth, window_idx, counter);
                children.push(child_info);
                count += 1;

                match walker.GetNextSiblingElement(&current) {
                    Ok(next) => current = next,
                    _ => break,
                }
            }
        }

        WinElementInfo {
            children,
            children_truncated: truncated,
            ..info
        }
    }
}

/// Reads the scalar properties of a single element into `WinElementInfo`
/// (no children). All property reads are fallible — missing/unsupported
/// properties degrade to empty/None rather than failing the whole walk.
unsafe fn read_element_info(element: &IUIAutomationElement) -> WinElementInfo {
    unsafe {
        let name = element
            .CurrentName()
            .map(|b: BSTR| b.to_string())
            .unwrap_or_default();

        let control_type = element
            .CurrentLocalizedControlType()
            .map(|b: BSTR| b.to_string())
            .unwrap_or_default();

        let is_password = element
            .CurrentIsPassword()
            .map(|b| b.as_bool())
            .unwrap_or(false);

        let rect = element.CurrentBoundingRectangle().ok().map(|r| WinRect {
            x: r.left,
            y: r.top,
            width: r.right - r.left,
            height: r.bottom - r.top,
        });

        // Read the Value pattern if the element supports it.
        // Password fields: we deliberately do NOT read the value even if the
        // pattern is available — `is_password` short-circuits to None so we
        // never leak credential text into the model context or session log.
        let value = if is_password {
            None
        } else {
            element
                .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                .ok()
                .and_then(|p| p.CurrentValue().ok())
                .map(|b: BSTR| b.to_string())
                .filter(|s| !s.is_empty())
        };

        WinElementInfo {
            element_id: None,
            name,
            control_type,
            value,
            rect,
            is_password,
            children: Vec::new(),
            children_truncated: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn inspect_foreground_returns_supported_tree() {
        // Integration test: inspects whatever window is currently in the
        // foreground. In a headless CI runner there may be no foreground
        // window, in which case we skip rather than fail.
        let tree = match inspect_element(&WinInspectOptions {
            window: None,
            max_depth: 1,
            ..Default::default()
        }) {
            Ok(t) => t,
            Err(e) => {
                // No foreground window or COM unavailable — skip.
                eprintln!("skipping inspect_foreground_returns_supported_tree: {e}");
                return;
            }
        };
        assert!(
            tree.supported,
            "tree.supported should be true when UIA is available"
        );
        // The root control type should be a non-empty localized string
        // (typically "window" or similar).
        assert!(!tree.root.control_type.is_empty() || !tree.root.name.is_empty());
    }

    #[test]
    #[cfg(windows)]
    fn inspect_max_depth_zero_returns_root_only() {
        let tree = match inspect_element(&WinInspectOptions {
            window: None,
            max_depth: 0,
            ..Default::default()
        }) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("skipping inspect_max_depth_zero_returns_root_only: {e}");
                return;
            }
        };
        assert!(
            tree.root.children.is_empty(),
            "max_depth=0 must not recurse"
        );
    }
}
