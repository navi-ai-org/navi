//! Application launching via the macOS `open` command.
//!
//! `open -a "App Name"` launches an application through LaunchServices.
//! This bypasses the GUI entirely (no Spotlight/Start menu simulation).

use anyhow::{Result, anyhow};

use crate::MacOpenAppResult;

/// Opens an application by name using `open -a`.
///
/// On macOS, `open -a "App Name"` launches the app via LaunchServices. The
/// `open` command does not return the launched process's PID, so `pid` is
/// always 0 in the result.
pub fn open_application(name: &str) -> Result<MacOpenAppResult> {
    let output = std::process::Command::new("open")
        .arg("-a")
        .arg(name)
        .output()
        .map_err(|e| anyhow!("failed to run `open -a {name}`: {e}"))?;

    if output.status.success() {
        Ok(MacOpenAppResult {
            launched: true,
            pid: 0, // `open` doesn't return a PID.
            message: format!("Launched {name}"),
        })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Ok(MacOpenAppResult {
            launched: false,
            pid: 0,
            message: format!("Failed to launch {name}: {stderr}"),
        })
    }
}
