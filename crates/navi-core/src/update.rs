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
    let mut parts = v.split(|c| c == '.' || c == '-' || c == '+');
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
pub async fn apply_update(info: &UpdateInfo) -> Result<()> {
    let version = info.latest_version.clone();
    tokio::task::spawn_blocking(move || apply_update_blocking(&version))
        .await
        .context("update task join")??;
    Ok(())
}

fn apply_update_blocking(version: &str) -> Result<()> {
    let version = normalize_version(version);
    match std::env::consts::OS {
        "windows" => {
            // Download install.ps1 and run with -Version
            let status = std::process::Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    &format!(
                        "irm {INSTALL_PS1} | iex; if (Get-Command Install-Navi -ErrorAction SilentlyContinue) {{ Install-Navi -Version {version} }} else {{ & ([scriptblock]::Create((irm {INSTALL_PS1}))) -Version {version} }}"
                    ),
                ])
                .status()
                .context("spawn powershell installer")?;
            if !status.success() {
                // Fallback: curl-style via iwr to temp
                let tmp = std::env::temp_dir().join("navi-install.ps1");
                let script = std::process::Command::new("powershell")
                    .args([
                        "-NoProfile",
                        "-Command",
                        &format!(
                            "Invoke-WebRequest -Uri '{INSTALL_PS1}' -OutFile '{}'",
                            tmp.display()
                        ),
                    ])
                    .status()
                    .context("download install.ps1")?;
                if !script.success() {
                    anyhow::bail!("failed to download install.ps1");
                }
                let status = std::process::Command::new("powershell")
                    .args([
                        "-NoProfile",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-File",
                        tmp.to_str().unwrap_or("navi-install.ps1"),
                        "-Version",
                        &version,
                    ])
                    .status()
                    .context("run install.ps1")?;
                if !status.success() {
                    anyhow::bail!("install.ps1 exited with {status}");
                }
            }
            Ok(())
        }
        _ => {
            // curl | sh with pinned version (checksum verified by install.sh)
            let status = std::process::Command::new("sh")
                .args([
                    "-c",
                    &format!("curl -fsSL {INSTALL_SH} | sh -s -- --version {version}"),
                ])
                .status()
                .context("spawn install.sh")?;
            if !status.success() {
                anyhow::bail!("install.sh exited with {status}");
            }
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
}
