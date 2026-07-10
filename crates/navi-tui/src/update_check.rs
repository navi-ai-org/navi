//! Background update check + install helpers for the TUI.

use navi_core::{UpdateInfo, apply_update, check_for_update, current_version, notify_desktop};
use navi_core::{NotificationUrgency, NotifyRequest};

use crate::app::TuiApp;
use crate::dispatch::AsyncEvent;
use crate::notifications::show_notification;
use crate::state::ModalKind;

/// Spawn a non-blocking GitHub Releases check. Result arrives as
/// [`AsyncEvent::UpdateChecked`].
///
/// No-ops when there is no current tokio runtime (unit tests constructing
/// `TuiApp` outside `#[tokio::test]`).
pub(crate) fn spawn_update_check(app: &TuiApp) {
    let cfg = app.loaded_config.config.updates.clone();
    if !cfg.check_enabled {
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let tx = app.async_sender();
    let current = current_version().to_string();
    let repo = cfg.repo.clone();
    let include_prerelease = cfg.include_prerelease;
    handle.spawn(async move {
        let result = check_for_update(&current, repo.as_deref(), include_prerelease)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(AsyncEvent::UpdateChecked { result });
    });
}

/// Install the given update (or the pending `app.available_update`).
pub(crate) fn spawn_apply_update(app: &mut TuiApp, info: Option<UpdateInfo>) {
    let Some(info) = info.or_else(|| app.available_update.clone()) else {
        show_notification(app, "Update", "No update available to install.");
        return;
    };
    if app.update_installing {
        show_notification(app, "Update", "An update is already installing…");
        return;
    }
    app.update_installing = true;
    show_notification(
        app,
        "Update",
        format!("Installing NAVI {}…", info.latest_version),
    );
    let tx = app.async_sender();
    tokio::spawn(async move {
        let result = apply_update(&info).await.map_err(|e| e.to_string());
        let _ = tx.send(AsyncEvent::UpdateApplied {
            version: info.latest_version,
            result,
        });
    });
}

pub(crate) fn handle_update_checked(
    app: &mut TuiApp,
    result: Result<Option<UpdateInfo>, String>,
    user_initiated: bool,
) {
    match result {
        Ok(Some(info)) => {
            app.available_update = Some(info.clone());
            let msg = format!(
                "NAVI {} available (you have {}). Commands → Install Update",
                info.latest_version, info.current_version
            );
            show_notification(app, "Update available", msg.clone());
            let _ = notify_desktop(
                &NotifyRequest::new("NAVI update available", &msg)
                    .with_urgency(NotificationUrgency::Normal)
                    .with_category("update"),
            );
            if app.loaded_config.config.updates.auto_update {
                spawn_apply_update(app, Some(info));
            } else if user_initiated {
                crate::keybindings::replace_modal(app, ModalKind::UpdateAvailable);
            }
        }
        Ok(None) => {
            app.available_update = None;
            if user_initiated {
                show_notification(
                    app,
                    "Up to date",
                    format!("You're on the latest NAVI ({}).", current_version()),
                );
            }
        }
        Err(err) => {
            if user_initiated {
                show_notification(app, "Update check failed", err);
            } else {
                tracing::debug!(%err, "background update check failed");
            }
        }
    }
}

pub(crate) fn handle_update_applied(
    app: &mut TuiApp,
    version: String,
    result: Result<(), String>,
) {
    app.update_installing = false;
    match result {
        Ok(()) => {
            app.available_update = None;
            show_notification(
                app,
                "Update installed",
                format!(
                    "NAVI {version} is on disk. Restart NAVI to use the new version."
                ),
            );
            let _ = notify_desktop(&NotifyRequest::new(
                "NAVI updated",
                format!("Installed {version}. Restart to apply."),
            ));
        }
        Err(err) => {
            show_notification(app, "Update failed", err);
        }
    }
}
