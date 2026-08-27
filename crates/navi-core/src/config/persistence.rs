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
        // Prefer the XDG-style `~/.config/navi/` when it exists — this is the
        // path documented in AGENTS.md and used on Linux/macOS. On Windows,
        // `directories::ProjectDirs` returns `%APPDATA%\navi\config` which is
        // not where users (following the docs) place their config.
        let global_dir = global_config_dir().unwrap_or_else(|| dirs.config_dir().to_path_buf());
        let global_path = global_dir.join("config.toml");
        let project_path = cwd.join(".navi").join("config.toml");

        // Capture mtime of the global config file at load time so we can
        // detect external edits before saving (avoids clobbering manual edits
        // while the TUI is open).
        let global_config_mtime = fs::metadata(&global_path)
            .ok()
            .and_then(|m| m.modified().ok());

        let mut config = NaviConfig::default();
        let _ = merge_from_file(&mut config, &global_path, ConfigSource::Trusted)?;
        let project_config_path =
            merge_from_file(&mut config, &project_path, ConfigSource::Project)?;

        // Apply environment overrides for memory configuration
        if let Ok(val) = std::env::var("NAVI_MEMORY_ENABLED")
            && let Ok(b) = val.parse::<bool>()
        {
            config.memory.enabled = b;
        }
        if let Ok(val) = std::env::var("NAVI_MEMORY_REBUILD_THRESHOLD")
            && let Ok(f) = val.parse::<f64>()
        {
            config.memory.rebuild_threshold = f;
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
            data_dir: navi_data_dir().unwrap_or_else(|| dirs.data_local_dir().to_path_buf()),
            app_scope: crate::app_scope::AppScope::none(),
            global_config_mtime,
        })
    }

    pub(crate) fn merge(&mut self, other: NaviConfig) {
        use crate::config::types::{
            AttachmentModelsConfig, BrowserConfig, GoalsConfig, McpConfig, ModelConfig,
            PluginMarketplaceConfig, SkillsConfig, TuiConfig, UpdatesConfig, VoiceConfig,
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
        if other.workflow != crate::config::types::WorkflowConfig::default() {
            self.workflow = other.workflow;
        }
        if other.acp != crate::config::types::AcpConfig::default() {
            self.acp = other.acp;
        }
        if !other.acp_agents.is_empty() {
            self.acp_agents = other.acp_agents;
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
///
/// If the file on disk has been modified since it was loaded (detected via
/// mtime), the on-disk version is reloaded and only the transient TUI state
/// (model selection, TUI preferences, attachment models, recent lists) is
/// overlaid on top. This prevents the TUI from clobbering manual edits made
/// while it was open.
pub fn save_global_config(global_path: &Path, config: &NaviConfig) -> Result<PathBuf> {
    save_global_config_with_mtime(global_path, config, None)
}

/// Like [`save_global_config`] but uses the provided `known_mtime` (captured at
/// load time) to detect external edits. When the on-disk mtime differs, the
/// disk version is reloaded and transient TUI state is merged on top.
pub fn save_global_config_with_mtime(
    global_path: &Path,
    config: &NaviConfig,
    known_mtime: Option<std::time::SystemTime>,
) -> Result<PathBuf> {
    if let Some(parent) = global_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    // Check if the file was externally modified since load.
    let disk_mtime = fs::metadata(global_path)
        .ok()
        .and_then(|m| m.modified().ok());
    let config_to_save = if let Some(known) = known_mtime
        && let Some(disk) = disk_mtime
        && known != disk
    {
        // File changed externally — reload from disk and overlay transient state.
        tracing::info!(
            path = %global_path.display(),
            "config.toml was modified externally since load; merging transient TUI state over disk version"
        );
        let raw = fs::read_to_string(global_path)
            .with_context(|| format!("failed to re-read {}", global_path.display()))?;
        let mut disk_config = toml::from_str::<NaviConfig>(&raw)
            .with_context(|| format!("failed to parse {}", global_path.display()))?;
        overlay_transient_state(&mut disk_config, config);
        disk_config
    } else {
        config.clone()
    };

    let mut config_to_save = config_to_save;
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
///
/// **Exception:** models that carry explicit metadata overrides (e.g.
/// `context_window_tokens`, `max_output_tokens`, `supports_thinking`) are
/// preserved. This allows users to pin custom metadata for registry providers
/// (e.g. when the registry is stale or wrong) without having their edits
/// silently stripped on the next TUI save.
fn strip_registry_provider_models(config: &mut NaviConfig) {
    let registry_ids: std::collections::HashSet<String> =
        crate::config::providers::base_provider_catalog()
            .into_iter()
            .map(|p| p.id)
            .collect();

    for provider in &mut config.providers {
        if registry_ids.contains(&provider.id) {
            // Only strip models that have no explicit metadata overrides.
            // Models with any user-set field (context_window_tokens,
            // max_output_tokens, supports_thinking, etc.) are preserved.
            provider.models.retain(|m| model_has_explicit_metadata(m));
        }
    }
}

/// Returns true if the model carries any explicit metadata that the user
/// might have set manually (as opposed to a bare name-only entry that the
/// registry can fully supply).
fn model_has_explicit_metadata(m: &crate::config::types::ProviderModelConfig) -> bool {
    m.context_window_tokens.is_some()
        || m.max_output_tokens.is_some()
        || m.recommended_temperature.is_some()
        || m.supports_thinking.is_some()
        || m.supports_images.is_some()
        || m.supports_audio.is_some()
        || m.supports_video.is_some()
        || m.supports_documents.is_some()
        || m.tool_prompt_manifest.is_some()
        || m.pricing_input_per_1m.is_some()
        || m.pricing_output_per_1m.is_some()
        || !m.reasoning_levels.is_empty()
        || m.default_reasoning_effort.is_some()
        || m.task_size.is_some()
}

/// Overlays transient TUI state from `tui_state` onto `disk_config`.
///
/// When the config file has been externally modified since the TUI loaded it,
/// we reload the disk version and apply only the fields the TUI typically
/// changes at runtime:
/// - `[model]` (provider + name selection)
/// - `[tui]` (theme, preferences, recent lists)
/// - `[attachment_models]` (image/audio/video/document fallbacks)
///
/// Everything else (providers, models, security, mcp, etc.) is preserved from
/// the disk version, so manual edits are not clobbered.
fn overlay_transient_state(disk_config: &mut NaviConfig, tui_state: &NaviConfig) {
    // Model selection
    disk_config.model = tui_state.model.clone();

    // TUI preferences
    disk_config.tui = tui_state.tui.clone();

    // Attachment fallback models
    disk_config.attachment_models = tui_state.attachment_models.clone();
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
        if file_config.acp.enabled || !file_config.acp_agents.is_empty() {
            tracing::warn!(
                path = %path.display(),
                "ignoring [acp]/[[acp_agents]] from project config (use global ~/.config/navi/config.toml)"
            );
        }
        file_config.plugins.clear();
        file_config.mcp = crate::config::types::McpConfig::default();
        file_config.wasm_plugins.clear();
        file_config.acp = crate::config::types::AcpConfig::default();
        file_config.acp_agents.clear();

        // Computer use deny-list: project config may extend the defaults but
        // cannot remove the "protected" entries (OS security + NAVI
        // self-protection). See ADR 0016 + `merge_deny_apps` in defaults.rs.
        file_config.security.computer_use_deny_apps =
            crate::config::defaults::merge_deny_apps(&file_config.security.computer_use_deny_apps);
    }
    config.merge(file_config);
    Ok(Some(path.to_path_buf()))
}

pub(crate) fn navi_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("dev", "navi", "navi").context("failed to locate user config directory")
}

/// Returns the directory that holds the global user config (`config.toml`,
/// `AGENTS.md`, …).
///
/// When `XDG_CONFIG_HOME` is set and non-empty, always uses
/// `$XDG_CONFIG_HOME/navi` — this is standard XDG behavior and allows test
/// isolation on all platforms (the directory is created on demand by
/// `save_global_config`). On Windows, `directories::ProjectDirs` returns
/// `%APPDATA%\navi\config` which is not where users (following the docs)
/// place their config, so we also check `~/.config/navi` via `HOME` or
/// `USERPROFILE` before falling back to the platform-native path.
pub fn global_config_dir() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.trim().is_empty()
    {
        return Some(PathBuf::from(xdg).join("navi"));
    }
    let home_candidates: [Option<String>; 2] = [
        std::env::var("HOME").ok().filter(|s| !s.trim().is_empty()),
        std::env::var("USERPROFILE")
            .ok()
            .filter(|s| !s.trim().is_empty()),
    ];
    for home in home_candidates.iter().flatten() {
        let p = PathBuf::from(home).join(".config").join("navi");
        if p.is_dir() {
            return Some(p);
        }
    }
    navi_dirs().ok().map(|d| d.config_dir().to_path_buf())
}

/// Returns the NAVI data directory (sessions, logs, plugins, …).
///
/// When `XDG_DATA_HOME` is set and non-empty, uses `$XDG_DATA_HOME/navi` —
/// this is standard XDG behavior and allows test isolation on all platforms.
/// Otherwise falls back to `directories::ProjectDirs::data_local_dir()`.
pub fn navi_data_dir() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
        && !xdg.trim().is_empty()
    {
        return Some(PathBuf::from(xdg).join("navi"));
    }
    navi_dirs().ok().map(|d| d.data_local_dir().to_path_buf())
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
last_message_action = "copy_session"
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
        assert_eq!(config.tui.last_message_action, "copy_session");
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
        let document = config
            .attachment_models
            .document
            .expect("document override");
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

        let image = config
            .attachment_models
            .image
            .expect("preserved image override");
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

    #[test]
    fn global_config_dir_prefers_xdg_style_when_exists() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let xdg_config = tempdir.path().join(".config").join("navi");
        fs::create_dir_all(&xdg_config).expect("mkdir");

        // Save HOME and XDG_CONFIG_HOME, restore on drop.
        let old_home = std::env::var_os("HOME");
        let old_xdg = std::env::var_os("XDG_CONFIG_HOME");
        // On Windows, HOME is typically unset; set it to the temp dir.
        // SAFETY: tests are single-threaded here; no other thread reads env.
        unsafe {
            std::env::set_var("HOME", tempdir.path());
            std::env::remove_var("XDG_CONFIG_HOME");
        }

        let resolved = global_config_dir();
        assert_eq!(
            resolved,
            Some(xdg_config.clone()),
            "global_config_dir should prefer $HOME/.config/navi when it exists"
        );

        // Restore.
        // SAFETY: same single-threaded context.
        unsafe {
            if let Some(v) = old_home {
                std::env::set_var("HOME", v);
            } else {
                std::env::remove_var("HOME");
            }
            if let Some(v) = old_xdg {
                std::env::set_var("XDG_CONFIG_HOME", v);
            } else {
                std::env::remove_var("XDG_CONFIG_HOME");
            }
        }
    }

    #[test]
    fn global_config_dir_xdg_config_home_takes_precedence() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let xdg_root = tempdir.path().join("xdg");
        let xdg_config = xdg_root.join("navi");
        fs::create_dir_all(&xdg_config).expect("mkdir");
        // Also create $HOME/.config/navi to prove XDG_CONFIG_HOME wins.
        let _home_config = tempdir.path().join(".config").join("navi");
        fs::create_dir_all(&_home_config).expect("mkdir");

        let old_home = std::env::var_os("HOME");
        let old_xdg = std::env::var_os("XDG_CONFIG_HOME");
        // SAFETY: tests are single-threaded here; no other thread reads env.
        unsafe {
            std::env::set_var("HOME", tempdir.path());
            std::env::set_var("XDG_CONFIG_HOME", &xdg_root);
        }

        let resolved = global_config_dir();
        assert_eq!(
            resolved,
            Some(xdg_config),
            "XDG_CONFIG_HOME should take precedence over $HOME/.config"
        );

        // SAFETY: same single-threaded context.
        unsafe {
            if let Some(v) = old_home {
                std::env::set_var("HOME", v);
            } else {
                std::env::remove_var("HOME");
            }
            if let Some(v) = old_xdg {
                std::env::set_var("XDG_CONFIG_HOME", v);
            } else {
                std::env::remove_var("XDG_CONFIG_HOME");
            }
        }
    }

    #[test]
    fn strip_preserves_models_with_explicit_metadata() {
        use crate::config::types::{ProviderConfig, ProviderKind, ProviderModelConfig};

        let mut config = NaviConfig::default();
        // "openai" is a registry provider — models would normally be stripped.
        // But models with explicit metadata should be preserved.
        config.providers.push(ProviderConfig {
            id: "openai".to_string(),
            label: "OpenAI".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "OPENAI_API_KEY".to_string(),
            base_url: None,
            models: vec![
                // Bare model — should be stripped
                ProviderModelConfig {
                    name: "gpt-bare".to_string(),
                    ..Default::default()
                },
                // Model with context_window_tokens — should be preserved
                ProviderModelConfig {
                    name: "gpt-custom-ctx".to_string(),
                    context_window_tokens: Some(500_000),
                    ..Default::default()
                },
                // Model with max_output_tokens — should be preserved
                ProviderModelConfig {
                    name: "gpt-custom-output".to_string(),
                    max_output_tokens: Some(32_000),
                    ..Default::default()
                },
                // Model with supports_thinking — should be preserved
                ProviderModelConfig {
                    name: "gpt-thinking".to_string(),
                    supports_thinking: Some(true),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        strip_registry_provider_models(&mut config);

        let provider = &config.providers[0];
        // Bare model should be stripped
        assert!(
            !provider.models.iter().any(|m| m.name == "gpt-bare"),
            "bare model without metadata should be stripped"
        );
        // Models with explicit metadata should be preserved
        assert!(
            provider
                .models
                .iter()
                .any(|m| m.name == "gpt-custom-ctx" && m.context_window_tokens == Some(500_000)),
            "model with context_window_tokens should be preserved"
        );
        assert!(
            provider
                .models
                .iter()
                .any(|m| m.name == "gpt-custom-output" && m.max_output_tokens == Some(32_000)),
            "model with max_output_tokens should be preserved"
        );
        assert!(
            provider
                .models
                .iter()
                .any(|m| m.name == "gpt-thinking" && m.supports_thinking == Some(true)),
            "model with supports_thinking should be preserved"
        );
    }

    #[test]
    fn strip_does_not_touch_custom_providers() {
        use crate::config::types::{ProviderConfig, ProviderKind, ProviderModelConfig};

        let mut config = NaviConfig::default();
        // "cc-bridge" is NOT a registry provider — models should never be stripped.
        config.providers.push(ProviderConfig {
            id: "cc-bridge".to_string(),
            label: "Custom Bridge".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "CC_API_KEY".to_string(),
            base_url: Some("http://127.0.0.1:9092/v1".to_string()),
            models: vec![
                ProviderModelConfig {
                    name: "custom-model-1".to_string(),
                    context_window_tokens: Some(1_000_000),
                    max_output_tokens: Some(384_000),
                    supports_thinking: Some(true),
                    ..Default::default()
                },
                ProviderModelConfig {
                    name: "custom-model-bare".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        strip_registry_provider_models(&mut config);

        let provider = &config.providers[0];
        assert_eq!(
            provider.models.len(),
            2,
            "custom (non-registry) provider models should never be stripped"
        );
    }

    #[test]
    fn save_with_mtime_merges_when_file_changed_externally() {
        use crate::config::types::{
            ModelConfig, ProviderConfig, ProviderKind, ProviderModelConfig, TuiConfig,
        };

        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("config.toml");

        // Write initial config with a custom provider + model metadata
        let initial = r#"
[model]
provider = "cc-bridge"
name = "deepseek/deepseek-v4-pro"

[[providers]]
id = "cc-bridge"
label = "Command Code (bridge)"
description = "Command Code via local bridge"
kind = "openai-chat-completions"
api_key_env = "COMMAND_CODE_API_KEY"
base_url = "http://127.0.0.1:9092/v1"
aggregator = false

[[providers.models]]
name = "deepseek/deepseek-v4-pro"
context_window_tokens = 1000000
max_output_tokens = 384000
supports_thinking = true
"#;
        fs::write(&path, initial).expect("write initial config");
        let mtime_at_load = fs::metadata(&path).ok().and_then(|m| m.modified().ok());

        // Simulate the TUI loading the config and changing only model selection + tui prefs
        let mut tui_config = NaviConfig::default();
        tui_config.model = ModelConfig {
            provider: "cc-bridge".to_string(),
            name: "xiaomi/mimo-v2.5-pro".to_string(),
        };
        tui_config.tui = TuiConfig {
            theme: "dark".to_string(),
            ..Default::default()
        };
        // TUI does NOT have the provider/models in its in-memory snapshot
        // (they were stripped or the TUI never loaded them properly)
        tui_config.providers = vec![ProviderConfig {
            id: "cc-bridge".to_string(),
            label: "Command Code (bridge)".to_string(),
            description: "Command Code via local bridge".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "COMMAND_CODE_API_KEY".to_string(),
            base_url: Some("http://127.0.0.1:9092/v1".to_string()),
            models: vec![ProviderModelConfig {
                name: "deepseek/deepseek-v4-pro".to_string(),
                context_window_tokens: Some(128_000), // wrong value in TUI memory
                max_output_tokens: Some(64_000),
                supports_thinking: Some(true),
                ..Default::default()
            }],
            aggregator: false,
            ..Default::default()
        }];

        // Simulate external edit: change the config on disk (e.g. user edited it)
        std::thread::sleep(std::time::Duration::from_millis(50));
        let external_edit = r#"
[model]
provider = "cc-bridge"
name = "deepseek/deepseek-v4-pro"

[[providers]]
id = "cc-bridge"
label = "Command Code (bridge)"
description = "Command Code via local bridge"
kind = "openai-chat-completions"
api_key_env = "COMMAND_CODE_API_KEY"
base_url = "http://127.0.0.1:9092/v1"
aggregator = false

[[providers.models]]
name = "deepseek/deepseek-v4-pro"
context_window_tokens = 1000000
max_output_tokens = 384000
supports_thinking = true

[[providers.models]]
name = "xiaomi/mimo-v2.5-pro"
context_window_tokens = 1000000
max_output_tokens = 131072
supports_thinking = true
"#;
        fs::write(&path, external_edit).expect("write external edit");

        // Now save with the old mtime — should detect the change and merge
        save_global_config_with_mtime(&path, &tui_config, mtime_at_load)
            .expect("save with mtime merge");

        // Reload and verify:
        // 1. TUI's model selection (mimo-v2.5-pro) should be applied
        // 2. TUI's tui theme should be applied
        // 3. Disk's provider models (with correct 1M/384K metadata) should be preserved
        let saved = fs::read_to_string(&path).expect("read saved config");
        let saved_config = toml::from_str::<NaviConfig>(&saved).expect("parse saved config");

        assert_eq!(
            saved_config.model.name, "xiaomi/mimo-v2.5-pro",
            "TUI model selection should be overlaid on disk config"
        );
        assert_eq!(
            saved_config.tui.theme, "dark",
            "TUI theme should be overlaid on disk config"
        );

        // The disk version had 2 models with correct metadata; TUI had 1 with wrong metadata.
        // After merge, the disk models should be preserved (not the TUI's wrong ones).
        let provider = saved_config
            .providers
            .iter()
            .find(|p| p.id == "cc-bridge")
            .expect("cc-bridge provider");
        let deepseek = provider
            .models
            .iter()
            .find(|m| m.name == "deepseek/deepseek-v4-pro")
            .expect("deepseek model preserved from disk");
        assert_eq!(
            deepseek.context_window_tokens,
            Some(1_000_000),
            "disk model metadata should be preserved, not TUI's stale 128K"
        );
        assert_eq!(
            deepseek.max_output_tokens,
            Some(384_000),
            "disk model metadata should be preserved, not TUI's stale 64K"
        );

        // The second model from disk should also be preserved
        assert!(
            provider
                .models
                .iter()
                .any(|m| m.name == "xiaomi/mimo-v2.5-pro"
                    && m.context_window_tokens == Some(1_000_000)),
            "disk's second model should be preserved"
        );
    }

    #[test]
    fn save_with_mtime_no_merge_when_file_unchanged() {
        use crate::config::types::ModelConfig;

        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("config.toml");

        let initial = r#"
[model]
provider = "openai"
name = "gpt-5"
"#;
        fs::write(&path, initial).expect("write initial");
        let mtime_at_load = fs::metadata(&path).ok().and_then(|m| m.modified().ok());

        // Save with the correct mtime — should NOT trigger merge
        let mut config = NaviConfig::default();
        config.model = ModelConfig {
            provider: "openai".to_string(),
            name: "gpt-5.5".to_string(),
        };
        save_global_config_with_mtime(&path, &config, mtime_at_load).expect("save without merge");

        let saved = fs::read_to_string(&path).expect("read saved");
        let saved_config = toml::from_str::<NaviConfig>(&saved).expect("parse saved");
        assert_eq!(
            saved_config.model.name, "gpt-5.5",
            "normal save (no external edit) should write the in-memory config"
        );
    }

    #[test]
    fn overlay_transient_state_preserves_disk_providers() {
        use crate::config::types::{
            ModelConfig, ProviderConfig, ProviderKind, ProviderModelConfig,
        };

        // Disk config has a custom provider with correct metadata
        let mut disk = NaviConfig::default();
        disk.providers.push(ProviderConfig {
            id: "cc-bridge".to_string(),
            label: "Bridge".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "CC_KEY".to_string(),
            base_url: Some("http://127.0.0.1:9092/v1".to_string()),
            models: vec![ProviderModelConfig {
                name: "deepseek/deepseek-v4-pro".to_string(),
                context_window_tokens: Some(1_000_000),
                max_output_tokens: Some(384_000),
                ..Default::default()
            }],
            ..Default::default()
        });

        // TUI state has different model selection but no provider info
        let mut tui_state = NaviConfig::default();
        tui_state.model = ModelConfig {
            provider: "cc-bridge".to_string(),
            name: "xiaomi/mimo-v2.5-pro".to_string(),
        };

        overlay_transient_state(&mut disk, &tui_state);

        // Model selection should come from TUI state
        assert_eq!(disk.model.name, "xiaomi/mimo-v2.5-pro");
        // Provider from disk should be preserved
        assert_eq!(disk.providers.len(), 1);
        assert_eq!(disk.providers[0].id, "cc-bridge");
        assert_eq!(
            disk.providers[0].models[0].context_window_tokens,
            Some(1_000_000),
            "disk provider metadata should be preserved"
        );
    }
}
