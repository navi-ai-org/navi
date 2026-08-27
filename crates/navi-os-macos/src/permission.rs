//! Accessibility permission check via `AXIsProcessTrusted()`.
//!
//! Returns whether the current process has been granted Accessibility
//! permission in System Settings → Privacy & Security → Accessibility.

/// Returns `true` if the process has Accessibility permission.
pub fn is_accessibility_trusted() -> bool {
    unsafe { accessibility_sys::AXIsProcessTrusted() }
}
