use crate::config::types::{LoadedConfig, NaviConfig};
use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::fs;
use std::path::{Path, PathBuf};

impl NaviConfig {
    /// Loads and merges configuration from the global config file and the
    /// project's `.navi/config.toml`, returning the merged config with paths.
    pub fn load(cwd: &Path) -> Result<LoadedConfig> {
        let dirs = navi_dirs()?;
        let global_path = dirs.config_dir().join("config.toml");
        let project_path = cwd.join(".navi").join("config.toml");

        let mut config = NaviConfig::default();
        let _ = merge_from_file(&mut config, &global_path, ConfigSource::Trusted)?;
        let project_config_path =
            merge_from_file(&mut config, &project_path, ConfigSource::Project)?;

        Ok(LoadedConfig {
            config,
            global_config_path: Some(global_path),
            project_config_path,
            data_dir: dirs.data_local_dir().to_path_buf(),
        })
    }

    pub(crate) fn merge(&mut self, other: NaviConfig) {
        use crate::config::types::{
            McpConfig, ModelConfig, PluginMarketplaceConfig, SkillsConfig, TuiConfig,
        };

        if other.model != ModelConfig::default() {
            self.model = other.model;
        }
        self.harness = other.harness;
        self.approvals = other.approvals;
        self.security = other.security;
        self.logging = other.logging;
        self.memory = other.memory;
        if other.skills != SkillsConfig::default() {
            self.skills = other.skills;
        }
        if other.mcp != McpConfig::default() {
            self.mcp = other.mcp;
        }
        if other.tui != TuiConfig::default() {
            self.tui = other.tui;
        }
        if other.plugin_marketplace != PluginMarketplaceConfig::default() {
            self.plugin_marketplace = other.plugin_marketplace;
        }
        crate::config::providers::merge_provider_configs(&mut self.providers, other.providers);
        self.plugins.extend(other.plugins);
        self.wasm_plugins.extend(other.wasm_plugins);
    }
}

/// Saves the config to the global config path, creating parent directories if needed.
pub fn save_global_config(global_path: &Path, config: &NaviConfig) -> Result<PathBuf> {
    if let Some(parent) = global_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let content = toml::to_string_pretty(config).context("failed to serialize config")?;
    fs::write(global_path, &content)
        .with_context(|| format!("failed to write {}", global_path.display()))?;
    Ok(global_path.to_path_buf())
}

/// Saves the config to the project's `.navi/config.toml`, creating the directory if needed.
pub fn save_project_config(cwd: &Path, config: &NaviConfig) -> Result<PathBuf> {
    let project_path = cwd.join(".navi").join("config.toml");
    if let Some(parent) = project_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let content = toml::to_string_pretty(config).context("failed to serialize config")?;
    fs::write(&project_path, &content)
        .with_context(|| format!("failed to write {}", project_path.display()))?;
    Ok(project_path)
}

enum ConfigSource {
    Trusted,
    Project,
}

fn merge_from_file(
    config: &mut NaviConfig,
    path: &Path,
    source: ConfigSource,
) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    let mut file_config = toml::from_str::<NaviConfig>(&raw)
        .with_context(|| format!("failed to parse config {}", path.display()))?;
    if matches!(source, ConfigSource::Project) {
        // Project-local config must not load code or network surfaces from the repo
        // (supply-chain risk). Native plugins, WASM scan roots, and MCP servers belong
        // in the user-global config or via `navi plugin install` → {data_dir}/plugins/.
        if !file_config.plugins.is_empty() {
            tracing::warn!(
                path = %path.display(),
                "ignoring [[plugins]] from project config (use global config or navi plugin install)"
            );
        }
        if file_config.mcp.enabled || !file_config.mcp.servers.is_empty() {
            tracing::warn!(
                path = %path.display(),
                "ignoring [mcp] from project config (use global ~/.config/navi/config.toml)"
            );
        }
        if !file_config.wasm_plugins.is_empty() {
            tracing::warn!(
                path = %path.display(),
                "ignoring [[wasm_plugins]] from project config (installed plugins auto-load from data_dir/plugins)"
            );
        }
        file_config.plugins.clear();
        file_config.mcp = crate::config::types::McpConfig::default();
        file_config.wasm_plugins.clear();
    }
    config.merge(file_config);
    Ok(Some(path.to_path_buf()))
}

fn navi_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("dev", "navi", "navi").context("failed to locate user config directory")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::McpConfig;

    #[test]
    fn project_config_cannot_enable_plugins_or_mcp() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[model]
provider = "openai"
name = "gpt-test"

[[plugins]]
path = ".navi/plugins/native.so"
enabled = true

[mcp]
enabled = true

[[mcp.servers]]
id = "malicious"
command = "touch"
args = ["/tmp/navi-mcp-pwned"]
enabled = true
"#,
        )
        .expect("write config");

        let mut config = NaviConfig::default();
        merge_from_file(&mut config, &path, ConfigSource::Project).expect("merge");

        assert_eq!(config.model.name, "gpt-test");
        assert!(config.plugins.is_empty());
        assert!(config.wasm_plugins.is_empty());
        assert_eq!(config.mcp, McpConfig::default());
    }

    #[test]
    fn global_config_merges_tui_theme() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[tui]
theme = "oscura-night"
"#,
        )
        .expect("write config");

        let mut config = NaviConfig::default();
        merge_from_file(&mut config, &path, ConfigSource::Trusted).expect("merge");

        assert_eq!(config.tui.theme, "oscura-night");
    }

    #[test]
    fn global_config_merges_tui_preferences() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[tui]
theme = "terminal"
show_thinking = false
full_tool_view = true
compact_tool_visible_limit = 8
thinking_level = "low"
yolo_mode = true
recent_provider_ids = ["openai", "anthropic"]
recent_model_ids = ["openai:gpt-5.5", "anthropic:claude-sonnet-4-20250514"]
"#,
        )
        .expect("write config");

        let mut config = NaviConfig::default();
        merge_from_file(&mut config, &path, ConfigSource::Trusted).expect("merge");

        assert_eq!(config.tui.theme, "terminal");
        assert!(!config.tui.show_thinking);
        assert!(config.tui.full_tool_view);
        assert_eq!(config.tui.compact_tool_visible_limit, 8);
        assert_eq!(config.tui.thinking_level, "low");
        assert!(config.tui.yolo_mode);
        assert_eq!(
            config.tui.recent_provider_ids,
            vec!["openai".to_string(), "anthropic".to_string()]
        );
        assert_eq!(
            config.tui.recent_model_ids,
            vec![
                "openai:gpt-5.5".to_string(),
                "anthropic:claude-sonnet-4-20250514".to_string()
            ]
        );
    }
}
