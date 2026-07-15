use std::time::Instant;

use navi_core::{NotificationUrgency, NotifyRequest, notify_desktop};

use crate::app::TuiApp;
use crate::state::Notification;
use crate::theme::NOTIFICATION_TTL;

pub(crate) fn show_notification(
    app: &mut TuiApp,
    title: impl Into<String>,
    message: impl Into<String>,
) {
    app.set_notification(Notification {
        title: title.into(),
        message: message.into(),
        created_at: Instant::now(),
        ttl: NOTIFICATION_TTL,
    });
}

/// Truncate a single-line body for OS toasts (keep first ~120 chars).
fn toast_body(text: &str) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX: usize = 120;
    if collapsed.chars().count() <= MAX {
        return collapsed;
    }
    let mut out: String = collapsed.chars().take(MAX.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Best-effort OS toast when the job finished and the user is not looking at NAVI.
///
/// No-ops when:
/// - desktop notifications are disabled in config, or
/// - the terminal still has focus (user already sees the result).
pub(crate) fn notify_job_done_if_unfocused(
    app: &TuiApp,
    title: impl Into<String>,
    body: impl Into<String>,
) {
    if !app.loaded_config.config.tui.desktop_notifications {
        return;
    }
    if app.terminal_focused {
        return;
    }
    let title = title.into();
    let body = toast_body(&body.into());
    let request = NotifyRequest::new(title, body)
        .with_urgency(NotificationUrgency::Normal)
        .with_category("turn_complete");
    if let Err(err) = notify_desktop(&request) {
        tracing::debug!(%err, "desktop job-done notification failed (best-effort)");
    }
}

pub(crate) fn push_diagnostic(app: &mut TuiApp, message: impl Into<String>) {
    app.push_diagnostic(message);
}

pub(crate) fn expire_notification(app: &mut TuiApp) -> bool {
    let expired = app
        .notification()
        .is_some_and(|notification| notification.created_at.elapsed() >= notification.ttl);
    if expired {
        app.clear_notification();
    }
    expired
}

pub(crate) fn visible_notification(app: &TuiApp) -> Option<&Notification> {
    app.notification()
        .filter(|notification| notification.created_at.elapsed() < notification.ttl)
}
