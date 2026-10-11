//! MCP server configuration management on [`NaviEngine`].
//!
//! Session-scoped `list_mcp_servers` reflects live connections. The methods
//! here edit the durable configuration:
//!
//! * each server lives in its own file under `<config dir>/mcp.d/<id>.toml`,
//!   written only by these methods (or by hand — files are re-read on load),
//! * the `[mcp] enabled` flag is machine-owned state and goes to `state.toml`.
//!
//! `config.toml` is never rewritten: `[[mcp.servers]]` blocks keep working as
//! hand-authored entries, and a `mcp.d` file wins over an entry with the same
//! id.

use std::path::{Path, PathBuf};

use navi_core::config::mcp_dropin;
use navi_core::{McpConfig, McpServerConfig, state_path_for_config};
use serde::{Deserialize, Serialize};

use crate::engine::NaviEngine;
use crate::types::{NaviConfigSaveTarget, NaviError};

type Result<T> = std::result::Result<T, NaviError>;

/// Snapshot of configured MCP servers (from config, not live connections).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfigSnapshot {
    pub enabled: bool,
    pub servers: Vec<McpServerConfig>,
}

impl NaviEngine {
    /// List MCP servers as configured in TOML (not session connection state).
    pub fn list_mcp_config(&self) -> McpConfigSnapshot {
        let loaded = self.loaded_config();
        McpConfigSnapshot {
            enabled: loaded.config.mcp.enabled,
            servers: loaded.config.mcp.servers.clone(),
        }
    }

    /// Enable/disable MCP integration globally (persisted in `state.toml`).
    pub fn set_mcp_enabled(
        &self,
        enabled: bool,
        save_target: NaviConfigSaveTarget,
    ) -> Result<Option<PathBuf>> {
        let mut loaded = self.loaded_config();
        loaded.config.mcp.enabled = enabled;
        let saved = self.persist_mcp_state(&loaded, save_target)?;
        self.replace_loaded_config(loaded);
        Ok(saved)
    }

    /// Insert or replace an MCP server entry by `id` (written to `mcp.d/`).
    pub fn upsert_mcp_server(
        &self,
        server: McpServerConfig,
        save_target: NaviConfigSaveTarget,
    ) -> Result<Option<PathBuf>> {
        if server.id.trim().is_empty() {
            return Err(NaviError::Config("MCP server id cannot be empty".into()));
        }
        let mut loaded = self.loaded_config();
        if let Some(existing) = loaded
            .config
            .mcp
            .servers
            .iter_mut()
            .find(|s| s.id == server.id)
        {
            *existing = server.clone();
        } else {
            loaded.config.mcp.servers.push(server.clone());
        }
        // Adding a server implies MCP is on.
        loaded.config.mcp.enabled = true;

        let saved = match save_target {
            NaviConfigSaveTarget::None => None,
            _ => {
                let dir = self.mcp_dropin_dir(&loaded)?;
                Some(mcp_dropin::write_server_file(&dir, &server).map_err(NaviError::from)?)
            }
        };
        // Keep the enable flag in state so a later load keeps MCP on.
        if saved.is_some() {
            let _ = self.persist_mcp_state(&loaded, save_target);
        }
        self.replace_loaded_config(loaded);
        Ok(saved)
    }

    /// Remove an MCP server by id. Returns whether a server was removed + the
    /// deleted file (if any).
    ///
    /// Servers declared by hand in `config.toml` cannot be removed from here:
    /// the error tells the caller to edit the file instead of silently
    /// reappearing on the next start.
    pub fn remove_mcp_server(
        &self,
        server_id: &str,
        save_target: NaviConfigSaveTarget,
    ) -> Result<(bool, Option<PathBuf>)> {
        let mut loaded = self.loaded_config();
        let dir = self.mcp_dropin_dir(&loaded)?;
        let removed_file = if matches!(save_target, NaviConfigSaveTarget::None) {
            None
        } else {
            mcp_dropin::remove_server_file(&dir, server_id).map_err(NaviError::from)?
        };

        let before = loaded.config.mcp.servers.len();
        let was_configured = loaded
            .config
            .mcp
            .servers
            .iter()
            .any(|s| s.id == server_id);
        if removed_file.is_none() && was_configured {
            return Err(NaviError::Config(format!(
                "MCP server `{server_id}` is declared in config.toml; remove its [[mcp.servers]] \
                 block there (NAVI does not rewrite hand-authored config)"
            )));
        }

        loaded.config.mcp.servers.retain(|s| s.id != server_id);
        let removed = loaded.config.mcp.servers.len() != before;
        if removed {
            self.replace_loaded_config(loaded);
        }
        Ok((removed, removed_file))
    }

    /// Replace the entire in-memory MCP config block.
    ///
    /// Servers are written to `mcp.d/` (one file each); files for servers that
    /// were dropped from the new block are deleted. Files that never
    /// corresponded to a configured server are left alone.
    pub fn set_mcp_config(
        &self,
        mcp: McpConfig,
        save_target: NaviConfigSaveTarget,
    ) -> Result<Option<PathBuf>> {
        let mut loaded = self.loaded_config();
        let previous: Vec<String> = loaded
            .config
            .mcp
            .servers
            .iter()
            .map(|server| server.id.clone())
            .collect();
        let dir = self.mcp_dropin_dir(&loaded)?;
        let mut last_written = None;
        if !matches!(save_target, NaviConfigSaveTarget::None) {
            for server in &mcp.servers {
                last_written =
                    Some(mcp_dropin::write_server_file(&dir, server).map_err(NaviError::from)?);
            }
            for id in previous {
                if !mcp.servers.iter().any(|server| server.id == id) {
                    let _ = mcp_dropin::remove_server_file(&dir, &id);
                }
            }
        }
        loaded.config.mcp = mcp;
        let saved = self
            .persist_mcp_state(&loaded, save_target)?
            .or(last_written);
        self.replace_loaded_config(loaded);
        Ok(saved)
    }

    /// Directory that holds this engine's `mcp.d/` files.
    fn mcp_dropin_dir(&self, loaded_config: &navi_core::LoadedConfig) -> Result<PathBuf> {
        let global_dir = loaded_config
            .global_config_path
            .as_ref()
            .and_then(|path| path.parent())
            .ok_or_else(|| NaviError::Config("global config path is unavailable".into()))?;
        Ok(mcp_dropin::mcp_dir(global_dir))
    }

    /// Persists the machine-owned slice (including `[mcp] enabled`) to `state.toml`.
    fn persist_mcp_state(
        &self,
        loaded_config: &navi_core::LoadedConfig,
        target: NaviConfigSaveTarget,
    ) -> Result<Option<PathBuf>> {
        match target {
            NaviConfigSaveTarget::None => Ok(None),
            NaviConfigSaveTarget::Project => {
                let path = navi_core::save_project_state(&self.inner.project_dir, &loaded_config.config)
                    .map_err(NaviError::from)?;
                Ok(Some(path))
            }
            // MCP is security-sensitive and global-only: project config may not
            // introduce executable surfaces, so the flag lives globally too.
            NaviConfigSaveTarget::Global | NaviConfigSaveTarget::Auto => {
                let global_path = loaded_config
                    .global_config_path
                    .as_ref()
                    .ok_or_else(|| NaviError::Config("global config path is unavailable".into()))?;
                let path = navi_core::save_state_config(global_path, &loaded_config.config)
                    .map_err(NaviError::from)?;
                Ok(Some(path))
            }
        }
    }
}

/// Path of the state file that backs the MCP enable flag (diagnostics/tests).
pub fn mcp_state_path(global_config_path: &Path) -> PathBuf {
    state_path_for_config(global_config_path)
}
