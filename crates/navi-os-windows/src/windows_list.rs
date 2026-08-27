//! Window enumeration via `EnumWindows`.

use super::{WinRect, WinWindowInfo};
use anyhow::Result;
use std::mem;
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible,
};

/// Enumerates all visible top-level windows.
///
/// Returns title, PID, bounding rectangle, and focus state for each window.
pub fn enumerate_windows() -> Result<Vec<WinWindowInfo>> {
    let mut windows: Vec<WinWindowInfo> = Vec::new();
    let state_ptr = &mut windows as *mut Vec<WinWindowInfo> as LPARAM;

    unsafe {
        EnumWindows(Some(enum_callback), state_ptr);
    }

    // Sort: focused window first, then by title for stable output.
    windows.sort_by(|a, b| {
        b.is_focused
            .cmp(&a.is_focused)
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });

    Ok(windows)
}

unsafe extern "system" fn enum_callback(hwnd: HWND, lparam: LPARAM) -> i32 {
    unsafe {
        let windows = &mut *(lparam as *mut Vec<WinWindowInfo>);

        // Skip invisible windows.
        if IsWindowVisible(hwnd) == 0 {
            return 1;
        }

        // Get window title.
        let title = get_window_text(hwnd);

        // Skip windows with no title (background windows, tooltips, etc.)
        if title.is_empty() {
            return 1;
        }

        // Get window rect.
        let mut rect: RECT = mem::zeroed();
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return 1;
        }

        // Get PID.
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);

        let focused = GetForegroundWindow() == hwnd;

        windows.push(WinWindowInfo {
            hwnd: hwnd as u64,
            title,
            pid,
            rect: WinRect {
                x: rect.left,
                y: rect.top,
                width: rect.right - rect.left,
                height: rect.bottom - rect.top,
            },
            is_focused: focused,
            is_visible: true,
        });

        1
    }
}

/// Reads the window title via `GetWindowTextW`.
unsafe fn get_window_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len as usize) + 1];
        GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        // Trim to the null terminator.
        let actual_len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..actual_len])
    }
}
