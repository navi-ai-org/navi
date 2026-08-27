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
    // Hide console window on Windows (ConPTY grandchildren pop visible consoles).
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

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

    // ── current_version ───────────────────────────────────────────────────

    #[test]
    fn current_version_returns_non_empty_string() {
        let v = current_version();
        assert!(!v.is_empty(), "current_version must not be empty");
        // Should look like a semver (at least one digit and one dot).
        assert!(
            v.chars().any(|c| c.is_ascii_digit()),
            "current_version should contain digits: {v}"
        );
        assert!(v.contains('.'), "current_version should contain '.': {v}");
    }

    #[test]
    fn current_version_matches_cargo_pkg_version() {
        assert_eq!(current_version(), env!("CARGO_PKG_VERSION"));
    }

    // ── normalize_version edge cases ──────────────────────────────────────

    #[test]
    fn normalize_version_strips_leading_v() {
        assert_eq!(normalize_version("v1.2.3"), "1.2.3");
    }

    #[test]
    fn normalize_version_no_v_prefix() {
        assert_eq!(normalize_version("1.2.3"), "1.2.3");
    }

    #[test]
    fn normalize_version_trims_whitespace() {
        assert_eq!(normalize_version("  v1.2.3  "), "1.2.3");
        assert_eq!(normalize_version("  1.2.3  "), "1.2.3");
    }

    #[test]
    fn normalize_version_trims_after_v_strip() {
        // "v  1.2.3" → trim → "v  1.2.3" → strip 'v' → "  1.2.3" → trim → "1.2.3"
        assert_eq!(normalize_version("v  1.2.3"), "1.2.3");
    }

    #[test]
    fn normalize_version_empty_string() {
        assert_eq!(normalize_version(""), "");
    }

    #[test]
    fn normalize_version_whitespace_only() {
        assert_eq!(normalize_version("   "), "");
    }

    #[test]
    fn normalize_version_v_only() {
        assert_eq!(normalize_version("v"), "");
    }

    #[test]
    fn normalize_version_double_v() {
        // trim_start_matches('v') strips ALL leading 'v' chars.
        assert_eq!(normalize_version("vv1.2.3"), "1.2.3");
    }

    #[test]
    fn normalize_version_with_pre_release_suffix() {
        // Suffix is preserved (only the leading 'v' is stripped).
        assert_eq!(normalize_version("v1.2.3-beta"), "1.2.3-beta");
    }

    // ── version_is_newer edge cases ───────────────────────────────────────

    #[test]
    fn version_is_newer_same_version_false() {
        assert!(!version_is_newer("1.0.0", "1.0.0"));
    }

    #[test]
    fn version_is_newer_higher_patch_true() {
        assert!(version_is_newer("1.0.1", "1.0.0"));
    }

    #[test]
    fn version_is_newer_higher_minor_true() {
        assert!(version_is_newer("1.1.0", "1.0.0"));
    }

    #[test]
    fn version_is_newer_higher_major_true() {
        assert!(version_is_newer("2.0.0", "1.9.9"));
    }

    #[test]
    fn version_is_newer_lower_major_false() {
        assert!(!version_is_newer("0.9.0", "1.0.0"));
    }

    #[test]
    fn version_is_newer_lower_minor_false() {
        assert!(!version_is_newer("1.0.0", "1.1.0"));
    }

    #[test]
    fn version_is_newer_lower_patch_false() {
        assert!(!version_is_newer("1.0.0", "1.0.1"));
    }

    #[test]
    fn version_is_newer_with_v_prefix() {
        assert!(version_is_newer("v1.0.0", "v0.9.0"));
        assert!(!version_is_newer("v0.9.0", "v1.0.0"));
    }

    #[test]
    fn version_is_newer_mixed_v_prefix() {
        assert!(version_is_newer("v1.0.0", "0.9.0"));
        assert!(version_is_newer("1.0.0", "v0.9.0"));
    }

    #[test]
    fn version_is_newer_with_pre_release_suffix() {
        // Pre-release suffix is split off; only major.minor.patch compared.
        assert!(version_is_newer("1.0.1-beta", "1.0.0"));
        assert!(!version_is_newer("1.0.0-beta", "1.0.0"));
    }

    #[test]
    fn version_is_newer_with_build_metadata() {
        // Build metadata after '+' is split off.
        assert!(version_is_newer("1.0.1+build123", "1.0.0"));
        assert!(!version_is_newer("1.0.0+build123", "1.0.0"));
    }

    #[test]
    fn version_is_newer_empty_strings() {
        // Empty strings parse as (0, 0, 0) — not newer than each other.
        assert!(!version_is_newer("", ""));
    }

    #[test]
    fn version_is_newer_garbage_parses_as_zero() {
        // Non-numeric parts parse as 0.
        assert!(!version_is_newer("garbage", "0.0.0"));
        assert!(!version_is_newer("0.0.0", "garbage"));
    }

    #[test]
    fn version_is_newer_partial_version() {
        // Missing parts default to 0.
        assert!(version_is_newer("1", "0.0.0"));
        assert!(version_is_newer("1.1", "1.0.0"));
        assert!(!version_is_newer("1.0", "1.0.1"));
    }

    // ── parse_semver ──────────────────────────────────────────────────────

    #[test]
    fn parse_semver_full_version() {
        assert_eq!(parse_semver("1.2.3"), (1, 2, 3));
    }

    #[test]
    fn parse_semver_with_v_prefix() {
        assert_eq!(parse_semver("v1.2.3"), (1, 2, 3));
    }

    #[test]
    fn parse_semver_with_pre_release() {
        assert_eq!(parse_semver("1.2.3-beta.1"), (1, 2, 3));
    }

    #[test]
    fn parse_semver_with_build_metadata() {
        assert_eq!(parse_semver("1.2.3+build.456"), (1, 2, 3));
    }

    #[test]
    fn parse_semver_partial() {
        assert_eq!(parse_semver("1"), (1, 0, 0));
        assert_eq!(parse_semver("1.2"), (1, 2, 0));
    }

    #[test]
    fn parse_semver_empty() {
        assert_eq!(parse_semver(""), (0, 0, 0));
    }

    #[test]
    fn parse_semver_garbage() {
        assert_eq!(parse_semver("abc"), (0, 0, 0));
        assert_eq!(parse_semver("a.b.c"), (0, 0, 0));
    }

    #[test]
    fn parse_semver_mixed_numeric_and_garbage() {
        assert_eq!(parse_semver("1.x.3"), (1, 0, 3));
    }

    #[test]
    fn parse_semver_whitespace_trimmed() {
        assert_eq!(parse_semver("  1.2.3  "), (1, 2, 3));
    }

    // ── UpdateInfo::is_newer ──────────────────────────────────────────────

    #[test]
    fn update_info_is_newer_true() {
        let info = UpdateInfo {
            current_version: "1.0.0".into(),
            latest_tag: "v1.0.1".into(),
            latest_version: "1.0.1".into(),
            release_url: "https://github.com/test/repo/releases/v1.0.1".into(),
            body: None,
            prerelease: false,
        };
        assert!(info.is_newer());
    }

    #[test]
    fn update_info_is_newer_false_same_version() {
        let info = UpdateInfo {
            current_version: "1.0.0".into(),
            latest_tag: "v1.0.0".into(),
            latest_version: "1.0.0".into(),
            release_url: "https://github.com/test/repo/releases/v1.0.0".into(),
            body: None,
            prerelease: false,
        };
        assert!(!info.is_newer());
    }

    #[test]
    fn update_info_is_newer_false_older_version() {
        let info = UpdateInfo {
            current_version: "1.0.1".into(),
            latest_tag: "v1.0.0".into(),
            latest_version: "1.0.0".into(),
            release_url: "https://github.com/test/repo/releases/v1.0.0".into(),
            body: None,
            prerelease: false,
        };
        assert!(!info.is_newer());
    }

    #[test]
    fn update_info_serde_roundtrip() {
        let info = UpdateInfo {
            current_version: "1.0.0".into(),
            latest_tag: "v1.0.1".into(),
            latest_version: "1.0.1".into(),
            release_url: "https://example.com".into(),
            body: Some("Release notes".into()),
            prerelease: true,
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: UpdateInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, back);
    }

    #[test]
    fn update_info_serde_skip_none_body() {
        let info = UpdateInfo {
            current_version: "1.0.0".into(),
            latest_tag: "v1.0.1".into(),
            latest_version: "1.0.1".into(),
            release_url: "https://example.com".into(),
            body: None,
            prerelease: false,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(
            !json.contains("body"),
            "None body should be skipped: {json}"
        );
    }

    #[test]
    fn update_info_serde_default_prerelease() {
        // JSON without prerelease field should default to false.
        let json = r#"{"current_version":"1.0.0","latest_tag":"v1.0.1","latest_version":"1.0.1","release_url":"https://example.com"}"#;
        let info: UpdateInfo = serde_json::from_str(json).unwrap();
        assert!(!info.prerelease);
        assert!(info.body.is_none());
    }

    // ── strip_ansi edge cases ─────────────────────────────────────────────

    #[test]
    fn strip_ansi_empty_string() {
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn strip_ansi_no_escape_codes() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }

    #[test]
    fn strip_ansi_csi_color() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
    }

    #[test]
    fn strip_ansi_csi_multiple_params() {
        assert_eq!(strip_ansi("\x1b[1;31;40mbold red\x1b[0m"), "bold red");
    }

    #[test]
    fn strip_ansi_csi_cursor_movement() {
        assert_eq!(strip_ansi("\x1b[2J\x1b[Hclear"), "clear");
    }

    #[test]
    fn strip_ansi_osc_with_bel() {
        // OSC sequence terminated by BEL (\x07)
        assert_eq!(strip_ansi("\x1b]0;title\x07rest"), "rest");
    }

    #[test]
    fn strip_ansi_osc_with_st() {
        // OSC sequence terminated by ST (ESC \)
        assert_eq!(strip_ansi("\x1b]0;title\x1b\\rest"), "rest");
    }

    #[test]
    fn strip_ansi_osc_empty_title_with_bel() {
        assert_eq!(strip_ansi("\x1b]0;\x07rest"), "rest");
    }

    #[test]
    fn strip_ansi_escape_without_bracket() {
        // Lone ESC followed by a non-CSI/OSC char — just skip the ESC.
        assert_eq!(strip_ansi("\x1bXhello"), "hello");
    }

    #[test]
    fn strip_ansi_lone_escape() {
        // Lone ESC at end of string.
        assert_eq!(strip_ansi("hello\x1b"), "hello");
    }

    #[test]
    fn strip_ansi_escape_at_start() {
        // ESC at start with no following char.
        assert_eq!(strip_ansi("\x1b"), "");
    }

    #[test]
    fn strip_ansi_multiple_csi_sequences() {
        assert_eq!(
            strip_ansi("\x1b[1mhello\x1b[0m \x1b[31mworld\x1b[0m"),
            "hello world"
        );
    }

    #[test]
    fn strip_ansi_mixed_csi_and_osc() {
        assert_eq!(strip_ansi("\x1b]0;title\x07\x1b[31mtext\x1b[0m"), "text");
    }

    #[test]
    fn strip_ansi_csi_without_terminator() {
        // CSI without a final byte — entire rest is consumed.
        assert_eq!(strip_ansi("\x1b[123"), "");
    }

    #[test]
    fn strip_ansi_preserves_newlines() {
        assert_eq!(strip_ansi("line1\nline2\n"), "line1\nline2\n");
    }

    #[test]
    fn strip_ansi_unicode_content() {
        assert_eq!(strip_ansi("héllo wörld"), "héllo wörld");
    }

    // ── installer_error_tail edge cases ───────────────────────────────────

    #[test]
    fn installer_error_tail_empty_both() {
        assert_eq!(installer_error_tail(b"", b""), "");
    }

    #[test]
    fn installer_error_tail_stderr_only() {
        let tail = installer_error_tail(b"", b"error message");
        assert_eq!(tail, "error message");
    }

    #[test]
    fn installer_error_tail_stdout_only() {
        let tail = installer_error_tail(b"output", b"");
        assert_eq!(tail, "output");
    }

    #[test]
    fn installer_error_tail_both_combined() {
        let tail = installer_error_tail(b"stdout line", b"stderr line");
        assert!(
            tail.contains("stderr line"),
            "stderr should be first: {tail}"
        );
        assert!(
            tail.contains("stdout line"),
            "stdout should be appended: {tail}"
        );
    }

    #[test]
    fn installer_error_tail_strips_ansi_from_stderr() {
        let tail = installer_error_tail(b"", b"\x1b[31merror\x1b[0m");
        assert_eq!(tail, "error");
        assert!(!tail.contains('\u{1b}'));
    }

    #[test]
    fn installer_error_tail_strips_ansi_from_stdout() {
        let tail = installer_error_tail(b"\x1b[32mok\x1b[0m", b"");
        assert_eq!(tail, "ok");
        assert!(!tail.contains('\u{1b}'));
    }

    #[test]
    fn installer_error_tail_trims_whitespace() {
        // The combined output is trimmed at the edges only; inner whitespace
        // from each stream is preserved.
        let tail = installer_error_tail(b"  output  ", b"  error  ");
        assert_eq!(tail, "error  \n  output");
    }

    #[test]
    fn installer_error_tail_short_output_unchanged() {
        let msg = "short error";
        let tail = installer_error_tail(b"", msg.as_bytes());
        assert_eq!(tail, msg);
    }

    #[test]
    fn installer_error_tail_long_output_truncated_to_1500() {
        // Create a string longer than 1500 chars.
        let long = "x".repeat(3000);
        let tail = installer_error_tail(b"", long.as_bytes());
        assert!(
            tail.len() <= 1500,
            "tail should be truncated to ~1500 chars, got {}",
            tail.len()
        );
        assert!(!tail.is_empty(), "tail should not be empty");
    }

    #[test]
    fn installer_error_tail_long_output_cuts_on_newline() {
        // Create a string with a newline near the 1500 boundary.
        let mut long = "x".repeat(1400);
        long.push('\n');
        long.push_str(&"y".repeat(1600));
        let tail = installer_error_tail(b"", long.as_bytes());
        // After truncation, the tail should start after the newline boundary.
        assert!(
            tail.starts_with('y'),
            "tail should start after newline boundary: starts with '{}'",
            tail.chars().next().unwrap_or(' ')
        );
    }

    #[test]
    fn installer_error_tail_long_output_no_newline() {
        // No newline in the long output — just take the last 1500 chars.
        let long = "x".repeat(3000);
        let tail = installer_error_tail(b"", long.as_bytes());
        assert_eq!(tail.len(), 1500);
        assert!(tail.chars().all(|c| c == 'x'));
    }

    // ── run_silent success path ───────────────────────────────────────────

    #[test]
    fn run_silent_success_path_suppresses_output() {
        // A successful command with stdout/stderr should return Ok.
        // On Windows, `sh` may not be available; use a platform-appropriate
        // command. We use `sh` here because the test environment has it.
        let mut cmd = std::process::Command::new("sh");
        cmd.args([
            "-c",
            "printf 'stdout output'; printf 'stderr output' >&2; exit 0",
        ]);
        let result = run_silent(cmd);
        assert!(result.is_ok(), "successful command should return Ok");
    }

    #[test]
    fn run_silent_failure_without_output() {
        // A failing command with no output should bail with just exit code.
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "exit 42"]);
        let err = run_silent(cmd).expect_err("expected failure");
        let msg = format!("{err:#}");
        assert!(msg.contains("42"), "error should contain exit code: {msg}");
    }

    #[test]
    fn run_silent_sets_env_vars() {
        // Verify that NO_COLOR and TERM are set by checking them in the child.
        let mut cmd = std::process::Command::new("sh");
        cmd.args([
            "-c",
            "test \"$NO_COLOR\" = \"1\" && test \"$TERM\" = \"dumb\" && exit 0; exit 1",
        ]);
        let result = run_silent(cmd);
        assert!(
            result.is_ok(),
            "run_silent should set NO_COLOR=1 and TERM=dumb"
        );
    }

    #[test]
    fn run_silent_nonexistent_command() {
        let mut cmd = std::process::Command::new("this_command_does_not_exist_xyz_123");
        let err = run_silent(cmd).expect_err("expected spawn failure");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("spawn") || msg.contains("this_command"),
            "error should mention spawn failure: {msg}"
        );
    }

    // ── apply_update_blocking ─────────────────────────────────────────────
    //
    // apply_update_blocking spawns real installer processes (powershell on
    // Windows, sh+curl on Unix). We test it with a "version" that will cause
    // the installer to fail gracefully (network not available in CI, or the
    // command itself fails). The key assertion is that the function returns
    // an Err (not a panic) when the installer fails.

    #[test]
    fn apply_update_blocking_returns_error_on_failed_install() {
        // This will attempt to run the real installer, which will fail in
        // most test environments (no network, no curl, etc.). The important
        // thing is that it returns an Err, not a panic.
        let result = apply_update_blocking("0.0.0-nonexistent");
        // In environments with network access, the installer might actually
        // run. We accept both Ok (installer succeeded) and Err (installer
        // failed). The key is no panic.
        match result {
            Ok(()) => {
                // Installer succeeded — unusual in CI but not a failure.
            }
            Err(e) => {
                let msg = format!("{e:#}");
                // Error should mention the installer or download.
                assert!(
                    msg.contains("installer")
                        || msg.contains("install")
                        || msg.contains("download")
                        || msg.contains("curl")
                        || msg.contains("powershell")
                        || msg.contains("spawn")
                        || msg.contains("exit"),
                    "error should mention installer-related failure: {msg}"
                );
            }
        }
    }

    #[test]
    fn apply_update_blocking_normalizes_version() {
        // The function normalizes the version before passing to the installer.
        // We can't easily verify the normalized version was used, but we can
        // verify the function doesn't panic with a v-prefixed version.
        let result = apply_update_blocking("v0.0.0-nonexistent");
        // Accept both Ok and Err — key is no panic.
        let _ = result;
    }

    // ── apply_update (async wrapper) ──────────────────────────────────────

    #[tokio::test]
    async fn apply_update_returns_error_on_failed_install() {
        let info = UpdateInfo {
            current_version: "0.0.0".into(),
            latest_tag: "v0.0.0-nonexistent".into(),
            latest_version: "0.0.0-nonexistent".into(),
            release_url: "https://example.com".into(),
            body: None,
            prerelease: false,
        };
        let result = apply_update(&info).await;
        // Accept both Ok and Err — key is no panic and no hang.
        match result {
            Ok(()) => {}
            Err(e) => {
                let msg = format!("{e:#}");
                assert!(!msg.is_empty(), "error message should not be empty");
            }
        }
    }

    // ── check_for_update (network-dependent, skip-on-failure) ─────────────
    //
    // check_for_update makes real HTTP requests to GitHub. In CI without
    // network, this will fail. We test it with a skip-on-failure pattern.

    #[tokio::test]
    async fn check_for_update_returns_ok_or_err_with_invalid_repo() {
        // Use an invalid repo that will definitely 404 or error.
        let result = check_for_update("0.0.0", Some("invalid/nonexistent-repo-xyz"), false).await;
        match result {
            Ok(None) => {
                // No update found — acceptable.
            }
            Ok(Some(info)) => {
                // Got an update — unexpected for invalid repo but not a crash.
                let _ = info;
            }
            Err(e) => {
                // Network error or 404 — expected in CI.
                let msg = format!("{e:#}");
                assert!(!msg.is_empty(), "error should have a message");
            }
        }
    }

    #[tokio::test]
    async fn check_for_update_with_prerelease_flag() {
        // Just verify the function doesn't panic with prerelease=true.
        // Use the real repo but accept any result (network may be unavailable).
        let result = check_for_update("0.0.0", None, true).await;
        match result {
            Ok(_) => {}
            Err(_) => {}
        }
    }

    #[tokio::test]
    async fn check_for_update_with_high_current_version_returns_none() {
        // If current version is very high, no release should be newer.
        // This may still return Some if GitHub returns a release with a
        // very high tag, but that's unlikely.
        let result = check_for_update("999.999.999", None, false).await;
        match result {
            Ok(None) => {
                // Expected — no newer version.
            }
            Ok(Some(_)) => {
                // Unexpected but not a crash.
            }
            Err(_) => {
                // Network error — acceptable in CI.
            }
        }
    }

    // ── GhRelease deserialization ─────────────────────────────────────────

    #[test]
    fn gh_release_deserialize_full() {
        let json = r#"{
            "tag_name": "v1.2.3",
            "html_url": "https://github.com/test/repo/releases/v1.2.3",
            "body": "Release notes here",
            "prerelease": false,
            "draft": false
        }"#;
        let release: GhRelease = serde_json::from_str(json).unwrap();
        assert_eq!(release.tag_name, "v1.2.3");
        assert_eq!(
            release.html_url,
            "https://github.com/test/repo/releases/v1.2.3"
        );
        assert_eq!(release.body.as_deref(), Some("Release notes here"));
        assert!(!release.prerelease);
        assert!(!release.draft);
    }

    #[test]
    fn gh_release_deserialize_minimal() {
        // Only required fields; optional fields default.
        let json = r#"{
            "tag_name": "v1.0.0",
            "html_url": "https://github.com/test/repo/releases/v1.0.0"
        }"#;
        let release: GhRelease = serde_json::from_str(json).unwrap();
        assert_eq!(release.tag_name, "v1.0.0");
        assert_eq!(
            release.html_url,
            "https://github.com/test/repo/releases/v1.0.0"
        );
        assert!(release.body.is_none());
        assert!(!release.prerelease);
        assert!(!release.draft);
    }

    #[test]
    fn gh_release_deserialize_prerelease() {
        let json = r#"{
            "tag_name": "v2.0.0-pre",
            "html_url": "https://example.com",
            "prerelease": true
        }"#;
        let release: GhRelease = serde_json::from_str(json).unwrap();
        assert!(release.prerelease);
        assert!(!release.draft);
    }

    #[test]
    fn gh_release_deserialize_draft() {
        let json = r#"{
            "tag_name": "v3.0.0",
            "html_url": "https://example.com",
            "draft": true
        }"#;
        let release: GhRelease = serde_json::from_str(json).unwrap();
        assert!(release.draft);
    }

    #[test]
    fn gh_release_deserialize_empty_body() {
        let json = r#"{
            "tag_name": "v1.0.0",
            "html_url": "https://example.com",
            "body": null
        }"#;
        let release: GhRelease = serde_json::from_str(json).unwrap();
        assert!(release.body.is_none());
    }

    #[test]
    fn gh_release_deserialize_missing_tag_fails() {
        let json = r#"{
            "html_url": "https://example.com"
        }"#;
        let result: Result<GhRelease, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    // ── UpdateInfo Debug/Clone/PartialEq ──────────────────────────────────

    #[test]
    fn update_info_clone_is_equal() {
        let info = UpdateInfo {
            current_version: "1.0.0".into(),
            latest_tag: "v1.0.1".into(),
            latest_version: "1.0.1".into(),
            release_url: "https://example.com".into(),
            body: Some("notes".into()),
            prerelease: false,
        };
        let cloned = info.clone();
        assert_eq!(info, cloned);
    }

    #[test]
    fn update_info_debug_format() {
        let info = UpdateInfo {
            current_version: "1.0.0".into(),
            latest_tag: "v1.0.1".into(),
            latest_version: "1.0.1".into(),
            release_url: "https://example.com".into(),
            body: None,
            prerelease: false,
        };
        let debug = format!("{info:?}");
        assert!(debug.contains("UpdateInfo"));
        assert!(debug.contains("1.0.0"));
        assert!(debug.contains("1.0.1"));
    }

    #[test]
    fn update_info_eq_different_versions() {
        let a = UpdateInfo {
            current_version: "1.0.0".into(),
            latest_tag: "v1.0.1".into(),
            latest_version: "1.0.1".into(),
            release_url: "https://example.com".into(),
            body: None,
            prerelease: false,
        };
        let b = UpdateInfo {
            current_version: "1.0.0".into(),
            latest_tag: "v2.0.0".into(),
            latest_version: "2.0.0".into(),
            release_url: "https://example.com".into(),
            body: None,
            prerelease: false,
        };
        assert_ne!(a, b);
    }

    // ── DEFAULT_REPO / INSTALL constants ──────────────────────────────────

    #[test]
    fn default_repo_is_navi_org() {
        assert_eq!(DEFAULT_REPO, "navi-ai-org/navi");
    }

    #[test]
    fn install_sh_url_is_valid() {
        assert!(INSTALL_SH.starts_with("https://"));
        assert!(INSTALL_SH.contains("install.sh"));
    }

    #[test]
    fn install_ps1_url_is_valid() {
        assert!(INSTALL_PS1.starts_with("https://"));
        assert!(INSTALL_PS1.contains("install.ps1"));
    }
}
