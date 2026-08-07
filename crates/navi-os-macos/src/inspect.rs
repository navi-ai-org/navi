//! Accessibility tree inspection via `AXUIElement`.
//!
//! Walks the AXUIElement tree of a window (or the focused UI element),
//! returning element names, roles, values, bounding rectangles, and
//! password-field flags. Requires Accessibility permission.

use anyhow::{Result, anyhow, bail};
use std::ffi::CString;

use crate::{
    MacDesktopSnapshot, MacElementInfo, MacElementTree, MacInspectOptions, MacRect,
    MacWindowSnapshot,
};

/// Maximum children per node before truncation (matches Windows backend).
const MAX_CHILDREN_PER_NODE: usize = 200;

/// Maximum number of windows inspected by `inspect_desktop`.
const MAX_DESKTOP_WINDOWS: usize = 20;

/// Inspects the accessibility tree of a window.
///
/// If `opts.window` is None, inspects the focused UI element.
/// If `opts.window` is Some, looks up the window by its CGWindowNumber
/// and inspects the AXUIElement for that application.
///
/// Element IDs are assigned in the format `"w0.e{counter}"` (window index 0
/// for direct `inspect_element` calls; `inspect_desktop` reassigns them with
/// the correct window index). The `raw_view` option has no effect on macOS —
/// AXUIElement always exposes the full tree (no ControlView/RawView split).
/// The `element_id` option (drill-down) is accepted for API compatibility but
/// not yet supported on macOS; the full window tree is returned instead.
pub fn inspect_element(opts: &MacInspectOptions) -> Result<MacElementTree> {
    unsafe {
        // Check Accessibility permission first.
        if !accessibility_sys::AXIsProcessTrusted() {
            bail!(
                "macOS Accessibility permission not granted. \
                 Grant it in System Settings → Privacy & Security → Accessibility."
            );
        }

        let root_element = if let Some(window_number) = opts.window {
            // Look up the PID for this window number.
            let pid = find_pid_for_window(window_number)
                .ok_or_else(|| anyhow!("no window found with number {window_number}"))?;
            // Create an application-level AXUIElement.
            let app = accessibility_sys::AXUIElementCreateApplication(pid);
            if app.is_null() {
                bail!("AXUIElementCreateApplication failed for pid {pid}");
            }
            app
        } else {
            // Use the system-wide element to get the focused UI element.
            let system = accessibility_sys::AXUIElementCreateSystemWide();
            if system.is_null() {
                bail!("AXUIElementCreateSystemWide failed");
            }
            let focused = get_focused_element(system);
            accessibility_sys::CFRelease(system as _);
            if let Some(f) = focused {
                f
            } else {
                // Fallback: use the frontmost application.
                let pid = frontmost_pid().unwrap_or(0);
                let app = accessibility_sys::AXUIElementCreateApplication(pid);
                if app.is_null() {
                    bail!("AXUIElementCreateApplication failed for pid {pid}");
                }
                app
            }
        };

        // Direct inspect_element calls use window index 0 for element IDs.
        // inspect_desktop reassigns IDs with the correct per-window index.
        let mut counter: usize = 0;
        let root = walk_element(root_element, opts.max_depth, 0, 0, &mut counter);
        accessibility_sys::CFRelease(root_element as _);

        Ok(MacElementTree {
            root,
            supported: true,
        })
    }
}

/// Returns a shallow snapshot of all visible windows and their top-level UI
/// elements (depth 2). Each element gets a stable `element_id` of the form
/// `"w{window_index}.e{counter}"` for drill-down via `inspect_element` or
/// click-by-ID via `simulate_input`.
///
/// Windows that fail inspection are skipped. At most [`MAX_DESKTOP_WINDOWS`]
/// windows are inspected.
pub fn inspect_desktop() -> Result<MacDesktopSnapshot> {
    let windows = crate::enumerate_windows()?;
    let mut snapshots = Vec::with_capacity(windows.len().min(MAX_DESKTOP_WINDOWS));

    for (index, win) in windows.into_iter().enumerate() {
        if index >= MAX_DESKTOP_WINDOWS {
            break;
        }

        let opts = MacInspectOptions {
            window: Some(win.hwnd),
            element_id: None,
            max_depth: 2,
            raw_view: false,
        };

        // Skip windows that fail inspection (e.g. no accessibility access for
        // that process) rather than failing the whole desktop snapshot.
        let tree = match inspect_element(&opts) {
            Ok(t) => t,
            Err(_) => continue,
        };

        // Reassign element IDs with the correct window index ("w{index}.e{n}").
        let mut counter: usize = 0;
        let mut root = tree.root;
        assign_element_ids(&mut root, index, &mut counter);

        snapshots.push(MacWindowSnapshot {
            window_id: format!("w{index}"),
            hwnd: win.hwnd,
            title: win.title,
            pid: win.pid,
            rect: win.rect,
            is_focused: win.is_focused,
            elements: vec![root],
        });
    }

    Ok(MacDesktopSnapshot { windows: snapshots })
}

/// Recursively assigns stable element IDs in the format `"w{window_index}.e{counter}"`.
fn assign_element_ids(element: &mut MacElementInfo, window_index: usize, counter: &mut usize) {
    element.element_id = Some(format!("w{window_index}.e{counter}"));
    *counter += 1;
    for child in &mut element.children {
        assign_element_ids(child, window_index, counter);
    }
}

/// Recursively walks the AXUIElement tree, assigning stable element IDs.
unsafe fn walk_element(
    element: accessibility_sys::AXUIElementRef,
    max_depth: u32,
    current_depth: u32,
    window_index: usize,
    counter: &mut usize,
) -> MacElementInfo {
    let role = get_string_attribute(element, "AXRole").unwrap_or_default();
    let title = get_string_attribute(element, "AXTitle").unwrap_or_default();
    let is_password = is_password_field(element, &role);
    let value = if is_password {
        None // Never read password field values.
    } else {
        get_string_attribute(element, "AXValue")
    };
    let rect = get_position_and_size(element);

    // Assign a stable element ID: "w{window_index}.e{counter}".
    let element_id = Some(format!("w{window_index}.e{counter}"));
    *counter += 1;

    let (children, children_truncated) = if current_depth < max_depth {
        get_children(element, max_depth, current_depth, window_index, counter)
    } else {
        (Vec::new(), false)
    };

    MacElementInfo {
        element_id,
        name: title,
        control_type: map_ax_role(&role),
        value,
        rect,
        is_password,
        children,
        children_truncated,
    }
}

/// Gets a string attribute from an AXUIElement.
unsafe fn get_string_attribute(
    element: accessibility_sys::AXUIElementRef,
    attr: &str,
) -> Option<String> {
    let attr_cf = cf_string_create(attr)?;
    let mut value: accessibility_sys::CFTypeRef = std::ptr::null();
    let err = accessibility_sys::AXUIElementCopyAttributeValue(element, attr_cf, &mut value);
    accessibility_sys::CFRelease(attr_cf as _);

    if err != accessibility_sys::kAXErrorSuccess as _ || value.is_null() {
        return None;
    }

    // The value should be a CFString or AXValue.
    let result = cf_type_id_string(value);
    accessibility_sys::CFRelease(value);
    result
}

/// Creates a CFString from a Rust string.
unsafe fn cf_string_create(s: &str) -> Option<accessibility_sys::CFStringRef> {
    let c_str = CString::new(s).ok()?;
    let cf_str = core_foundation_sys::string::CFStringCreateWithCString(
        std::ptr::null_mut(),
        c_str.as_ptr(),
        core_foundation_sys::string::kCFStringEncodingUTF8,
    );
    if cf_str.is_null() { None } else { Some(cf_str) }
}

/// Converts a CFTypeRef to a String if it's a CFString.
unsafe fn cf_type_id_string(value: accessibility_sys::CFTypeRef) -> Option<String> {
    let type_id = core_foundation_sys::base::CFGetTypeID(value);
    let string_type_id = core_foundation_sys::string::CFStringGetTypeID();
    if type_id == string_type_id {
        let cf_str = value as core_foundation_sys::string::CFStringRef;
        Some(cf_string_to_string(cf_str))
    } else {
        None
    }
}

/// Converts a CFStringRef to a Rust String.
unsafe fn cf_string_to_string(cf_str: core_foundation_sys::string::CFStringRef) -> String {
    let length = core_foundation_sys::string::CFStringGetLength(cf_str);
    let max_size = core_foundation_sys::string::CFStringGetMaximumSizeForEncoding(
        length,
        core_foundation_sys::string::kCFStringEncodingUTF8,
    );
    let mut buffer = vec![0u8; max_size as usize + 1];
    let mut actual_length: core_foundation_sys::base::CFIndex = 0;
    let success = core_foundation_sys::string::CFStringGetBytes(
        cf_str,
        core_foundation_sys::base::CFRange {
            location: 0,
            length,
        },
        core_foundation_sys::string::kCFStringEncodingUTF8,
        0,
        false as _,
        buffer.as_mut_ptr(),
        max_size,
        &mut actual_length,
    );
    if success {
        buffer.truncate(actual_length as usize);
        String::from_utf8_lossy(&buffer).into_owned()
    } else {
        String::new()
    }
}

/// Checks if an element is a password field.
unsafe fn is_password_field(element: accessibility_sys::AXUIElementRef, role: &str) -> bool {
    // AXSecureTextField is the macOS password field role.
    if role == "AXSecureTextField" {
        return true;
    }
    // Also check the AXIsPasswordField attribute if available.
    let attr_cf = match cf_string_create("AXIsPasswordField") {
        Some(a) => a,
        None => return false,
    };
    let mut value: accessibility_sys::CFTypeRef = std::ptr::null();
    let err = accessibility_sys::AXUIElementCopyAttributeValue(element, attr_cf, &mut value);
    accessibility_sys::CFRelease(attr_cf as _);
    if err == accessibility_sys::kAXErrorSuccess as _ && !value.is_null() {
        let type_id = core_foundation_sys::base::CFGetTypeID(value);
        let bool_type_id = core_foundation_sys::number::CFBooleanGetTypeID();
        accessibility_sys::CFRelease(value);
        if type_id == bool_type_id {
            return value as u64 == core_foundation_sys::number::kCFBooleanTrue as u64;
        }
    }
    false
}

/// Gets the position and size of an element as a MacRect.
unsafe fn get_position_and_size(element: accessibility_sys::AXUIElementRef) -> Option<MacRect> {
    let position = get_ax_value_attribute(element, "AXPosition")?;
    let size = get_ax_value_attribute(element, "AXSize")?;

    let pos_rect = ax_value_to_rect(position)?;
    let size_rect = ax_value_to_rect(size)?;

    accessibility_sys::CFRelease(position as _);
    accessibility_sys::CFRelease(size as _);

    Some(MacRect {
        x: pos_rect.0 as i32,
        y: pos_rect.1 as i32,
        width: size_rect.0 as i32,
        height: size_rect.1 as i32,
    })
}

/// Gets an AXValue attribute.
unsafe fn get_ax_value_attribute(
    element: accessibility_sys::AXUIElementRef,
    attr: &str,
) -> Option<accessibility_sys::CFTypeRef> {
    let attr_cf = cf_string_create(attr)?;
    let mut value: accessibility_sys::CFTypeRef = std::ptr::null();
    let err = accessibility_sys::AXUIElementCopyAttributeValue(element, attr_cf, &mut value);
    accessibility_sys::CFRelease(attr_cf as _);
    if err == accessibility_sys::kAXErrorSuccess as _ && !value.is_null() {
        Some(value)
    } else {
        None
    }
}

/// Converts an AXValue (of type .rect) to (x, y, w, h).
unsafe fn ax_value_to_rect(value: accessibility_sys::CFTypeRef) -> Option<(f64, f64, f64, f64)> {
    // AXValue of type .rect wraps an CGRect { origin: {x, y}, size: {w, h} }.
    // We need to use AXValueGetValue with kAXValueCGPointType.
    // accessibility-sys may not expose AXValueGetValue directly, so we
    // do a raw memory read if the value is an AXValue.
    let mut x = 0.0f64;
    let mut y = 0.0f64;
    let mut w = 0.0f64;
    let mut h = 0.0f64;

    // Try to get the CGPoint and CGSize from the AXValue.
    // The AXValueRef is an opaque type; we use AXValueGetValue.
    let value_ref = value as accessibility_sys::AXValueRef;
    let mut point: CGPoint = CGPoint { x: 0.0, y: 0.0 };
    let point_got = accessibility_sys::AXValueGetValue(
        value_ref,
        accessibility_sys::kAXValueCGPointType,
        &mut point as *mut CGPoint as *mut _,
    );
    if point_got {
        x = point.x;
        y = point.y;
    }

    let mut size: CGSize = CGSize {
        width: 0.0,
        height: 0.0,
    };
    let size_got = accessibility_sys::AXValueGetValue(
        value_ref,
        accessibility_sys::kAXValueCGSizeType,
        &mut size as *mut CGSize as *mut _,
    );
    if size_got {
        w = size.width;
        h = size.height;
    }

    if point_got || size_got {
        Some((x, y, w, h))
    } else {
        None
    }
}

#[repr(C)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
struct CGSize {
    width: f64,
    height: f64,
}

/// Gets the children of an element.
unsafe fn get_children(
    element: accessibility_sys::AXUIElementRef,
    max_depth: u32,
    current_depth: u32,
    window_index: usize,
    counter: &mut usize,
) -> (Vec<MacElementInfo>, bool) {
    let attr_cf = match cf_string_create("AXChildren") {
        Some(a) => a,
        None => return (Vec::new(), false),
    };
    let mut value: accessibility_sys::CFTypeRef = std::ptr::null();
    let err = accessibility_sys::AXUIElementCopyAttributeValue(element, attr_cf, &mut value);
    accessibility_sys::CFRelease(attr_cf as _);

    if err != accessibility_sys::kAXErrorSuccess as _ || value.is_null() {
        return (Vec::new(), false);
    }

    // value should be a CFArray of AXUIElementRefs.
    let array = value as core_foundation_sys::array::CFArrayRef;
    let count = core_foundation_sys::array::CFArrayGetCount(array);
    let mut children = Vec::new();

    let limit = std::cmp::min(count as usize, MAX_CHILDREN_PER_NODE);
    for i in 0..limit {
        let child = core_foundation_sys::array::CFArrayGetValueAtIndex(array, i as _);
        if !child.is_null() {
            children.push(walk_element(
                child as accessibility_sys::AXUIElementRef,
                max_depth,
                current_depth + 1,
                window_index,
                counter,
            ));
        }
    }

    accessibility_sys::CFRelease(value);
    let truncated = count as usize > MAX_CHILDREN_PER_NODE;
    (children, truncated)
}

/// Gets the focused UI element from a system-wide AXUIElement.
unsafe fn get_focused_element(
    system: accessibility_sys::AXUIElementRef,
) -> Option<accessibility_sys::AXUIElementRef> {
    let attr_cf = cf_string_create("AXFocusedUIElement")?;
    let mut value: accessibility_sys::CFTypeRef = std::ptr::null();
    let err = accessibility_sys::AXUIElementCopyAttributeValue(system, attr_cf, &mut value);
    accessibility_sys::CFRelease(attr_cf as _);
    if err == accessibility_sys::kAXErrorSuccess as _ && !value.is_null() {
        Some(value as accessibility_sys::AXUIElementRef)
    } else {
        None
    }
}

/// Maps an AX role string to a human-readable control type (matching Windows UIA names).
fn map_ax_role(role: &str) -> String {
    match role {
        "AXButton" => "button",
        "AXTextField" => "edit",
        "AXSecureTextField" => "edit",
        "AXTextArea" => "edit",
        "AXCheckBox" => "checkbox",
        "AXRadioButton" => "radio button",
        "AXPopUpButton" => "combobox",
        "AXMenuButton" => "combobox",
        "AXComboBox" => "combobox",
        "AXList" => "list",
        "AXTable" => "table",
        "AXMenuItem" => "menu item",
        "AXMenuBarItem" => "menu item",
        "AXTab" => "tab item",
        "AXLink" => "hyperlink",
        "AXSlider" => "slider",
        "AXProgressIndicator" => "progress bar",
        "AXWindow" => "window",
        "AXSheet" => "window",
        "AXDrawer" => "pane",
        "AXGroup" => "group",
        "AXToolbar" => "toolbar",
        "AXMenuBar" => "menubar",
        "AXOutline" => "tree",
        "AXRow" => "tree item",
        "AXBrowser" => "pane",
        "AXScrollArea" => "pane",
        "AXSplitGroup" => "pane",
        "AXTabGroup" => "tab",
        "AXTableGroup" => "group",
        "AXStatusBar" => "statusbar",
        "AXImage" => "image",
        "AXStaticText" => "text",
        "AXTextArea" | "AXTextField" => "text",
        _ => role,
    }
    .to_string()
}

/// Finds the PID for a CGWindowNumber.
fn find_pid_for_window(window_number: u64) -> Option<u32> {
    // Use CGWindowListCopyWindowInfo to find the window with this number.
    // For simplicity, we enumerate all windows and find the matching one.
    // This is a bit inefficient but correct.
    // TODO: optimize with a direct lookup.
    crate::windows_list::enumerate_windows()
        .ok()?
        .into_iter()
        .find(|w| w.hwnd == window_number)
        .map(|w| w.pid)
}

/// Returns the frontmost application PID.
fn frontmost_pid() -> Option<u32> {
    // TODO: implement via NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier
    // For now, return None — the caller will fall back to pid 0 which
    // AXUIElementCreateApplication will handle gracefully.
    None
}
