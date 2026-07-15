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

        // Apply environment overrides for memory configuration
        if let Ok(val) = std::env::var("NAVI_MEMORY_ENABLED") {
            if let Ok(b) = val.parse::<bool>() {
                config.memory.enabled = b;
            }
        }
        if let Ok(val) = std::env::var("NAVI_MEMORY_REBUILD_THRESHOLD") {
            if let Ok(f) = val.parse::<f64>() {
                config.memory.rebuild_threshold = f;
            }
        }
        if let Ok(val) = std::env::var("NAVI_MEMORY_CHECKPOINT_THRESHOLDS") {
            let parsed: Vec<f64> = val
                .split(',')
                .filter_map(|s| s.trim().parse::<f64>().ok())
                .collect();
            if !parsed.is_empty() {
                config.memory.checkpoint_thresholds = parsed;
            }
        }

        Ok(LoadedConfig {
            config,
            global_config_path: Some(global_path),
            project_config_path,
            data_dir: dirs.data_local_dir().to_path_buf(),
        })
    }

    pub(crate) fn merge(&mut self, other: NaviConfig) {
        use crate::config::types::{
            AttachmentModelsConfig, BackgroundModelsConfig, BrowserConfig, GoalsConfig, McpConfig,
            ModelConfig, PluginMarketplaceConfig, SkillsConfig, TuiConfig, UpdatesConfig,
            VoiceConfig,
        };

        if other.model != ModelConfig::default() {
            self.model = other.model;
        }
        // Attachment fallback models (image/audio/video/document) must survive
        // global + project merge; without this, TUI/API overrides are lost on reload.
        if other.attachment_models != AttachmentModelsConfig::default() {
            self.attachment_models = other.attachment_models;
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
        if other.background_models != BackgroundModelsConfig::default() {
            self.background_models = other.background_models;
        }
        if other.goals != GoalsConfig::default() {
            self.goals = other.goals;
        }
        if other.updates != UpdatesConfig::default() {
            self.updates = other.updates;
        }
        // Remote dictation / local ASR — only override when the file actually
        // customizes [voice] (serde fills defaults for missing tables).
        if other.voice != VoiceConfig::default() {
            self.voice = other.voice;
        }
        if other.browser != BrowserConfig::default() {
            self.browser = other.browser;
        }
        crate::config::providers::merge_provider_configs(&mut self.providers, other.providers);
        self.plugins.extend(other.plugins);
        self.wasm_plugins.extend(other.wasm_plugins);
    }
}

/// Saves the config to the global config path, creating parent directories if needed.
/// Strips model lists from providers that exist in the registry catalog so the
/// config.toml stays clean — the registry SQLite is the authoritative source
/// for model metadata, not the config file.
pub fn save_global_config(global_path: &Path, config: &NaviConfig) -> Result<PathBuf> {
    if let Some(parent) = global_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut config_to_save = config.clone();
    strip_registry_provider_models(&mut config_to_save);
    let content = toml::to_string_pretty(&config_to_save).context("failed to serialize config")?;
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
    let mut config_to_save = config.clone();
    strip_registry_provider_models(&mut config_to_save);
    let content = toml::to_string_pretty(&config_to_save).context("failed to serialize config")?;
    fs::write(&project_path, &content)
        .with_context(|| format!("failed to write {}", project_path.display()))?;
    Ok(project_path)
}

/// Removes model lists from provider overrides that match a provider in the
/// registry catalog. The registry is the authoritative source for model
/// metadata (context_window_tokens, pricing, etc). Provider overrides in
/// config.toml should only carry provider-level settings (base_url,
/// request_options, etc), not model lists.
fn strip_registry_provider_models(config: &mut NaviConfig) {
    let registry_ids: std::collections::HashSet<String> =
        crate::config::providers::base_provider_catalog()
            .into_iter()
            .map(|p| p.id)
            .collect();

    for provider in &mut config.providers {
        if registry_ids.contains(&provider.id) {
            provider.models.clear();
        }
    }
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

pub(crate) fn navi_dirs() -> Result<ProjectDirs> {
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
desktop_notifications = false
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
        assert!(!config.tui.desktop_notifications);
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

    #[test]
    fn global_config_merges_voice_remote_provider() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[voice]
provider = "openai"
model = "whisper-1"
enabled = true
"#,
        )
        .expect("write config");

        let mut config = NaviConfig::default();
        assert_eq!(config.voice.provider, "local");
        merge_from_file(&mut config, &path, ConfigSource::Trusted).expect("merge");

        assert_eq!(config.voice.provider, "openai");
        assert_eq!(config.voice.model, "whisper-1");
        assert!(config.voice.enabled);
        assert!(config.voice.uses_remote_transcription());
    }

    #[test]
    fn missing_voice_table_does_not_wipe_existing_provider() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[model]
provider = "openai"
name = "gpt-test"
"#,
        )
        .expect("write config");

        let mut config = NaviConfig::default();
        config.voice.provider = "groq".to_string();
        config.voice.model = "whisper-large-v3-turbo".to_string();
        merge_from_file(&mut config, &path, ConfigSource::Project).expect("merge");

        assert_eq!(config.voice.provider, "groq");
        assert_eq!(config.voice.model, "whisper-large-v3-turbo");
    }

    #[test]
    fn global_config_merges_attachment_models() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[attachment_models.image]
provider = "openai"
name = "gpt-4o"

[attachment_models.document]
provider = "anthropic"
name = "claude-sonnet-4-20250514"
"#,
        )
        .expect("write config");

        let mut config = NaviConfig::default();
        assert!(config.attachment_models.image.is_none());
        merge_from_file(&mut config, &path, ConfigSource::Trusted).expect("merge");

        let image = config.attachment_models.image.expect("image override");
        assert_eq!(image.provider, "openai");
        assert_eq!(image.name, "gpt-4o");
        let document = config.attachment_models.document.expect("document override");
        assert_eq!(document.provider, "anthropic");
        assert_eq!(document.name, "claude-sonnet-4-20250514");
        assert!(config.attachment_models.audio.is_none());
        assert!(config.attachment_models.video.is_none());
    }

    #[test]
    fn missing_attachment_models_table_does_not_wipe_existing() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[model]
provider = "openai"
name = "gpt-test"
"#,
        )
        .expect("write config");

        let mut config = NaviConfig::default();
        config.attachment_models.image = Some(crate::config::types::ModelConfig {
            provider: "xai".to_string(),
            name: "grok-2-vision".to_string(),
        });
        merge_from_file(&mut config, &path, ConfigSource::Project).expect("merge");

        let image = config.attachment_models.image.expect("preserved image override");
        assert_eq!(image.provider, "xai");
        assert_eq!(image.name, "grok-2-vision");
    }

    #[test]
    fn save_and_reload_preserves_attachment_models() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("config.toml");

        let mut config = NaviConfig::default();
        config.attachment_models.image = Some(crate::config::types::ModelConfig {
            provider: "google-gemini".to_string(),
            name: "gemini-2.5-flash".to_string(),
        });
        config.attachment_models.audio = Some(crate::config::types::ModelConfig {
            provider: "openai".to_string(),
            name: "gpt-4o-audio-preview".to_string(),
        });
        save_global_config(&path, &config).expect("save");

        let mut reloaded = NaviConfig::default();
        merge_from_file(&mut reloaded, &path, ConfigSource::Trusted).expect("merge");

        let image = reloaded.attachment_models.image.expect("image");
        assert_eq!(image.provider, "google-gemini");
        assert_eq!(image.name, "gemini-2.5-flash");
        let audio = reloaded.attachment_models.audio.expect("audio");
        assert_eq!(audio.provider, "openai");
        assert_eq!(audio.name, "gpt-4o-audio-preview");
    }
}
