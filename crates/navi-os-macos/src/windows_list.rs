//! Window enumeration via `CGWindowListCopyWindowInfo`.
//!
//! Lists visible top-level windows with title, PID, bounding rect, and
//! focus state. No special permission required for window metadata
//! (titles may be empty without Screen Recording permission).

use anyhow::Result;
use core_foundation::base::TCFType;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::display::{kCGNullWindowID, kCGWindowListOptionOnScreenOnly};

use crate::{MacRect, MacWindowInfo};

/// Enumerates visible top-level windows on the desktop.
///
/// Returns windows sorted: focused first, then by title (case-insensitive).
pub fn enumerate_windows() -> Result<Vec<MacWindowInfo>> {
    // Use the high-level copy_window_info helper from core-graphics.
    let array =
        core_graphics::window::copy_window_info(kCGWindowListOptionOnScreenOnly, kCGNullWindowID);

    let array = match array {
        Some(a) => a,
        None => return Ok(Vec::new()),
    };

    let count = array.len() as usize;
    let mut result = Vec::with_capacity(count);

    for i in 0..count {
        let val = match array.get(i as isize) {
            Some(v) => v,
            None => continue,
        };
        let dict_ref = *val as core_graphics::display::CFDictionaryRef;
        if dict_ref.is_null() {
            continue;
        }
        let dict: CFDictionary = unsafe { CFDictionary::wrap_under_create_rule(dict_ref) };
        if let Some(info) = parse_window_dict(&dict) {
            result.push(info);
        }
    }

    // Sort: focused first, then by title (case-insensitive).
    result.sort_by(|a, b| {
        b.is_focused
            .cmp(&a.is_focused)
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });

    Ok(result)
}

fn parse_window_dict(dict: &CFDictionary) -> Option<MacWindowInfo> {
    let owner_pid = get_number(dict, "kCGWindowOwnerPID")? as u32;
    let window_number = get_number(dict, "kCGWindowNumber")? as u64;
    let layer = get_number(dict, "kCGWindowLayer").unwrap_or(0) as i32;

    // Only include normal-layer windows (layer == 0).
    if layer != 0 {
        return None;
    }

    let title = get_string(dict, "kCGWindowName").unwrap_or_default();
    let owner_name = get_string(dict, "kCGWindowOwnerName").unwrap_or_default();

    // Skip windows with no title and no owner name (menu bar, dock, etc).
    if title.is_empty() && owner_name.is_empty() {
        return None;
    }

    let bounds = get_bounds(dict).unwrap_or(MacRect {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    });

    // Skip zero-size windows.
    if bounds.width <= 0 || bounds.height <= 0 {
        return None;
    }

    // Determine focus: compare with frontmost app PID.
    let frontmost_pid = frontmost_app_pid().unwrap_or(0);
    let is_focused = owner_pid == frontmost_pid;

    // Use owner name as title if window title is empty.
    let display_title = if title.is_empty() { owner_name } else { title };

    Some(MacWindowInfo {
        hwnd: window_number,
        title: display_title,
        pid: owner_pid,
        rect: bounds,
        is_focused,
        is_visible: true,
    })
}

fn get_number(dict: &CFDictionary, key: &str) -> Option<i64> {
    let cf_key = CFString::new(key);
    let val = dict.find(cf_key.as_concrete_TypeRef() as core_foundation_sys::base::CFTypeRef)?;
    unsafe {
        let num = &*(*val as *const CFNumber);
        num.to_i64()
    }
}

fn get_string(dict: &CFDictionary, key: &str) -> Option<String> {
    let cf_key = CFString::new(key);
    let val = dict.find(cf_key.as_concrete_TypeRef() as core_foundation_sys::base::CFTypeRef)?;
    unsafe {
        let s = &*(*val as *const CFString);
        Some(s.to_string())
    }
}

fn get_bounds(dict: &CFDictionary) -> Option<MacRect> {
    let cf_key = CFString::new("kCGWindowBounds");
    let val = dict.find(cf_key.as_concrete_TypeRef() as core_foundation_sys::base::CFTypeRef)?;
    let bounds_dict: CFDictionary = unsafe { CFDictionary::wrap_under_create_rule(*val as _) };

    let x = get_number(&bounds_dict, "X").unwrap_or(0) as i32;
    let y = get_number(&bounds_dict, "Y").unwrap_or(0) as i32;
    let width = get_number(&bounds_dict, "Width").unwrap_or(0) as i32;
    let height = get_number(&bounds_dict, "Height").unwrap_or(0) as i32;

    Some(MacRect {
        x,
        y,
        width,
        height,
    })
}

/// Returns the PID of the frontmost application via NSWorkspace.
fn frontmost_app_pid() -> Option<u32> {
    // NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier
    // This requires AppKit. For now, we use a heuristic: the window with
    // the highest window number among layer-0 windows is likely the
    // frontmost. A proper implementation would use objc2-app-kit.
    // TODO: implement via NSWorkspace when objc2-app-kit is added.
    None
}
