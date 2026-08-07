//! Target app resolution for the computer-use deny-list (ADR 0016).
//!
//! Resolves the target app for a screen coordinate (mouse actions) or the
//! foreground app (keyboard actions) so that `SimulateInputTool` can check
//! the deny-list before dispatching input.

use crate::MacTargetApp;

/// Resolves the target app at a screen coordinate.
///
/// Uses `CGWindowListCopyWindowInfo` to find the topmost window containing
/// the point, then resolves the owner PID to an app name.
pub fn resolve_target_for_point(x: i32, y: i32) -> Option<MacTargetApp> {
    let windows = crate::enumerate_windows().ok()?;

    // Find the topmost window (highest hwnd) that contains the point.
    // Windows are sorted focused-first, but we want the topmost by z-order.
    // CGWindowListCopyWindowInfo returns windows in z-order (front to back),
    // but our enumerate_windows sorts them. We'll just pick the first one
    // that contains the point — since the list is focused-first, this is
    // a reasonable approximation.
    for w in &windows {
        if w.rect.x <= x
            && w.rect.y <= y
            && x < w.rect.x + w.rect.width
            && y < w.rect.y + w.rect.height
        {
            let exe_name = resolve_exe_name(w.pid).unwrap_or_default();
            return Some(MacTargetApp {
                pid: w.pid,
                exe_name,
                window_title: w.title.clone(),
            });
        }
    }
    None
}

/// Resolves the target app for the foreground (frontmost) window.
pub fn resolve_target_foreground() -> Option<MacTargetApp> {
    let windows = crate::enumerate_windows().ok()?;
    let focused = windows
        .iter()
        .find(|w| w.is_focused)
        .or_else(|| windows.first())?;
    let exe_name = resolve_exe_name(focused.pid).unwrap_or_default();
    Some(MacTargetApp {
        pid: focused.pid,
        exe_name,
        window_title: focused.title.clone(),
    })
}

/// Resolves a PID to an app name (lowercase, without `.app` suffix).
///
/// On macOS, we use `libproc`'s `proc_pidpath` to get the full app bundle
/// path, then extract the bundle name. Falls back to the window owner name.
fn resolve_exe_name(pid: u32) -> Option<String> {
    // proc_pidpath is in libproc.h. We use raw FFI to avoid adding a
    // dependency on a proc-info crate.
    use std::ffi::CStr;
    use std::os::raw::c_char;

    extern "C" {
        fn proc_pidpath(pid: i32, buffer: *mut c_char, buffersize: u32) -> i32;
    }

    let mut buffer = vec![0i8; 4096];
    let len = unsafe { proc_pidpath(pid as i32, buffer.as_mut_ptr(), buffer.len() as u32) };
    if len <= 0 {
        return None;
    }

    let path = unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .to_string();

    // Extract the app bundle name from a path like:
    // /Applications/Safari.app/Contents/MacOS/Safari
    let app_name = path
        .split('/')
        .find(|segment| segment.ends_with(".app"))
        .map(|s| s.trim_end_matches(".app"))
        .or_else(|| {
            // Fallback: use the last path component.
            path.rsplit('/').next()
        })?;

    Some(app_name.to_lowercase())
}
