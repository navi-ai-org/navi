//! Desktop notifications, URL open, and self-update APIs on [`NaviEngine`].
//!
//! Browser / remote hosts should listen for [`AgentEvent::NotificationRequested`]
//! and [`AgentEvent::UpdateAvailable`] (via the event stream) and map them to
//! the Web Notifications API or in-app toasts. Desktop hosts also get OS toasts
//! when [`Self::notify`] is called with `desktop: true`.

use navi_core::{
    NotifyRequest, UpdateInfo, apply_update, check_for_update, current_version, notify_desktop,
    open_url,
};

use crate::engine::NaviEngine;
use crate::types::NaviError;

type Result<T> = std::result::Result<T, NaviError>;

impl NaviEngine {
    /// Show a notification. When `desktop` is true, also attempt an OS toast
    /// (Windows / macOS / Linux). Always returns the request so browser hosts
    /// can surface Web Notifications from the same payload.
    ///
    /// Prefer emitting via session events when a session is available so
    /// remote clients receive [`navi_core::AgentEvent::NotificationRequested`].
    pub fn notify(&self, request: NotifyRequest, desktop: bool) -> Result<NotifyRequest> {
        if desktop && let Err(err) = notify_desktop(&request) {
            tracing::debug!(%err, "desktop notification failed (best-effort)");
        }
        Ok(request)
    }

    /// Convenience: build and deliver a simple notification.
    pub fn notify_simple(
        &self,
        title: impl Into<String>,
        body: impl Into<String>,
        desktop: bool,
    ) -> Result<NotifyRequest> {
        self.notify(NotifyRequest::new(title, body), desktop)
    }

    /// Open a URL in the user's default browser / handler.
    pub fn open_url(&self, url: &str) -> Result<()> {
        open_url(url).map_err(NaviError::from)
    }

    /// Running binary version (workspace package version at build time).
    pub fn app_version(&self) -> String {
        current_version().to_string()
    }

    /// Check GitHub Releases for a newer NAVI version.
    ///
    /// Returns `Ok(None)` when already up to date.
    pub async fn check_for_update(&self) -> Result<Option<UpdateInfo>> {
        let cfg = self.loaded_config().config.updates;
        let current = current_version();
        check_for_update(current, cfg.repo.as_deref(), cfg.include_prerelease)
            .await
            .map_err(NaviError::from)
    }

    /// Check with explicit overrides (used by CLI / tests).
    pub async fn check_for_update_with(
        &self,
        current: &str,
        repo: Option<&str>,
        include_prerelease: bool,
    ) -> Result<Option<UpdateInfo>> {
        check_for_update(current, repo, include_prerelease)
            .await
            .map_err(NaviError::from)
    }

    /// Apply an update by re-running the official installer for `info.latest_version`.
    ///
    /// On success the new binary is on disk; the running process should exit.
    pub async fn apply_update(&self, info: &UpdateInfo) -> Result<()> {
        apply_update(info).await.map_err(NaviError::from)
    }

    /// Whether automatic install is enabled in config.
    pub fn auto_update_enabled(&self) -> bool {
        self.loaded_config().config.updates.auto_update
    }

    /// Set auto-update preference and persist to the global config when possible.
    pub fn set_auto_update(&self, enabled: bool) -> Result<()> {
        let mut loaded = self.loaded_config();
        loaded.config.updates.auto_update = enabled;
        if let Some(path) = loaded.global_config_path.as_ref() {
            navi_core::save_global_config(path, &loaded.config).map_err(NaviError::from)?;
        }
        self.replace_loaded_config(loaded);
        Ok(())
    }
}
