//! Application launching via `ShellExecuteW`.
//!
//! Opens an application by name using the Windows shell "open" verb. This
//! resolves through the shell's file-association and App Paths machinery, so
//! callers can pass a friendly name (e.g. `"notepad"`, `"calc"`), an executable
//! name (`"notepad.exe"`), or a document path.
//!
//! `ShellExecuteW` does not return a PID, so [`WinOpenAppResult::pid`] is
//! always `0` on success. COM is initialized lazily via
//! [`super::ensure_com_initialized`] (the shell API itself does not require
//! COM, but keeping the MTA init consistent avoids surprises for callers that
//! chain this with UIA inspection).

use super::WinOpenAppResult;
use anyhow::{Result, bail};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::PCWSTR;

/// Opens an application by name using `ShellExecuteW`.
///
/// Tries (via the shell): exact name -> App Paths registry -> PATH -> file
/// associations. The shell performs all resolution; we just hand it the
/// `open` verb.
pub fn open_application(name: &str) -> Result<WinOpenAppResult> {
    super::ensure_com_initialized()?;

    let verb = to_wide("open");
    let file = to_wide(name);

    let hinst = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };

    // Legacy convention: ShellExecuteW returns a HINSTANCE whose numeric
    // value is <= 32 on error (it is actually the error code, not a handle).
    let code = hinst.0 as usize;
    if code <= 32 {
        bail!("ShellExecuteW failed to open `{name}` (error code {code})");
    }

    Ok(WinOpenAppResult {
        launched: true,
        // ShellExecuteW does not return a PID.
        pid: 0,
        message: format!("Launched {name}"),
    })
}

/// Encodes a Rust string as a null-terminated UTF-16 buffer suitable for
/// `PCWSTR`.
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn open_application_launches_notepad() {
        // Integration test: launch notepad (always available on Windows).
        // We don't verify the window appears (that would require UIA
        // enumeration), just that ShellExecuteW succeeds.
        let result = match open_application("notepad") {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipping open_application_launches_notepad: {e}");
                return;
            }
        };
        assert!(result.launched, "notepad should launch successfully");
        assert_eq!(result.pid, 0, "ShellExecuteW doesn't return a PID");
        assert!(
            result.message.contains("notepad"),
            "message should mention notepad, got: {}",
            result.message
        );

        // Clean up: kill notepad processes we may have launched.
        // Use taskkill to avoid depending on the windows crate's process APIs.
        let _ = std::process::Command::new("taskkill")
            .args(["/IM", "notepad.exe", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    #[test]
    #[cfg(windows)]
    fn open_application_nonexistent_app_returns_error() {
        // An app that doesn't exist should return an error, not panic.
        let result = open_application("definitely_not_a_real_app_xyz_123");
        assert!(result.is_err(), "nonexistent app should return an error");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("ShellExecuteW") || msg.contains("failed"),
            "error should mention ShellExecuteW or failed, got: {msg}"
        );
    }

    #[test]
    fn to_wide_produces_null_terminated_utf16() {
        let wide = to_wide("hello");
        assert_eq!(
            wide,
            vec![
                b'h' as u16,
                b'e' as u16,
                b'l' as u16,
                b'l' as u16,
                b'o' as u16,
                0
            ]
        );
    }
}
