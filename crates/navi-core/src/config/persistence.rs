use crate::config::providers::dropin;
use crate::config::state::{self, NaviState};
use crate::config::types::{LoadedConfig, NaviConfig};
use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::fs;
use std::path::{Path, PathBuf};

impl NaviConfig {
    /// Loads and merges configuration from the global config file and the
    /// project's `.navi/config.toml`, returning the merged config with paths.
    ///
    /// Layers, in order (later wins):
    ///
    /// 1. built-in defaults
    /// 2. `~/.config/navi/config.toml` — hand-authored, NAVI never writes it
    /// 3. `~/.config/navi/state.toml` — machine-owned (model, TUI prefs, flags)
    /// 4. `<project>/.navi/config.toml` — hand-authored
    /// 5. `<project>/.navi/state.toml` — machine-owned
    /// 6. `providers.d/**/*.toml` drop-in providers (win over `[[providers]]`)
    pub fn load(cwd: &Path) -> Result<LoadedConfig> {
        let dirs = navi_dirs()?;
        // Prefer the XDG-style `~/.config/navi/` when it exists — this is the
        // path documented in AGENTS.md and used on Linux/macOS. On Windows,
        // `directories::ProjectDirs` returns `%APPDATA%\navi\config` which is
        // not where users (following the docs) place their config.
        let global_dir = global_config_dir().unwrap_or_else(|| dirs.config_dir().to_path_buf());
        let global_path = global_dir.join("config.toml");
        let project_path = cwd.join(".navi").join("config.toml");

        let mut config = NaviConfig::default();
        let _ = merge_from_file(&mut config, &global_path, ConfigSource::Trusted)?;

        // Machine-owned state (`state.toml`) sits next to each config file and
        // wins for the keys it owns: the last model/TUI choice made in the UI
        // beats the declared default in `config.toml`.
        let global_state = state::load_state(&state::state_path_for_config(&global_path));
        if let Some(state) = &global_state {
            state::apply_state(state, &mut config);
        }

        let project_config_path =
            merge_from_file(&mut config, &project_path, ConfigSource::Project)?;
        let project_state = state::load_state(&state::state_path_for_config(&project_path));
        if let Some(state) = &project_state {
            state::apply_state(state, &mut config);
        }

        // Additive skill allowlist entries from both state layers. No-op unless
        // the effective `[skills] active` list is non-empty.
        let mut state_layers: Vec<&NaviState> = Vec::new();
        if let Some(state) = &global_state {
            state_layers.push(state);
        }
        if let Some(state) = &project_state {
            state_layers.push(state);
        }
        state::apply_extra_active(&state_layers, &mut config);

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

        // Drop-in provider definitions. These are re-read at runtime by
        // `dropin::apply_if_changed`, so editing a file needs no restart.
        let roots = dropin::roots_for(Some(global_dir.as_path()), cwd);
        dropin::apply_to_config(&mut config, roots);

        // Drop-in MCP servers (`mcp.d/`), merged by id over `[[mcp.servers]]`.
        crate::config::mcp_dropin::apply_to_config(&mut config, &global_dir);

        Ok(LoadedConfig {
            config,
            global_config_path: Some(global_path),
            project_config_path,
            data_dir: dirs.data_local_dir().to_path_buf(),
        })
    }

    pub(crate) fn merge(&mut self, other: NaviConfig) {
        use crate::config::types::{
            AttachmentModelsConfig, BrowserConfig, GoalsConfig, McpConfig, ModelConfig,
            SkillsConfig, TuiConfig, UpdatesConfig,
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
        if other.goals != GoalsConfig::default() {
            self.goals = other.goals;
        }
        if other.updates != UpdatesConfig::default() {
            self.updates = other.updates;
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
    }
}

/// Persists the machine-owned slice of `config` to the `state.toml` that sits
/// next to `global_config_path`.
///
/// NAVI never writes `config.toml`: the hand-authored file keeps its comments,
/// key order and unknown keys, and can be edited while NAVI is running.
pub fn save_state_config(global_config_path: &Path, config: &NaviConfig) -> Result<PathBuf> {
    state::save_state(&state::state_path_for_config(global_config_path), config)
}

/// Persists the machine-owned slice of `config` to `<cwd>/.navi/state.toml`.
pub fn save_project_state(cwd: &Path, config: &NaviConfig) -> Result<PathBuf> {
    let dir = cwd.join(".navi");
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    state::save_state(&state::state_path_in_dir(&dir), config)
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
        // (supply-chain risk). MCP servers belong in the user-global config.
        if file_config.mcp.enabled || !file_config.mcp.servers.is_empty() {
            tracing::warn!(
                path = %path.display(),
                "ignoring [mcp] from project config (use global ~/.config/navi/config.toml)"
            );
        }
        if file_config.acp.enabled || !file_config.acp_agents.is_empty() {
            tracing::warn!(
                path = %path.display(),
                "ignoring [acp]/[[acp_agents]] from project config (use global ~/.config/navi/config.toml)"
            );
        }
        file_config.mcp = crate::config::types::McpConfig::default();
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
/// Prefers the XDG-style `~/.config/navi/` when it exists — this is the path
/// documented in AGENTS.md and used on Linux/macOS. On Windows,
/// `directories::ProjectDirs` returns `%APPDATA%\navi\config` which is not
/// where users (following the docs) place their config. When the XDG-style
/// directory does not exist, falls back to the platform-native
/// `ProjectDirs::config_dir()`.
pub(crate) fn global_config_dir() -> Option<PathBuf> {
    global_config_dir_with(&|key| std::env::var(key).ok())
}

/// [`global_config_dir`] with an injectable environment lookup.
///
/// Injected so tests are deterministic without mutating the process
/// environment (a race with other parallel tests reading `HOME`/
/// `XDG_CONFIG_HOME` made this suite flaky).
fn global_config_dir_with(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(xdg) = env("XDG_CONFIG_HOME")
        && !xdg.trim().is_empty()
    {
        let p = PathBuf::from(xdg).join("navi");
        if p.is_dir() {
            return Some(p);
        }
    }
    let home_candidates: [Option<String>; 2] = [
        env("HOME").filter(|s| !s.trim().is_empty()),
        env("USERPROFILE").filter(|s| !s.trim().is_empty()),
    ];
    for home in home_candidates.iter().flatten() {
        let p = PathBuf::from(home).join(".config").join("navi");
        if p.is_dir() {
            return Some(p);
        }
    }
    navi_dirs().ok().map(|d| d.config_dir().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::McpConfig;

    #[test]
    fn project_config_cannot_enable_mcp() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[model]
provider = "openai"
name = "gpt-test"

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
    fn state_file_is_loaded_next_to_the_config_and_wins_for_its_keys() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let config_path = tempdir.path().join("config.toml");
        fs::write(
            &config_path,
            "[model]\nprovider = \"openai\"\nname = \"gpt-5.5\"\n\n[tui]\ntheme = \"default\"\n",
        )
        .expect("write config");

        let mut state = NaviState::default();
        state.model = Some(crate::config::types::ModelConfig {
            provider: "devin".to_string(),
            name: "devin/swe-2".to_string(),
        });
        let mut tui = crate::config::types::TuiConfig::default();
        tui.theme = "lain".to_string();
        state.tui = Some(tui);
        state::write_state(&state::state_path_for_config(&config_path), &state).expect("state");

        let mut config = NaviConfig::default();
        merge_from_file(&mut config, &config_path, ConfigSource::Trusted).expect("merge");
        assert_eq!(config.model.name, "gpt-5.5", "config.toml applies first");

        let state = state::load_state(&state::state_path_for_config(&config_path)).expect("state");
        state::apply_state(&state, &mut config);
        assert_eq!(config.model.name, "devin/swe-2", "state wins for model");
        assert_eq!(config.tui.theme, "lain", "state wins for TUI prefs");
    }

    #[test]
    fn save_state_config_never_rewrites_config_toml() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let config_path = tempdir.path().join("config.toml");
        let original = "# hand written\n[tui]\ntheme = \"lain\"\n";
        fs::write(&config_path, original).expect("write config");

        let mut config = NaviConfig::default();
        config.model.name = "devin/swe-2".to_string();
        let saved = save_state_config(&config_path, &config).expect("save state");

        assert_eq!(saved.file_name().unwrap(), "state.toml");
        assert_eq!(
            fs::read_to_string(&config_path).expect("read config"),
            original,
            "config.toml must stay byte-identical"
        );
    }

    #[test]
    fn drop_in_providers_are_loaded_from_providers_d() {
        use crate::config::providers::dropin::{self, DropInRoots};

        // A local cache slot keeps this test off the process-global scan cache.
        let cache = std::sync::RwLock::new(None);
        let tempdir = tempfile::tempdir().expect("tempdir");
        let providers_dir = tempdir.path().join("providers.d");
        fs::create_dir_all(&providers_dir).expect("mkdir");
        fs::write(
            providers_dir.join("devin.toml"),
            "base_url = \"http://127.0.0.1:8787/v1\"\napi_key_env = \"DEVIN_PROXY_KEY\"\n\n\
             [[models]]\nname = \"devin/swe-2\"\n",
        )
        .expect("write provider");

        let mut config = NaviConfig::default();
        let snapshot = dropin::apply_to_config_in(
            &cache,
            &mut config,
            DropInRoots {
                global: Some(providers_dir),
                project: None,
            },
        );
        assert_eq!(snapshot.provider_count(), 1);
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers[0].id, "devin");
        assert_eq!(config.providers[0].models[0].name, "devin/swe-2");
    }

    #[test]
    fn global_config_dir_prefers_xdg_style_when_exists() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let xdg_config = tempdir.path().join(".config").join("navi");
        fs::create_dir_all(&xdg_config).expect("mkdir");

        // No process-env mutation: the lookup is injected, so parallel tests
        // cannot observe (or corrupt) this test's HOME/XDG values.
        let home = tempdir.path().to_string_lossy().to_string();
        let resolved = global_config_dir_with(&|key| match key {
            "HOME" => Some(home.clone()),
            _ => None,
        });
        assert_eq!(
            resolved,
            Some(xdg_config.clone()),
            "global_config_dir should prefer $HOME/.config/navi when it exists"
        );
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

        let home = tempdir.path().to_string_lossy().to_string();
        let xdg = xdg_root.to_string_lossy().to_string();
        let resolved = global_config_dir_with(&|key| match key {
            "XDG_CONFIG_HOME" => Some(xdg.clone()),
            "HOME" => Some(home.clone()),
            _ => None,
        });
        assert_eq!(
            resolved,
            Some(xdg_config),
            "XDG_CONFIG_HOME should take precedence over $HOME/.config"
        );
    }
}
