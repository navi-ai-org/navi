use std::time::Instant;

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
