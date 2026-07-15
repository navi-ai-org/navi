//! MCP server configuration management on [`NaviEngine`].
//!
//! Session-scoped `list_mcp_servers` reflects live connections. The methods
//! here edit the durable `[mcp]` config (global/project TOML).

use std::path::PathBuf;

use navi_core::{McpConfig, McpServerConfig, save_global_config, save_project_config};
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

    /// Enable/disable MCP integration globally in config.
    pub fn set_mcp_enabled(
        &self,
        enabled: bool,
        save_target: NaviConfigSaveTarget,
    ) -> Result<Option<PathBuf>> {
        let mut loaded = self.loaded_config();
        loaded.config.mcp.enabled = enabled;
        let saved = self.persist_mcp_config(&loaded, save_target)?;
        self.replace_loaded_config(loaded);
        Ok(saved)
    }

    /// Insert or replace an MCP server entry by `id`.
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
            *existing = server;
        } else {
            loaded.config.mcp.servers.push(server);
        }
        // Ensure MCP is on when adding a server (unless caller disabled it later).
        if !loaded.config.mcp.enabled {
            loaded.config.mcp.enabled = true;
        }
        let saved = self.persist_mcp_config(&loaded, save_target)?;
        self.replace_loaded_config(loaded);
        Ok(saved)
    }

    /// Remove an MCP server by id. Returns whether a server was removed + save path.
    pub fn remove_mcp_server(
        &self,
        server_id: &str,
        save_target: NaviConfigSaveTarget,
    ) -> Result<(bool, Option<PathBuf>)> {
        let mut loaded = self.loaded_config();
        let before = loaded.config.mcp.servers.len();
        loaded.config.mcp.servers.retain(|s| s.id != server_id);
        let removed = loaded.config.mcp.servers.len() != before;
        let saved = if removed {
            self.persist_mcp_config(&loaded, save_target)?
        } else {
            None
        };
        if removed {
            self.replace_loaded_config(loaded);
        }
        Ok((removed, saved))
    }

    /// Replace the entire MCP config block.
    pub fn set_mcp_config(
        &self,
        mcp: McpConfig,
        save_target: NaviConfigSaveTarget,
    ) -> Result<Option<PathBuf>> {
        let mut loaded = self.loaded_config();
        loaded.config.mcp = mcp;
        let saved = self.persist_mcp_config(&loaded, save_target)?;
        self.replace_loaded_config(loaded);
        Ok(saved)
    }

    fn persist_mcp_config(
        &self,
        loaded_config: &navi_core::LoadedConfig,
        target: NaviConfigSaveTarget,
    ) -> Result<Option<PathBuf>> {
        match target {
            NaviConfigSaveTarget::None => Ok(None),
            NaviConfigSaveTarget::Project => {
                let path = save_project_config(&self.inner.project_dir, &loaded_config.config)
                    .map_err(NaviError::from)?;
                Ok(Some(path))
            }
            NaviConfigSaveTarget::Global => {
                let global_path = loaded_config
                    .global_config_path
                    .as_ref()
                    .ok_or_else(|| NaviError::Config("global config path is unavailable".into()))?;
                let path = save_global_config(global_path, &loaded_config.config)
                    .map_err(NaviError::from)?;
                Ok(Some(path))
            }
            NaviConfigSaveTarget::Auto => {
                // MCP is security-sensitive: prefer global config (project MCP is ignored on load).
                if let Some(global_path) = loaded_config.global_config_path.as_ref() {
                    let path = save_global_config(global_path, &loaded_config.config)
                        .map_err(NaviError::from)?;
                    Ok(Some(path))
                } else {
                    let path = save_project_config(&self.inner.project_dir, &loaded_config.config)
                        .map_err(NaviError::from)?;
                    Ok(Some(path))
                }
            }
        }
    }
}
