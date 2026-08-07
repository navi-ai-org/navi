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
