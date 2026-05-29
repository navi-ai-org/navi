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
        let _ = merge_from_file(&mut config, &global_path)?;
        let project_config_path = merge_from_file(&mut config, &project_path)?;

        Ok(LoadedConfig {
            config,
            global_config_path: Some(global_path),
            project_config_path,
            data_dir: dirs.data_local_dir().to_path_buf(),
        })
    }

    pub(crate) fn merge(&mut self, other: NaviConfig) {
        use crate::config::types::{McpConfig, ModelConfig, SkillsConfig};

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
        crate::config::providers::merge_provider_configs(&mut self.providers, other.providers);
        self.plugins.extend(other.plugins);
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

fn merge_from_file(config: &mut NaviConfig, path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    let file_config = toml::from_str::<NaviConfig>(&raw)
        .with_context(|| format!("failed to parse config {}", path.display()))?;
    config.merge(file_config);
    Ok(Some(path.to_path_buf()))
}

fn navi_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("dev", "navi", "navi").context("failed to locate user config directory")
}
