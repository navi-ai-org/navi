//! Target app resolution for the computer-use deny-list (ADR 0016).
//!
//! Given a screen coordinate or the foreground window, resolves the owning
//! process and returns its exe name (lowercase, without `.exe`) and window
//! title. The `SimulateInputTool` uses this to check whether the target app
//! is on the deny-list before dispatching input.
//!
//! - Mouse actions (`click`, `scroll`, `mouse_move`) → `WindowFromPoint(x, y)`
//! - Keyboard actions (`key`, `type`) → `GetForegroundWindow`
//! - PID → exe name via `QueryFullProcessImageNameW` (handles elevated
//!   targets via `PROCESS_QUERY_LIMITED_INFORMATION`)

use super::WinTargetApp;
use anyhow::Result;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, WindowFromPoint,
};

/// Resolves the target app at a screen coordinate (for mouse actions).
pub fn resolve_target_for_point(x: i32, y: i32) -> Option<WinTargetApp> {
    unsafe {
        let hwnd = WindowFromPoint(POINT { x, y });
        if hwnd.0.is_null() {
            return None;
        }
        Some(build_target(hwnd))
    }
}

/// Resolves the target app from the foreground window (for keyboard actions).
pub fn resolve_target_foreground() -> Option<WinTargetApp> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        Some(build_target(hwnd))
    }
}

/// Builds a `WinTargetApp` from an HWND: PID, exe name, window title.
unsafe fn build_target(hwnd: HWND) -> WinTargetApp {
    unsafe {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let exe_name = process_exe_name(pid).unwrap_or_default();
        let title = get_window_text(hwnd);
        WinTargetApp {
            pid,
            exe_name,
            window_title: title,
        }
    }
}

/// Returns the exe name (lowercase, without `.exe`) for a PID.
///
/// Uses `PROCESS_QUERY_LIMITED_INFORMATION` which works against elevated
/// processes (unlike `PROCESS_QUERY_INFORMATION`).
fn process_exe_name(pid: u32) -> Result<String> {
    use windows::Win32::System::Threading::QueryFullProcessImageNameW;
    use windows::core::PWSTR;

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?;
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        // Extract the file name (last segment) and strip `.exe`.
        let exe = path.rsplit(['\\', '/']).next().unwrap_or(&path);
        let exe = exe.strip_suffix(".exe").unwrap_or(exe);
        Ok(exe.to_ascii_lowercase())
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
        GetWindowTextW(hwnd, &mut buf);
        let actual_len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..actual_len])
    }
}

/// Returns the bounding rectangle of a window, if resolvable.
#[allow(dead_code)]
pub fn window_rect(hwnd: HWND) -> Option<RECT> {
    unsafe {
        let mut rect: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rect).ok()?;
        Some(rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn resolve_target_foreground_returns_something_or_none() {
        // In a headless CI runner there may be no foreground window.
        // We only assert the call doesn't panic.
        let _ = resolve_target_foreground();
    }

    #[test]
    #[cfg(windows)]
    fn resolve_target_for_point_does_not_panic() {
        // (0, 0) is the top-left of the screen; there's usually a window there.
        let _ = resolve_target_for_point(0, 0);
    }
}
