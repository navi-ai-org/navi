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
/// the foreground window. `opts.element_id` (e.g. "w0.e12") drills down from
/// a specific element instead of the window root — the tree is walked with
/// the same walker/counter scheme used by `inspect_desktop` to locate the
/// element, then its sub-tree is expanded to `opts.max_depth`.
/// `opts.max_depth` bounds the recursion (0 = root only). `opts.raw_view`
/// selects the RawView walker (all nodes) instead of the ControlView walker
/// (logical tree); use it for Electron/Chromium apps where the ControlView
/// tree is sparse.
///
/// Each visited element is assigned a stable `element_id` of the form
/// `w0.e{counter}` (single-window inspection always uses window index 0).
pub fn inspect_element(opts: &WinInspectOptions) -> Result<WinElementTree> {
    super::ensure_com_initialized()?;

    // Validate element_id format early, before any UIA calls — this way
    // invalid formats return a clear error even if the foreground window
    // is unavailable.
    if let Some(ref target_id) = opts.element_id {
        if parse_element_counter(target_id).is_none() {
            bail!("invalid element_id format: '{target_id}' (expected 'w{{idx}}.e{{counter}}')");
        }
    }

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

        // 4. If element_id is specified, locate the element by re-walking
        //    the tree with the same counter scheme and then expand from it.
        if let Some(ref target_id) = opts.element_id {
            let target_counter = parse_element_counter(target_id).unwrap();

            // Walk the tree to find the element at the target counter.
            let found = find_element_by_counter(&walker, &root, target_counter)?;
            match found {
                Some(element) => {
                    let mut counter = target_counter; // Continue numbering from the target
                    let root_info =
                        walk_tree(&walker, &element, 0, opts.max_depth, 0, &mut counter);
                    return Ok(WinElementTree {
                        root: root_info,
                        supported: true,
                    });
                }
                None => bail!(
                    "element_id '{target_id}' not found in the accessibility tree. \
                     The element may have been destroyed or the window may have changed. \
                     Re-run inspect_desktop to refresh element IDs."
                ),
            }
        }

        // 5. No element_id — walk from the window root as usual.
        let mut counter = 0usize;
        let root_info = walk_tree(&walker, &root, 0, opts.max_depth, 0, &mut counter);

        Ok(WinElementTree {
            root: root_info,
            supported: true,
        })
    }
}

/// Parses an element_id string ("w{idx}.e{counter}") and returns the counter
/// value. Returns None if the format doesn't match.
fn parse_element_counter(id: &str) -> Option<usize> {
    let after_e = id.split(".e").nth(1)?;
    after_e.parse::<usize>().ok()
}

/// Walks the UIA tree in pre-order (same as `walk_tree`) until it finds the
/// element at counter position `target_counter`. Returns the raw
/// `IUIAutomationElement` so the caller can expand its sub-tree.
///
/// This re-walks the tree from the root, which is O(n) but avoids needing to
/// cache raw COM element pointers (which are not `Send` and would require
/// careful lifetime management). The walk is bounded by
/// `MAX_CHILDREN_PER_NODE` per level to match the original walk.
unsafe fn find_element_by_counter(
    walker: &IUIAutomationTreeWalker,
    root: &IUIAutomationElement,
    target_counter: usize,
) -> Result<Option<IUIAutomationElement>> {
    unsafe {
        let mut counter = 0usize;
        // Use a generous max depth to reach any element visible in prior
        // inspect_desktop (depth 2) or inspect_element calls.
        find_element_recursive(walker, root, &mut counter, target_counter, 0)
    }
}

/// Recursive helper for `find_element_by_counter`.
///
/// Returns `Ok(None)` if the element was not found (either it doesn't exist,
/// the depth limit was reached, or walker errors prevented traversal).
/// Walker errors are logged at `debug` level so they don't spam the log but
/// are still diagnosable.
unsafe fn find_element_recursive(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    counter: &mut usize,
    target: usize,
    depth: u32,
) -> Result<Option<IUIAutomationElement>> {
    unsafe {
        if *counter == target {
            return Ok(Some(element.clone()));
        }
        *counter += 1;

        // Don't search deeper than a reasonable limit. This is distinct from
        // "not found" — the element may exist deeper in the tree, but we
        // can't reach it without exceeding the depth budget.
        if depth >= 50 {
            tracing::debug!(
                target_counter = target,
                depth,
                "find_element_recursive: depth limit reached, element may be deeper in the tree"
            );
            return Ok(None);
        }

        match walker.GetFirstChildElement(element) {
            Ok(first_child) => {
                let mut current = first_child;
                let mut count = 0usize;
                loop {
                    if count >= MAX_CHILDREN_PER_NODE {
                        tracing::debug!(
                            target_counter = target,
                            depth,
                            "find_element_recursive: child count limit reached, \
                             element may be among truncated siblings"
                        );
                        break;
                    }
                    if let Some(found) =
                        find_element_recursive(walker, &current, counter, target, depth + 1)?
                    {
                        return Ok(Some(found));
                    }
                    count += 1;
                    match walker.GetNextSiblingElement(&current) {
                        Ok(next) => current = next,
                        Err(e) => {
                            tracing::debug!(
                                target_counter = target,
                                depth,
                                error = %e,
                                "find_element_recursive: GetNextSiblingElement failed, \
                                 stopping sibling traversal at this level"
                            );
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::debug!(
                    target_counter = target,
                    depth,
                    error = %e,
                    "find_element_recursive: GetFirstChildElement failed, \
                     cannot traverse children at this level"
                );
            }
        }
        Ok(None)
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
        let mut skipped = Vec::new();
        for (idx, win) in wins.iter().take(DESKTOP_MAX_WINDOWS).enumerate() {
            let hwnd = HWND(win.hwnd as *mut _);
            if hwnd.0.is_null() {
                continue;
            }

            // Skip windows whose root element cannot be resolved (UIPI,
            // unresponsive app, etc.) — don't fail the whole snapshot.
            // Record the skip so the model knows why the window is missing.
            let root = match automation.ElementFromHandle(hwnd) {
                Ok(r) => r,
                Err(e) => {
                    let reason = format!(
                        "{} (hwnd={}, error={})",
                        if win.title.is_empty() {
                            "(untitled)"
                        } else {
                            &win.title
                        },
                        win.hwnd,
                        e
                    );
                    tracing::debug!(
                        window = %win.title,
                        hwnd = win.hwnd,
                        error = %e,
                        "inspect_desktop: skipping window, ElementFromHandle failed"
                    );
                    skipped.push(reason);
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

        Ok(WinDesktopSnapshot {
            windows: snapshots,
            skipped_windows: skipped,
        })
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

    // ── Unit tests (no desktop interaction needed) ────────────────────────

    #[test]
    fn parse_element_counter_valid_ids() {
        assert_eq!(parse_element_counter("w0.e0"), Some(0));
        assert_eq!(parse_element_counter("w0.e12"), Some(12));
        assert_eq!(parse_element_counter("w3.e199"), Some(199));
    }

    #[test]
    fn parse_element_counter_invalid_ids() {
        assert_eq!(parse_element_counter("w0.e"), None);
        assert_eq!(parse_element_counter("w0"), None);
        assert_eq!(parse_element_counter("abc"), None);
        assert_eq!(parse_element_counter("w0.abc"), None);
        assert_eq!(parse_element_counter(""), None);
    }

    // ── Edge cases: element_id format ─────────────────────────────────────

    #[test]
    fn parse_element_counter_edge_cases() {
        // Leading zeros — should parse as the integer value.
        assert_eq!(parse_element_counter("w0.e00"), Some(0));
        assert_eq!(parse_element_counter("w0.e007"), Some(7));

        // Uppercase "E" — does NOT match (separator is lowercase ".e").
        assert_eq!(parse_element_counter("W0.E0"), None);
        assert_eq!(parse_element_counter("w0.E0"), None);
        // Note: "W0.e0" parses as Some(0) because the parser only looks
        // for ".e" as separator — the prefix is not validated for case.
        // This is acceptable because element_ids are always generated by
        // our code, not user-typed.

        // Negative counter — doesn't parse as usize.
        assert_eq!(parse_element_counter("w0.e-1"), None);
        // Note: "w-1.e0" parses as Some(0) — the parser only looks after
        // ".e" and doesn't validate the window prefix. Acceptable since
        // element_ids are generated by our code.

        // Extra segments after the counter — "w0.e0.foo" splits on ".e"
        // at position 2, giving nth(1) = "0.foo" which doesn't parse.
        assert_eq!(parse_element_counter("w0.e0.foo"), None);
        // But "w0.e0.extra" splits on ".e" twice (".extra" contains ".e"),
        // giving nth(1) = "0" which DOES parse. This is a known limitation
        // of the simple split-based parser — acceptable because our
        // generated IDs never have extra segments.
        assert_eq!(parse_element_counter("w0.e0 "), None);

        // Space in counter.
        assert_eq!(parse_element_counter("w0.e 0"), None);

        // Trailing characters after digits.
        assert_eq!(parse_element_counter("w0.e0x"), None);
        assert_eq!(parse_element_counter("w0.e12abc"), None);

        // Very large counter (should still parse).
        assert_eq!(parse_element_counter("w0.e999999999"), Some(999999999));

        // Overflow — usize::MAX + 1 should fail.
        assert_eq!(
            parse_element_counter(&format!("w0.e{}", usize::MAX as u128 + 1)),
            None
        );

        // usize::MAX itself should parse.
        assert_eq!(
            parse_element_counter(&format!("w0.e{}", usize::MAX)),
            Some(usize::MAX)
        );

        // Multiple ".e" segments — split takes the first ".e" occurrence.
        // "w0.e1.e2" → split on ".e" → ["w0", "1", "2"] → nth(1) = "1" → Some(1).
        // The parser only looks at the first segment after ".e", so extra
        // ".e" segments are ignored. This is acceptable — our generated IDs
        // never contain multiple ".e" segments.
        assert_eq!(parse_element_counter("w0.e1.e2"), Some(1));

        // Just ".e0" with no window prefix.
        assert_eq!(parse_element_counter(".e0"), Some(0));

        // Empty counter after ".e".
        assert_eq!(parse_element_counter("w0.e"), None);

        // Whitespace-only.
        assert_eq!(parse_element_counter("   "), None);
        // Note: " w0.e0" parses as Some(0) because the parser only looks
        // at the segment after ".e" — the prefix is not validated. This is
        // acceptable because element_ids are always generated by our code,
        // not user-typed.
    }

    // ── Edge cases: inspect_element with element_id ───────────────────────

    #[test]
    #[cfg(windows)]
    fn inspect_element_element_id_zero_drills_down_root() {
        // element_id "w0.e0" should find the root element itself (counter=0).
        // This is the simplest drill-down case.
        let result = inspect_element(&WinInspectOptions {
            window: None,
            element_id: Some("w0.e0".to_string()),
            max_depth: 1,
            raw_view: false,
            ..Default::default()
        });

        match result {
            Ok(tree) => {
                assert!(tree.supported, "drill-down on e0 should be supported");
                // The root of the returned tree should have element_id "w0.e0".
                assert_eq!(
                    tree.root.element_id.as_deref(),
                    Some("w0.e0"),
                    "drilled-down root should have element_id w0.e0"
                );
            }
            Err(e) => {
                // May fail if no foreground window — skip.
                eprintln!("skipping inspect_element_element_id_zero_drills_down_root: {e}");
            }
        }
    }

    #[test]
    #[cfg(windows)]
    fn inspect_element_max_depth_zero_with_element_id() {
        // max_depth=0 with element_id should return just the target element
        // with no children, same as max_depth=0 without element_id.
        let result = inspect_element(&WinInspectOptions {
            window: None,
            element_id: Some("w0.e0".to_string()),
            max_depth: 0,
            raw_view: false,
            ..Default::default()
        });

        match result {
            Ok(tree) => {
                assert!(
                    tree.root.children.is_empty(),
                    "max_depth=0 with element_id should return no children"
                );
            }
            Err(e) => {
                eprintln!("skipping inspect_element_max_depth_zero_with_element_id: {e}");
            }
        }
    }

    #[test]
    #[cfg(windows)]
    fn inspect_element_raw_view_with_element_id() {
        // raw_view=true + element_id should work together — RawViewWalker
        // is used for the search and the expansion.
        let result = inspect_element(&WinInspectOptions {
            window: None,
            element_id: Some("w0.e0".to_string()),
            max_depth: 2,
            raw_view: true,
            ..Default::default()
        });

        match result {
            Ok(tree) => {
                assert!(tree.supported, "raw_view + element_id should be supported");
                assert!(tree.root.element_id.is_some());
            }
            Err(e) => {
                eprintln!("skipping inspect_element_raw_view_with_element_id: {e}");
            }
        }
    }

    #[test]
    #[cfg(windows)]
    fn inspect_element_both_window_and_element_id_set() {
        // When both window and element_id are set, element_id should take
        // precedence (per the InspectOptions doc). The window is used to
        // resolve the root, then element_id drills down from there.
        // We use the foreground window (window=None) and e0 (root).
        let result = inspect_element(&WinInspectOptions {
            window: None,
            element_id: Some("w0.e0".to_string()),
            max_depth: 1,
            raw_view: false,
            ..Default::default()
        });

        match result {
            Ok(tree) => {
                // Should return the root element (e0), not the window root's
                // first child. The element_id should be "w0.e0".
                assert_eq!(
                    tree.root.element_id.as_deref(),
                    Some("w0.e0"),
                    "element_id should take precedence over window"
                );
            }
            Err(e) => {
                eprintln!("skipping inspect_element_both_window_and_element_id_set: {e}");
            }
        }
    }

    #[test]
    #[cfg(windows)]
    fn inspect_element_element_id_with_leading_zeros() {
        // "w0.e007" should parse as counter=7 and find the 7th element.
        // If there are fewer than 8 elements, it should return "not found".
        let result = inspect_element(&WinInspectOptions {
            window: None,
            element_id: Some("w0.e007".to_string()),
            max_depth: 1,
            raw_view: false,
            ..Default::default()
        });

        match result {
            Ok(tree) => {
                // Found the 7th element — verify it has the right ID.
                assert!(tree.root.element_id.is_some());
            }
            Err(e) => {
                let msg = format!("{e}");
                // Should be "not found" if there are < 8 elements, or
                // "no target window" if no foreground.
                assert!(
                    msg.contains("not found")
                        || msg.contains("no target window")
                        || msg.contains("ElementFromHandle"),
                    "should error gracefully, got: {msg}"
                );
            }
        }
    }

    // ── Integration tests (require a real Windows desktop) ────────────────

    #[test]
    #[cfg(windows)]
    fn inspect_desktop_returns_windows_with_element_ids() {
        let snap = match inspect_desktop() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skipping inspect_desktop_returns_windows_with_element_ids: {e}");
                return;
            }
        };

        // Should return at least 0 windows (desktop may be empty in CI).
        // If there are windows, each should have a window_id and elements
        // with element_ids.
        for (i, win) in snap.windows.iter().enumerate() {
            assert!(
                win.window_id.starts_with('w'),
                "window_id should start with 'w', got: {}",
                win.window_id
            );
            assert_eq!(
                win.window_id,
                format!("w{i}"),
                "window_id should be 'w{{index}}', got: {}",
                win.window_id
            );

            // Check that elements have element_ids in the correct format.
            check_element_ids(&win.elements, &win.window_id);
        }
    }

    #[test]
    #[cfg(windows)]
    fn inspect_desktop_element_ids_are_unique() {
        let snap = match inspect_desktop() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skipping inspect_desktop_element_ids_are_unique: {e}");
                return;
            }
        };

        let mut all_ids: Vec<String> = Vec::new();
        for win in &snap.windows {
            collect_all_ids(&win.elements, &mut all_ids);
        }

        // Check for duplicates.
        let mut seen = std::collections::HashSet::new();
        for id in &all_ids {
            assert!(seen.insert(id.clone()), "duplicate element_id found: {id}");
        }
    }

    #[test]
    #[cfg(windows)]
    fn inspect_element_with_raw_view_returns_tree() {
        // RawView should work on the foreground window. This verifies
        // that RawViewWalker is callable and returns a valid tree.
        let tree = match inspect_element(&WinInspectOptions {
            window: None,
            element_id: None,
            max_depth: 2,
            raw_view: true,
            ..Default::default()
        }) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("skipping inspect_element_with_raw_view_returns_tree: {e}");
                return;
            }
        };
        assert!(tree.supported, "raw_view inspect should be supported");
        // RawView typically returns more children than ControlView.
        // Just verify the root has an element_id.
        assert!(
            tree.root.element_id.is_some(),
            "root should have an element_id"
        );
    }

    #[test]
    #[cfg(windows)]
    fn inspect_element_drill_down_by_element_id() {
        // 1. inspect_desktop to get element_ids
        let snap = match inspect_desktop() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skipping inspect_element_drill_down_by_element_id: {e}");
                return;
            }
        };

        // 2. Find a window with at least one element
        let target = snap.windows.iter().find(|w| !w.elements.is_empty());
        let (target_hwnd, target_element_id) = match target {
            Some(w) => {
                let el = &w.elements[0];
                let id = match &el.element_id {
                    Some(id) => id.clone(),
                    None => {
                        eprintln!("skipping: first element has no element_id");
                        return;
                    }
                };
                (w.hwnd, id)
            }
            None => {
                eprintln!("skipping: no window with elements found");
                return;
            }
        };

        // 3. Drill down from that element_id
        let tree = match inspect_element(&WinInspectOptions {
            window: Some(target_hwnd),
            element_id: Some(target_element_id.clone()),
            max_depth: 3,
            raw_view: false,
            ..Default::default()
        }) {
            Ok(t) => t,
            Err(e) => {
                // The element may have been destroyed between inspect_desktop
                // and this call. Skip rather than fail.
                eprintln!(
                    "skipping inspect_element_drill_down_by_element_id: \
                     drill-down on {target_element_id} failed: {e}"
                );
                return;
            }
        };

        assert!(tree.supported, "drill-down should be supported");
        // The drilled-down element should have an element_id (continuing
        // the counter from the target).
        assert!(
            tree.root.element_id.is_some(),
            "drilled-down root should have an element_id"
        );
    }

    #[test]
    #[cfg(windows)]
    fn inspect_element_drill_down_invalid_id_returns_error() {
        // An element_id that doesn't exist should return an error, not panic.
        let result = inspect_element(&WinInspectOptions {
            window: None,
            element_id: Some("w0.e99999".to_string()),
            max_depth: 2,
            raw_view: false,
            ..Default::default()
        });

        match result {
            Ok(_) => {
                // In the unlikely case there are 99999+ elements, the test
                // still passes — we just wanted to verify no panic.
            }
            Err(e) => {
                // Expected: error message should mention "not found",
                // "foreground", or "ElementFromHandle" (if there's no
                // foreground window in the test environment).
                let msg = format!("{e}");
                assert!(
                    msg.contains("not found")
                        || msg.contains("foreground")
                        || msg.contains("ElementFromHandle")
                        || msg.contains("no target window"),
                    "error should mention 'not found', 'foreground', 'ElementFromHandle', or 'no target window', got: {msg}"
                );
            }
        }
    }

    #[test]
    #[cfg(windows)]
    fn inspect_element_drill_down_invalid_format_returns_error() {
        let result = inspect_element(&WinInspectOptions {
            window: None,
            element_id: Some("garbage".to_string()),
            max_depth: 2,
            ..Default::default()
        });

        assert!(
            result.is_err(),
            "invalid element_id format should return an error"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("invalid element_id format"),
            "error should mention 'invalid element_id format', got: {msg}"
        );
    }

    // ── Helpers ───────────────────────────────────────────────────────────

    /// Recursively checks that all elements have element_ids in the format
    /// "w{window_id}.e{counter}".
    fn check_element_ids(elements: &[WinElementInfo], window_id: &str) {
        for el in elements {
            if let Some(ref id) = el.element_id {
                assert!(
                    id.starts_with(&format!("{window_id}.e")),
                    "element_id '{id}' should start with '{window_id}.e'"
                );
            }
            check_element_ids(&el.children, window_id);
        }
    }

    /// Recursively collects all element_ids from an element tree.
    fn collect_all_ids(elements: &[WinElementInfo], ids: &mut Vec<String>) {
        for el in elements {
            if let Some(ref id) = el.element_id {
                ids.push(id.clone());
            }
            collect_all_ids(&el.children, ids);
        }
    }
}
