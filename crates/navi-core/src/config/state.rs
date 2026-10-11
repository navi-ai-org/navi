//! Machine-owned NAVI state (`state.toml`).
//!
//! NAVI keeps hand-authored configuration (`config.toml`, project
//! `.navi/config.toml`) and machine-written state in separate files:
//!
//! | File | Owner | Written by NAVI |
//! |---|---|---|
//! | `config.toml` | the user | never |
//! | `state.toml` | NAVI | always (atomic) |
//! | `providers.d/**/*.toml` | the user | only explicit `navi provider …` commands |
//!
//! Everything NAVI mutates on its own (selected model, TUI preferences, MCP
//! enable flag, auto-update, skill-visibility additions) lives here, so a theme
//! toggle can no longer rewrite the provider list the user just hand-edited.
//!
//! Every field is optional: the file only carries the keys NAVI actually wrote,
//! which makes precedence unambiguous — `state.toml` never masks a hand-written
//! `config.toml` value it does not own.
//!
//! Precedence: `defaults < config.toml < state.toml` per layer (global, then
//! project), i.e. the most recent interactive choice wins over the declared
//! default. Delete `state.toml` (or run `navi config reset-state`) to fall back
//! to `config.toml`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::types::{ModelConfig, NaviConfig, TuiConfig};

/// File name of the machine-owned state file (sibling of `config.toml`).
pub const STATE_FILE_NAME: &str = "state.toml";

/// `[mcp]` slice owned by state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StateMcp {
    /// Runtime enable flag toggled from the UI/CLI. `None` means "not written".
    pub enabled: Option<bool>,
}

/// `[updates]` slice owned by state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StateUpdates {
    /// Auto-update preference toggled from the UI/CLI.
    pub auto_update: Option<bool>,
}

/// `[skills]` slice owned by state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StateSkills {
    /// Skill ids appended to a hand-written `[skills] active` allowlist.
    ///
    /// Only ever additive: NAVI must not be able to remove a skill the user
    /// pinned in `config.toml`. Ignored entirely while `active` is empty
    /// ("all discovered skills are active").
    pub extra_active: Vec<String>,
}

/// Machine-owned runtime state persisted to `state.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NaviState {
    /// Last model selected from the UI/API.
    pub model: Option<ModelConfig>,
    /// TUI preferences (theme, toggles, recents, …).
    pub tui: Option<TuiConfig>,
    /// MCP enable flag.
    pub mcp: StateMcp,
    /// Update preferences.
    pub updates: StateUpdates,
    /// Additive skill allowlist entries.
    pub skills: StateSkills,
}

impl NaviState {
    /// Returns `true` when the state file would carry no information.
    pub fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.tui.is_none()
            && self.mcp == StateMcp::default()
            && self.updates == StateUpdates::default()
            && self.skills == StateSkills::default()
    }
}

/// Returns the `state.toml` path that sits next to a `config.toml` path.
pub fn state_path_for_config(config_path: &Path) -> PathBuf {
    config_path.with_file_name(STATE_FILE_NAME)
}

/// Returns the `state.toml` path inside a config directory (e.g. `~/.config/navi`).
pub fn state_path_in_dir(dir: &Path) -> PathBuf {
    dir.join(STATE_FILE_NAME)
}

/// Loads `state.toml`. A missing file is `None`; a broken file is logged and
/// treated as `None` (state must never make NAVI unusable).
pub fn load_state(path: &Path) -> Option<NaviState> {
    if !path.exists() {
        return None;
    }
    match fs::read_to_string(path) {
        Ok(raw) => match toml::from_str::<NaviState>(&raw) {
            Ok(state) => Some(state),
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "ignoring unparsable state file (fix or delete it; NAVI will rewrite it)"
                );
                None
            }
        },
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "failed to read state file");
            None
        }
    }
}

/// Serializes the machine-owned slice of `config` into `state.toml`.
///
/// The file is read first so keys NAVI does not own from this call (for
/// example `[skills] extra_active`, written by `navi skill install`) survive.
pub fn save_state(path: &Path, config: &NaviConfig) -> Result<PathBuf> {
    let mut state = load_state(path).unwrap_or_default();
    state.model = Some(config.model.clone());
    state.tui = Some(config.tui.clone());
    state.mcp.enabled = Some(config.mcp.enabled);
    state.updates.auto_update = Some(config.updates.auto_update);
    write_state(path, &state)
}

/// Writes `state` to `path` atomically, creating parent directories.
pub fn write_state(path: &Path, state: &NaviState) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let content = toml::to_string_pretty(state).context("failed to serialize state")?;
    write_atomic(path, &content)?;
    Ok(path.to_path_buf())
}

/// Reads `state.toml`, applies `edit`, and writes it back atomically.
pub fn update_state<F>(path: &Path, edit: F) -> Result<PathBuf>
where
    F: FnOnce(&mut NaviState),
{
    let mut state = load_state(path).unwrap_or_default();
    edit(&mut state);
    write_state(path, &state)
}

/// Applies one state layer on top of `config` (scalar slices only).
///
/// Called after the matching `config.toml` layer, so state wins for the keys it
/// owns. `[skills] extra_active` is applied separately by
/// [`apply_extra_active`] because it depends on the fully merged allowlist.
pub fn apply_state(state: &NaviState, config: &mut NaviConfig) {
    if let Some(model) = &state.model
        && (!model.provider.trim().is_empty() || !model.name.trim().is_empty())
    {
        config.model = model.clone();
    }
    if let Some(tui) = &state.tui {
        config.tui = tui.clone();
    }
    if let Some(enabled) = state.mcp.enabled {
        config.mcp.enabled = enabled;
    }
    if let Some(auto_update) = state.updates.auto_update {
        config.updates.auto_update = auto_update;
    }
}

/// Merges additive `[skills] extra_active` entries from every state layer.
///
/// No-op while the effective `[skills] active` list is empty, because an empty
/// list means "every discovered skill is catalog-active".
pub fn apply_extra_active(states: &[&NaviState], config: &mut NaviConfig) {
    if config.skills.active.is_empty() {
        return;
    }
    for state in states {
        for id in &state.skills.extra_active {
            if !config.skills.active.iter().any(|existing| existing == id) {
                config.skills.active.push(id.clone());
            }
        }
    }
}

/// Writes `content` to `path` via a temporary file + rename.
pub(crate) fn write_atomic(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content).with_context(|| format!("failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| {
        format!(
            "failed to move {} into place at {}",
            tmp.display(),
            path.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::SkillsConfig;

    #[test]
    fn state_round_trips_the_machine_owned_slice() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = state_path_in_dir(dir.path());

        let mut config = NaviConfig::default();
        config.model.provider = "devin".to_string();
        config.model.name = "devin/swe-2".to_string();
        config.tui.theme = "lain".to_string();
        config.tui.recent_model_ids = vec!["devin:devin/swe-2".to_string()];
        config.mcp.enabled = true;
        config.updates.auto_update = true;

        save_state(&path, &config).expect("save state");
        let state = load_state(&path).expect("state file exists");

        assert_eq!(
            state.model.as_ref().map(|m| m.name.as_str()),
            Some("devin/swe-2")
        );
        assert_eq!(state.tui.as_ref().map(|t| t.theme.as_str()), Some("lain"));
        assert_eq!(state.mcp.enabled, Some(true));
        assert_eq!(state.updates.auto_update, Some(true));

        let mut restored = NaviConfig::default();
        apply_state(&state, &mut restored);
        assert_eq!(restored.model.name, "devin/swe-2");
        assert_eq!(restored.tui.theme, "lain");
        assert!(restored.mcp.enabled);
        assert!(restored.updates.auto_update);
    }

    #[test]
    fn save_state_preserves_keys_it_does_not_own() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = state_path_in_dir(dir.path());
        update_state(&path, |state| {
            state.skills.extra_active = vec!["socratic".to_string()];
        })
        .expect("seed state");

        save_state(&path, &NaviConfig::default()).expect("save state");
        let state = load_state(&path).expect("state file exists");
        assert_eq!(state.skills.extra_active, vec!["socratic".to_string()]);
    }

    #[test]
    fn state_never_writes_providers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = state_path_in_dir(dir.path());

        let mut config = NaviConfig::default();
        config.providers.push(crate::config::types::ProviderConfig {
            id: "devin".to_string(),
            label: "Devin".to_string(),
            api_key_env: "DEVIN_PROXY_KEY".to_string(),
            ..Default::default()
        });
        save_state(&path, &config).expect("save state");

        let raw = fs::read_to_string(&path).expect("read state");
        assert!(
            !raw.contains("devin"),
            "state.toml must not carry provider definitions:\n{raw}"
        );
        assert!(!raw.contains("[[providers]]"));
    }

    #[test]
    fn apply_state_is_additive_for_skill_allowlists() {
        let mut config = NaviConfig::default();
        config.skills = SkillsConfig {
            enabled: true,
            active: vec!["socratic".to_string()],
        };
        let state = NaviState {
            skills: StateSkills {
                extra_active: vec!["socratic".to_string(), "osint-recon".to_string()],
            },
            ..Default::default()
        };

        apply_extra_active(&[&state], &mut config);
        assert_eq!(
            config.skills.active,
            vec!["socratic".to_string(), "osint-recon".to_string()]
        );
    }

    #[test]
    fn extra_active_is_ignored_without_a_hand_written_allowlist() {
        let mut config = NaviConfig::default();
        assert!(config.skills.active.is_empty());
        let state = NaviState {
            skills: StateSkills {
                extra_active: vec!["osint-recon".to_string()],
            },
            ..Default::default()
        };

        apply_extra_active(&[&state], &mut config);
        assert!(config.skills.active.is_empty());
    }

    #[test]
    fn broken_state_file_is_ignored_instead_of_failing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = state_path_in_dir(dir.path());
        fs::write(&path, "this is not toml = = =").expect("write garbage");

        assert!(load_state(&path).is_none());
    }
}
