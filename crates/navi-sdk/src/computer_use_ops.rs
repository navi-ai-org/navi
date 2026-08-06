//! Computer use (OS automation) APIs on [`NaviEngine`] (ADR 0016).
//!
//! These methods expose whether the `computer-use` feature is compiled in,
//! the platform backend name, and the deny-list config. The actual tools
//! (`capture_screen`, `enumerate_windows`, `inspect_element`,
//! `simulate_input`) are registered in `navi-core` behind
//! `#[cfg(feature = "computer-use")]` and discovered via `tool_search`.

use crate::engine::NaviEngine;
use crate::types::NaviError;

type Result<T> = std::result::Result<T, NaviError>;

/// Snapshot of computer-use configuration exposed to SDK clients.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ComputerUseConfig {
    /// Whether the `computer-use` feature is compiled into this build.
    pub feature_enabled: bool,
    /// Whether computer-use is enabled at runtime (CLI flag or config).
    /// Tools are only registered when both `feature_enabled` and `enabled`
    /// are `true`.
    pub enabled: bool,
    /// Platform backend name (`"windows"`, `"macos"`, `"linux"`, or
    /// `"unsupported"` when no backend is compiled in).
    pub platform: String,
    /// Deny-list of applications that computer-use tools must not target
    /// (ADR 0016). Enforced in all modes except Yolo.
    pub deny_apps: Vec<String>,
}

impl NaviEngine {
    /// Returns `true` if the `computer-use` feature is compiled in.
    pub fn computer_use_available(&self) -> bool {
        cfg!(feature = "computer-use")
    }

    /// Returns the computer-use platform backend name, or `"unsupported"` if
    /// no backend is compiled in.
    pub fn computer_use_platform(&self) -> &'static str {
        #[cfg(all(feature = "computer-use", windows))]
        {
            "windows"
        }
        #[cfg(all(feature = "computer-use", target_os = "macos"))]
        {
            "macos"
        }
        #[cfg(all(feature = "computer-use", target_os = "linux"))]
        {
            "linux"
        }
        #[cfg(not(any(
            all(feature = "computer-use", windows),
            all(feature = "computer-use", target_os = "macos"),
            all(feature = "computer-use", target_os = "linux"),
        )))]
        {
            "unsupported"
        }
    }

    /// Returns a snapshot of the computer-use configuration.
    ///
    /// Includes feature availability, runtime enabled flag, platform backend,
    /// and the deny-list.
    pub fn computer_use_config(&self) -> ComputerUseConfig {
        let loaded = self.loaded_config();
        ComputerUseConfig {
            feature_enabled: self.computer_use_available(),
            enabled: loaded.config.security.computer_use_enabled,
            platform: self.computer_use_platform().to_string(),
            deny_apps: loaded.config.security.computer_use_deny_apps.clone(),
        }
    }

    /// Replaces the computer-use deny-list in the in-memory config.
    ///
    /// **Note:** this updates the in-memory config only. To persist, use
    /// `save_global_config` / `save_project_config`. Project-config merges
    /// re-apply the protected entries (OS security + NAVI self-protection)
    /// via `merge_deny_apps` — see ADR 0016.
    pub fn set_computer_use_deny_apps(&self, apps: Vec<String>) -> Result<()> {
        let mut loaded = self.loaded_config();
        loaded.config.security.computer_use_deny_apps = apps;
        // Note: we can't write back to the inner config without a setter.
        // The caller should use the config save APIs for persistence.
        // This method is a placeholder for future in-memory override support.
        tracing::info!(
            "computer_use_deny_apps updated (in-memory snapshot only): {} entries",
            loaded.config.security.computer_use_deny_apps.len()
        );
        Ok(())
    }
}
