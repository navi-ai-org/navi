//! NAVI self-update: check GitHub Releases and apply via the official installer.
//!
//! Modern frictionless path:
//! 1. `check_for_update` → compare current `CARGO_PKG_VERSION` to latest release tag
//! 2. TUI / SDK surfaces the result
//! 3. `apply_update` re-runs `install.sh` (or install.ps1 on Windows) pinned to that version

use anyhow::{Context, Result};
use serde::Deserialize;

const DEFAULT_REPO: &str = "navi-ai-org/navi";
const INSTALL_SH: &str =
    "https://github.com/navi-ai-org/navi/raw/refs/heads/main/scripts/install.sh";
const INSTALL_PS1: &str =
    "https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.ps1";

/// Information about an available NAVI release that is newer than the running binary.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UpdateInfo {
    /// Currently running version (semver without leading `v`).
    pub current_version: String,
    /// Latest GitHub release tag (may include leading `v`).
    pub latest_tag: String,
    /// Latest version normalized without leading `v`.
    pub latest_version: String,
    /// HTML URL of the release page.
    pub release_url: String,
    /// Release body / notes when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Whether this is a prerelease.
    #[serde(default)]
    pub prerelease: bool,
}

impl UpdateInfo {
    pub fn is_newer(&self) -> bool {
        version_is_newer(&self.latest_version, &self.current_version)
    }
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

/// Running binary version (from the crate that embeds this code at build time
/// for the CLI; callers may override with an explicit current version).
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Normalize a tag or version string to bare semver (`v0.2.3` → `0.2.3`).
pub fn normalize_version(v: &str) -> String {
    v.trim().trim_start_matches('v').trim().to_string()
}

/// Compare two bare semver strings. Returns true if `candidate` is strictly greater.
pub fn version_is_newer(candidate: &str, current: &str) -> bool {
    let c = parse_semver(candidate);
    let cur = parse_semver(current);
    c > cur
}

fn parse_semver(v: &str) -> (u64, u64, u64) {
    let v = normalize_version(v);
    let mut parts = v.split(['.', '-', '+']);
    let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

/// Check GitHub Releases for a newer version of NAVI.
///
/// Returns `Ok(None)` when already up to date (or latest is a draft/prerelease
/// older/equal). Network failures return `Err`.
pub async fn check_for_update(
    current: &str,
    repo: Option<&str>,
    include_prerelease: bool,
) -> Result<Option<UpdateInfo>> {
    let repo = repo.unwrap_or(DEFAULT_REPO);
    let current_version = normalize_version(current);
    let url = if include_prerelease {
        format!("https://api.github.com/repos/{repo}/releases?per_page=5")
    } else {
        format!("https://api.github.com/repos/{repo}/releases/latest")
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(format!("navi/{current_version}"))
        .build()
        .context("build HTTP client for update check")?;

    let release = if include_prerelease {
        let list: Vec<GhRelease> = client
            .get(&url)
            .send()
            .await
            .context("fetch releases list")?
            .error_for_status()
            .context("releases list HTTP error")?
            .json()
            .await
            .context("parse releases list")?;
        list.into_iter()
            .find(|r| !r.draft && (include_prerelease || !r.prerelease))
            .context("no suitable release found")?
    } else {
        client
            .get(&url)
            .send()
            .await
            .context("fetch latest release")?
            .error_for_status()
            .context("latest release HTTP error")?
            .json::<GhRelease>()
            .await
            .context("parse latest release")?
    };

    if release.draft {
        return Ok(None);
    }
    if release.prerelease && !include_prerelease {
        return Ok(None);
    }

    let latest_version = normalize_version(&release.tag_name);
    if !version_is_newer(&latest_version, &current_version) {
        return Ok(None);
    }

    Ok(Some(UpdateInfo {
        current_version,
        latest_tag: release.tag_name,
        latest_version,
        release_url: release.html_url,
        body: release.body.filter(|b| !b.trim().is_empty()),
        prerelease: release.prerelease,
    }))
}

/// Apply an update by re-running the official installer for `info.latest_version`.
///
/// Spawns the platform installer and waits for completion. On success the
/// new binary is on disk; the running process should exit so the user restarts.
///
/// **Important:** the installer process does **not** inherit the parent
/// stdout/stderr. That would paint raw ANSI progress over an active TUI
/// alternate screen. Output is captured and only attached to errors.
pub async fn apply_update(info: &UpdateInfo) -> Result<()> {
    let version = info.latest_version.clone();
    tokio::task::spawn_blocking(move || apply_update_blocking(&version))
        .await
        .context("update task join")??;
    Ok(())
}

/// Run a child process with piped stdio so TUI/alternate-screen sessions are
/// not corrupted by installer progress ANSI. Captured output is kept for errors.
fn run_silent(mut cmd: std::process::Command) -> Result<()> {
    use std::process::Stdio;

    // Discourage colored progress from install.sh / powershell host noise.
    cmd.env("NO_COLOR", "1");
    cmd.env("TERM", "dumb");
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = cmd
        .output()
        .with_context(|| format!("spawn installer: {:?}", cmd.get_program()))?;

    if output.status.success() {
        // Success path: discard installer chatter (TUI already shows its own toast).
        if !output.stdout.is_empty() {
            tracing::debug!(bytes = output.stdout.len(), "installer stdout (suppressed)");
        }
        if !output.stderr.is_empty() {
            tracing::debug!(bytes = output.stderr.len(), "installer stderr (suppressed)");
        }
        return Ok(());
    }

    let tail = installer_error_tail(&output.stdout, &output.stderr);
    if tail.is_empty() {
        anyhow::bail!("installer exited with {}", output.status);
    }
    anyhow::bail!("installer exited with {}: {}", output.status, tail);
}

fn installer_error_tail(stdout: &[u8], stderr: &[u8]) -> String {
    let mut combined = String::new();
    if !stderr.is_empty() {
        combined.push_str(&String::from_utf8_lossy(stderr));
    }
    if !stdout.is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(&String::from_utf8_lossy(stdout));
    }
    // Strip ANSI so error notifications stay readable in the TUI.
    let plain = strip_ansi(&combined);
    // Keep last ~1.5 KiB of meaningful lines.
    let trimmed = plain.trim();
    if trimmed.len() <= 1500 {
        return trimmed.to_string();
    }
    let start = trimmed.len().saturating_sub(1500);
    // Prefer cutting on a newline boundary.
    let slice = &trimmed[start..];
    match slice.find('\n') {
        Some(i) => slice[i + 1..].trim().to_string(),
        None => slice.trim().to_string(),
    }
}

fn strip_ansi(s: &str) -> String {
    // Minimal CSI / OSC stripper for installer progress codes (no dependency).
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                // CSI: ESC [ ... final byte @-~
                for next in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                // OSC: ESC ] ... BEL or ST (ESC \)
                while let Some(next) = chars.next() {
                    if next == '\u{07}' {
                        break;
                    }
                    if next == '\u{1b}' && matches!(chars.peek(), Some('\\')) {
                        let _ = chars.next();
                        break;
                    }
                }
            }
            Some(_) | None => {}
        }
    }
    out
}

fn apply_update_blocking(version: &str) -> Result<()> {
    let version = normalize_version(version);
    match std::env::consts::OS {
        "windows" => {
            // Download install.ps1 and run with -Version
            let mut primary = std::process::Command::new("powershell");
            primary.args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &format!(
                    "irm {INSTALL_PS1} | iex; if (Get-Command Install-Navi -ErrorAction SilentlyContinue) {{ Install-Navi -Version {version} }} else {{ & ([scriptblock]::Create((irm {INSTALL_PS1}))) -Version {version} }}"
                ),
            ]);
            if run_silent(primary).is_ok() {
                return Ok(());
            }
            // Fallback: curl-style via iwr to temp
            let tmp = std::env::temp_dir().join("navi-install.ps1");
            let mut download = std::process::Command::new("powershell");
            download.args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Invoke-WebRequest -Uri '{INSTALL_PS1}' -OutFile '{}'",
                    tmp.display()
                ),
            ]);
            run_silent(download).context("download install.ps1")?;
            let mut install = std::process::Command::new("powershell");
            install.args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                tmp.to_str().unwrap_or("navi-install.ps1"),
                "-Version",
                &version,
            ]);
            run_silent(install).context("run install.ps1")?;
            Ok(())
        }
        _ => {
            // curl | sh with pinned version (checksum verified by install.sh).
            // Stdio is piped (not inherited) so an active TUI is not corrupted.
            let mut cmd = std::process::Command::new("sh");
            cmd.args([
                "-c",
                &format!("curl -fsSL {INSTALL_SH} | sh -s -- --version {version}"),
            ]);
            run_silent(cmd).context("run install.sh")?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_v() {
        assert_eq!(normalize_version("v0.2.3"), "0.2.3");
        assert_eq!(normalize_version("0.2.3"), "0.2.3");
    }

    #[test]
    fn semver_compare() {
        assert!(version_is_newer("0.2.3", "0.2.2"));
        assert!(version_is_newer("1.0.0", "0.9.9"));
        assert!(!version_is_newer("0.2.2", "0.2.3"));
        assert!(!version_is_newer("0.2.3", "0.2.3"));
    }

    #[test]
    fn strip_ansi_removes_csi_sequences() {
        let raw = "\x1b[1mlinux-x64\x1b[0m installed to \x1b[1m/home/enrell/.local/bin/navi\x1b[0m";
        assert_eq!(
            strip_ansi(raw),
            "linux-x64 installed to /home/enrell/.local/bin/navi"
        );
    }

    #[test]
    fn installer_error_tail_prefers_stderr_and_strips_ansi() {
        let stderr = b"\x1b[0;31m[navi]\x1b[0m boom\n";
        let stdout = b"progress line\n";
        let tail = installer_error_tail(stdout, stderr);
        assert!(tail.contains("[navi] boom"));
        assert!(tail.contains("progress line"));
        assert!(!tail.contains('\u{1b}'));
    }

    #[test]
    fn run_silent_does_not_inherit_stdio_and_captures_failure() {
        // A tiny failing command must not write to our inherited stdout.
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "printf '\\033[1mFAIL\\033[0m\\n' >&2; exit 7"]);
        let err = run_silent(cmd).expect_err("expected failure");
        let msg = format!("{err:#}");
        assert!(msg.contains("exit"), "{msg}");
        assert!(msg.contains("FAIL"), "{msg}");
        assert!(!msg.contains('\u{1b}'), "{msg}");
    }
}
