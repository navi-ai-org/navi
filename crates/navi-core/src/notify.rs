//! Cross-platform desktop notifications and URL opening.
//!
//! Used by the TUI, CLI, and SDK so host apps (desktop, browser via events)
//! can surface user-visible alerts consistently.

use std::process::Command;

/// Urgency / priority for a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationUrgency {
    Low,
    #[default]
    Normal,
    Critical,
}

impl NotificationUrgency {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::Critical => "critical",
        }
    }
}

/// A desktop / OS notification request.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NotifyRequest {
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub urgency: NotificationUrgency,
    /// Optional category for clients (e.g. `update`, `turn_complete`, `error`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

impl NotifyRequest {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            urgency: NotificationUrgency::Normal,
            category: None,
        }
    }

    pub fn with_urgency(mut self, urgency: NotificationUrgency) -> Self {
        self.urgency = urgency;
        self
    }

    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }
}

/// Show a desktop notification on the current OS.
///
/// Falls back to platform tools (`notify-send`, `osascript`, PowerShell toast)
/// so we stay dependency-light and work on headless CI (returns `Ok` if the
/// notifier is missing — notifications are best-effort).
pub fn notify_desktop(req: &NotifyRequest) -> anyhow::Result<()> {
    match std::env::consts::OS {
        "linux" => notify_linux(req),
        "macos" => notify_macos(req),
        "windows" => notify_windows(req),
        other => {
            tracing::debug!(os = other, "desktop notifications not supported on this OS");
            Ok(())
        }
    }
}

/// Open a URL in the user's default browser (or associated handler).
pub fn open_url(url: &str) -> anyhow::Result<()> {
    let url = url.trim();
    if url.is_empty() {
        anyhow::bail!("empty URL");
    }
    // Basic safety: only allow http(s) and a few known schemes.
    let ok = url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with("mailto:")
        || url.starts_with("file://");
    if !ok {
        anyhow::bail!("refusing to open non-http(s) URL: {url}");
    }
    match std::env::consts::OS {
        "linux" => {
            // xdg-open is the portable Linux path.
            let status = Command::new("xdg-open").arg(url).status();
            match status {
                Ok(s) if s.success() => Ok(()),
                Ok(s) => anyhow::bail!("xdg-open exited with {s}"),
                Err(err) => anyhow::bail!("xdg-open failed: {err}"),
            }
        }
        "macos" => {
            let status = Command::new("open").arg(url).status();
            match status {
                Ok(s) if s.success() => Ok(()),
                Ok(s) => anyhow::bail!("open exited with {s}"),
                Err(err) => anyhow::bail!("open failed: {err}"),
            }
        }
        "windows" => {
            // `start` is a cmd builtin; use cmd /C start "" url
            let status = Command::new("cmd").args(["/C", "start", "", url]).status();
            match status {
                Ok(s) if s.success() => Ok(()),
                Ok(s) => anyhow::bail!("cmd start exited with {s}"),
                Err(err) => anyhow::bail!("cmd start failed: {err}"),
            }
        }
        other => anyhow::bail!("open_url not supported on {other}"),
    }
}

fn notify_linux(req: &NotifyRequest) -> anyhow::Result<()> {
    // Prefer notify-send (libnotify). Missing tool is not fatal.
    if !command_exists("notify-send") {
        tracing::debug!("notify-send not found; skipping desktop notification");
        return Ok(());
    }
    let mut cmd = Command::new("notify-send");
    cmd.arg(format!("--urgency={}", req.urgency.as_str()));
    cmd.arg("--app-name=NAVI");
    if let Some(cat) = &req.category {
        cmd.arg(format!("--category={cat}"));
    }
    cmd.arg(&req.title);
    cmd.arg(&req.body);
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => {
            tracing::debug!(?s, "notify-send non-zero exit");
            Ok(())
        }
        Err(err) => {
            tracing::debug!(%err, "notify-send failed");
            Ok(())
        }
    }
}

fn notify_macos(req: &NotifyRequest) -> anyhow::Result<()> {
    // Notification Center via osascript.
    let title = escape_applescript(&req.title);
    let body = escape_applescript(&req.body);
    let script =
        format!("display notification \"{body}\" with title \"{title}\" subtitle \"NAVI\"");
    match Command::new("osascript").args(["-e", &script]).status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => {
            tracing::debug!(?s, "osascript notification non-zero");
            Ok(())
        }
        Err(err) => {
            tracing::debug!(%err, "osascript notification failed");
            Ok(())
        }
    }
}

fn notify_windows(req: &NotifyRequest) -> anyhow::Result<()> {
    // Toast via PowerShell WinRT (Windows 10+). Best-effort.
    let title = escape_ps(&req.title);
    let body = escape_ps(&req.body);
    let script = format!(
        r#"
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
$template = @"
<toast>
  <visual>
    <binding template="ToastGeneric">
      <text>{title}</text>
      <text>{body}</text>
    </binding>
  </visual>
</toast>
"@
$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$xml.LoadXml($template)
$toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier("NAVI").Show($toast)
"#
    );
    match Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
    {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => {
            tracing::debug!(?s, "powershell toast non-zero");
            Ok(())
        }
        Err(err) => {
            tracing::debug!(%err, "powershell toast failed");
            Ok(())
        }
    }
}

fn command_exists(name: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {name} >/dev/null 2>&1")])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn escape_ps(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_request_builder() {
        let n = NotifyRequest::new("t", "b")
            .with_urgency(NotificationUrgency::Critical)
            .with_category("update");
        assert_eq!(n.title, "t");
        assert_eq!(n.category.as_deref(), Some("update"));
        assert_eq!(n.urgency, NotificationUrgency::Critical);
    }

    #[test]
    fn open_url_rejects_shell_metachar() {
        assert!(open_url("javascript:alert(1)").is_err());
        assert!(open_url("").is_err());
    }
}
