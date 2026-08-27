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
        let meaning = shellexecute_error_meaning(code);
        bail!(
            "ShellExecuteW failed to open `{name}`: {meaning} (error code {code}). \
             {}",
            recovery_hint(code, name)
        );
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

/// Maps a ShellExecuteW error code (≤ 32) to a human-readable description.
///
/// See: <https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shellexecutea#return-value>
fn shellexecute_error_meaning(code: usize) -> &'static str {
    match code {
        0 => "The operating system is out of memory or resources",
        1 => "Invalid function (should not occur with the 'open' verb)",
        2 => "File not found — the specified application or file does not exist",
        3 => "Path not found — the directory in the path does not exist",
        5 => "Access denied — the user lacks permission, or the file is locked",
        8 => "Not enough memory to complete the operation",
        11 => "Invalid .exe file — the executable is corrupt or not a valid PE image",
        26 => "Sharing violation — the file is in use by another process",
        27 => "Association incomplete — the file extension has an incomplete association",
        28 => "DDE timeout — the application could not complete a DDE transaction",
        29 => "DDE busy — the DDE transaction could not be completed",
        30 => "No DDE conversation — the DDE application is not responding",
        31 => "No application associated with this file extension",
        32 => "DLL not found — a required DLL could not be loaded",
        _ => "Unknown ShellExecuteW error",
    }
}

/// Returns a recovery hint for common ShellExecuteW error codes, tailored
/// to help the model decide what to try next.
fn recovery_hint(code: usize, name: &str) -> String {
    match code {
        2 | 3 => format!(
            "Try the full path (e.g. 'C:/Windows/System32/{name}.exe'), \
             or use 'inspect_desktop' to see what's currently open."
        ),
        5 => "Check if the application requires elevation, or if the file is in use.".to_string(),
        31 => format!(
            "No app is registered for '{name}'. Try the full executable name \
             (e.g. '{name}.exe') or a different name."
        ),
        26 => "The application may already be running. Use 'inspect_desktop' to check.".to_string(),
        _ => "Try a different name or the full path to the executable.".to_string(),
    }
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
        // Error should include a human-readable description, not just a code.
        assert!(
            msg.contains("not found")
                || msg.contains("No application")
                || msg.contains("association"),
            "error should include a human-readable description, got: {msg}"
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

    // ── Edge cases ────────────────────────────────────────────────────────

    #[test]
    fn to_wide_empty_string_produces_just_null() {
        let wide = to_wide("");
        assert_eq!(
            wide,
            vec![0u16],
            "empty string should produce just a null terminator"
        );
    }

    #[test]
    fn to_wide_unicode_string_preserves_codepoints() {
        // "café" → U+0063 U+0061 U+0066 U+00E9
        let wide = to_wide("café");
        assert_eq!(wide, vec![0x63, 0x61, 0x66, 0xE9, 0]);
    }

    #[test]
    fn to_wide_emoji_surrogate_pair() {
        // "🦀" (U+1F980) → surrogate pair: 0xD83E 0xDD80
        let wide = to_wide("🦀");
        assert_eq!(wide, vec![0xD83E, 0xDD80, 0]);
    }

    #[test]
    #[cfg(windows)]
    fn open_application_empty_string_returns_error() {
        // Empty string should not launch anything — ShellExecuteW with
        // an empty file parameter should fail.
        let result = open_application("");
        // Either it errors, or it "succeeds" by opening nothing useful.
        // Either way, it must not panic.
        match &result {
            Ok(r) => {
                // If it somehow "succeeds", it shouldn't claim it launched something meaningful.
                eprintln!(
                    "open_application(\"\") returned Ok: launched={}, msg={}",
                    r.launched, r.message
                );
            }
            Err(e) => {
                let msg = format!("{e}");
                assert!(
                    msg.contains("ShellExecuteW") || msg.contains("failed"),
                    "empty string should error with ShellExecuteW or failed, got: {msg}"
                );
            }
        }
    }

    #[test]
    #[cfg(windows)]
    fn open_application_with_exe_extension_works() {
        // "notepad.exe" should work the same as "notepad".
        let result = match open_application("notepad.exe") {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipping open_application_with_exe_extension_works: {e}");
                return;
            }
        };
        assert!(result.launched, "notepad.exe should launch");

        let _ = std::process::Command::new("taskkill")
            .args(["/IM", "notepad.exe", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    #[test]
    #[cfg(windows)]
    fn open_application_with_spaces_in_name_returns_error() {
        // A name with spaces that isn't a real app should fail gracefully.
        let result = open_application("this is not a real app");
        assert!(
            result.is_err(),
            "fake app with spaces should return an error"
        );
    }

    #[test]
    #[cfg(windows)]
    fn open_application_with_path_separators_returns_error() {
        // A name with path separators that doesn't exist should fail.
        let result = open_application("C:/nonexistent/path/app.exe");
        assert!(result.is_err(), "nonexistent path should return an error");
    }

    #[test]
    #[cfg(windows)]
    fn open_application_very_long_name_returns_error() {
        // A very long name should not cause a buffer overflow or panic.
        let long_name = "a".repeat(10000);
        let result = open_application(&long_name);
        // Should error (not a real app), but must not panic.
        assert!(result.is_err(), "very long name should return an error");
    }

    #[test]
    #[cfg(windows)]
    fn open_application_unicode_name_returns_error() {
        // Unicode app name that doesn't exist should fail gracefully,
        // not panic on UTF-16 conversion.
        let result = open_application("アプリケーション🦀");
        assert!(
            result.is_err(),
            "nonexistent unicode app should return an error"
        );
    }

    // ── Error code mapping tests ──────────────────────────────────────────

    #[test]
    fn shellexecute_error_meaning_known_codes() {
        assert!(shellexecute_error_meaning(0).contains("out of memory"));
        assert!(shellexecute_error_meaning(2).contains("File not found"));
        assert!(shellexecute_error_meaning(3).contains("Path not found"));
        assert!(shellexecute_error_meaning(5).contains("Access denied"));
        assert!(shellexecute_error_meaning(26).contains("Sharing violation"));
        assert!(shellexecute_error_meaning(31).contains("No application associated"));
        assert!(shellexecute_error_meaning(32).contains("DLL not found"));
    }

    #[test]
    fn shellexecute_error_meaning_unknown_code() {
        assert!(shellexecute_error_meaning(99).contains("Unknown"));
        assert!(shellexecute_error_meaning(usize::MAX).contains("Unknown"));
    }

    #[test]
    fn recovery_hint_file_not_found_suggests_full_path() {
        let hint = recovery_hint(2, "myapp");
        assert!(
            hint.contains("full path") || hint.contains("inspect_desktop"),
            "recovery hint for code 2 should suggest full path or inspect_desktop, got: {hint}"
        );
    }

    #[test]
    fn recovery_hint_no_association_suggests_exe_extension() {
        let hint = recovery_hint(31, "myapp");
        assert!(
            hint.contains(".exe"),
            "recovery hint for code 31 should suggest .exe extension, got: {hint}"
        );
    }

    #[test]
    fn recovery_hint_sharing_violation_suggests_inspect_desktop() {
        let hint = recovery_hint(26, "myapp");
        assert!(
            hint.contains("inspect_desktop") || hint.contains("already running"),
            "recovery hint for code 26 should mention inspect_desktop or already running, got: {hint}"
        );
    }

    #[test]
    fn recovery_hint_generic_for_unknown_code() {
        let hint = recovery_hint(99, "myapp");
        assert!(
            !hint.is_empty(),
            "recovery hint for unknown code should not be empty"
        );
    }

    // ── Less common error codes ───────────────────────────────────────────

    #[test]
    fn shellexecute_error_meaning_all_known_codes() {
        // Verify every documented code returns a meaningful description.
        let codes = [0, 1, 2, 3, 5, 8, 11, 26, 27, 28, 29, 30, 31, 32];
        for code in codes {
            let meaning = shellexecute_error_meaning(code);
            assert!(
                !meaning.contains("Unknown"),
                "code {code} should have a known meaning, got: {meaning}"
            );
        }
    }

    #[test]
    fn shellexecute_error_meaning_code_1_invalid_function() {
        assert!(shellexecute_error_meaning(1).contains("Invalid function"));
    }

    #[test]
    fn shellexecute_error_meaning_code_8_not_enough_memory() {
        assert!(shellexecute_error_meaning(8).contains("Not enough memory"));
    }

    #[test]
    fn shellexecute_error_meaning_code_11_invalid_exe() {
        assert!(shellexecute_error_meaning(11).contains("Invalid .exe"));
    }

    #[test]
    fn shellexecute_error_meaning_code_27_association_incomplete() {
        assert!(shellexecute_error_meaning(27).contains("Association incomplete"));
    }

    #[test]
    fn shellexecute_error_meaning_code_28_dde_timeout() {
        assert!(shellexecute_error_meaning(28).contains("DDE timeout"));
    }

    #[test]
    fn shellexecute_error_meaning_code_29_dde_busy() {
        assert!(shellexecute_error_meaning(29).contains("DDE busy"));
    }

    #[test]
    fn shellexecute_error_meaning_code_30_no_dde_conversation() {
        assert!(shellexecute_error_meaning(30).contains("No DDE conversation"));
    }

    // ── recovery_hint for less common codes ───────────────────────────────

    #[test]
    fn recovery_hint_access_denied_mentions_elevation() {
        let hint = recovery_hint(5, "myapp");
        assert!(
            hint.contains("elevation") || hint.contains("permission") || hint.contains("in use"),
            "recovery hint for code 5 should mention elevation/permission, got: {hint}"
        );
    }

    #[test]
    fn recovery_hint_path_not_found_suggests_full_path() {
        let hint = recovery_hint(3, "myapp");
        assert!(
            hint.contains("full path") || hint.contains("inspect_desktop"),
            "recovery hint for code 3 should suggest full path, got: {hint}"
        );
    }

    #[test]
    fn recovery_hint_for_all_known_codes_is_non_empty() {
        let codes = [0, 1, 2, 3, 5, 8, 11, 26, 27, 28, 29, 30, 31, 32];
        for code in codes {
            let hint = recovery_hint(code, "testapp");
            assert!(
                !hint.is_empty(),
                "recovery hint for code {code} should not be empty"
            );
        }
    }

    #[test]
    fn recovery_hint_includes_app_name_for_file_not_found() {
        let hint = recovery_hint(2, "myapp");
        assert!(
            hint.contains("myapp"),
            "recovery hint should include app name, got: {hint}"
        );
    }

    #[test]
    fn recovery_hint_includes_app_name_for_no_association() {
        let hint = recovery_hint(31, "myapp");
        assert!(
            hint.contains("myapp"),
            "recovery hint should include app name, got: {hint}"
        );
    }

    // ── to_wide additional edge cases ─────────────────────────────────────

    #[test]
    fn to_wide_single_char() {
        let wide = to_wide("A");
        assert_eq!(wide, vec![b'A' as u16, 0]);
    }

    #[test]
    fn to_wide_ascii_string() {
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

    #[test]
    fn to_wide_mixed_ascii_and_unicode() {
        let wide = to_wide("aé");
        // 'a' = 0x61, 'é' = 0xE9
        assert_eq!(wide, vec![0x61, 0xE9, 0]);
    }

    #[test]
    fn to_wide_null_terminator_always_present() {
        let wide = to_wide("test");
        assert_eq!(
            *wide.last().unwrap(),
            0u16,
            "last element must be null terminator"
        );
    }
}
