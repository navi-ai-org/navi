use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NaviConfig {
    pub model: ModelConfig,
    pub harness: HarnessConfig,
    pub approvals: ApprovalConfig,
    pub security: SecurityConfig,
    pub logging: LoggingConfig,
    pub providers: Vec<ProviderConfig>,
    pub plugins: Vec<PluginConfig>,
    pub memory: MemoryConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    pub provider: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HarnessConfig {
    pub profile: HarnessProfile,
    pub observation_bytes_small: usize,
    pub observation_bytes_medium: usize,
    pub micro_compact_gap_minutes: u64,
    pub autocompact_buffer_tokens: u64,
    pub autocompact_warning_buffer_tokens: u64,
    pub autocompact_error_buffer_tokens: u64,
    pub autocompact_max_output_tokens: u64,
    pub autocompact_max_consecutive_failures: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessProfile {
    Auto,
    Small,
    Medium,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApprovalConfig {
    pub allow_reads: bool,
    pub require_for_writes: bool,
    pub require_for_commands: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    pub restrict_paths_to_project: bool,
    pub protect_git_metadata: bool,
    pub redact_secrets_in_sessions: bool,
    pub allow_external_plugins: bool,
    pub blocked_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub enabled: bool,
    pub level: String,
    pub file_enabled: bool,
    pub stdout_enabled: bool,
    pub retention_days: u64,
    pub max_files: usize,
    pub include_payloads: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    pub kind: ProviderKind,
    pub api_key_env: String,
    pub base_url: Option<String>,
    #[serde(default)]
    pub models: Vec<ProviderModelConfig>,
    #[serde(default)]
    pub request_timeout_ms: Option<u64>,
    #[serde(default)]
    pub stream_idle_timeout_ms: Option<u64>,
    #[serde(default)]
    pub request_max_retries: Option<u32>,
    #[serde(default)]
    pub stream_max_retries: Option<u32>,
    #[serde(default)]
    pub websocket_connect_timeout_ms: Option<u64>,
    #[serde(default)]
    pub retry_429: Option<bool>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            description: String::new(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: String::new(),
            base_url: None,
            models: Vec::new(),
            request_timeout_ms: None,
            stream_idle_timeout_ms: None,
            request_max_retries: None,
            stream_max_retries: None,
            websocket_connect_timeout_ms: None,
            retry_429: None,
        }
    }
}

impl ProviderConfig {
    pub fn request_timeout_ms(&self) -> u64 {
        self.request_timeout_ms.unwrap_or(120_000)
    }

    pub fn stream_idle_timeout_ms(&self) -> u64 {
        self.stream_idle_timeout_ms.unwrap_or(300_000)
    }

    pub fn request_max_retries(&self) -> u32 {
        self.request_max_retries.unwrap_or(4)
    }

    pub fn stream_max_retries(&self) -> u32 {
        self.stream_max_retries.unwrap_or(5)
    }

    pub fn websocket_connect_timeout_ms(&self) -> u64 {
        self.websocket_connect_timeout_ms.unwrap_or(15_000)
    }

    pub fn retry_429(&self) -> bool {
        self.retry_429.unwrap_or(false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    OpenAiResponses,
    OpenAiChatCompletions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModelConfig {
    pub name: String,
    pub task_size: ModelTaskSize,
    #[serde(default)]
    pub context_window_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelTaskSize {
    Large,
    Small,
}

#[derive(Debug, Clone)]
pub struct ModelOption {
    pub name: String,
    pub provider_id: String,
    pub provider_label: String,
    pub provider_description: String,
    pub task_size: ModelTaskSize,
    pub context_window_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    pub path: PathBuf,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    pub session_memory_enabled: bool,
    pub max_memory_entries: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            session_memory_enabled: false,
            max_memory_entries: 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: NaviConfig,
    pub global_config_path: Option<PathBuf>,
    pub project_config_path: Option<PathBuf>,
    pub data_dir: PathBuf,
}

impl NaviConfig {
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

    fn merge(&mut self, other: NaviConfig) {
        if other.model != ModelConfig::default() {
            self.model = other.model;
        }
        self.harness = other.harness;
        self.approvals = other.approvals;
        self.security = other.security;
        self.logging = other.logging;
        self.memory = other.memory;
        merge_provider_configs(&mut self.providers, other.providers);
        self.plugins.extend(other.plugins);
    }
}

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

impl Default for NaviConfig {
    fn default() -> Self {
        Self {
            model: ModelConfig::default(),
            harness: HarnessConfig::default(),
            approvals: ApprovalConfig::default(),
            security: SecurityConfig::default(),
            logging: LoggingConfig::default(),
            providers: Vec::new(),
            plugins: Vec::new(),
            memory: MemoryConfig::default(),
        }
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            name: "gpt-5.5".to_string(),
        }
    }
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            profile: HarnessProfile::Auto,
            observation_bytes_small: 12 * 1024,
            observation_bytes_medium: 48 * 1024,
            micro_compact_gap_minutes: 60,
            autocompact_buffer_tokens: 13_000,
            autocompact_warning_buffer_tokens: 20_000,
            autocompact_error_buffer_tokens: 20_000,
            autocompact_max_output_tokens: 20_000,
            autocompact_max_consecutive_failures: 3,
        }
    }
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            allow_reads: true,
            require_for_writes: true,
            require_for_commands: true,
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            restrict_paths_to_project: true,
            protect_git_metadata: true,
            redact_secrets_in_sessions: true,
            allow_external_plugins: false,
            blocked_commands: default_blocked_commands(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            level: "info".to_string(),
            file_enabled: true,
            stdout_enabled: false,
            retention_days: 14,
            max_files: 30,
            include_payloads: false,
        }
    }
}

fn default_blocked_commands() -> Vec<String> {
    [
        "rm", "rmdir", "shred", "mkfs", "dd", "sudo", "su", "doas", "chmod", "chown", "chgrp",
        "mount", "umount", "reboot", "shutdown", "poweroff",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
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

fn default_true() -> bool {
    true
}

pub fn provider_catalog(config: &NaviConfig) -> Vec<ProviderConfig> {
    let mut providers = built_in_providers();
    merge_provider_configs(&mut providers, config.providers.clone());
    providers
}

pub fn canonical_provider_id(id: &str) -> &str {
    match id {
        "opencode-zen" => "opencode",
        other => other,
    }
}

pub fn resolve_provider_config(config: &NaviConfig, id: &str) -> Option<ProviderConfig> {
    let canonical_id = canonical_provider_id(id);
    provider_catalog(config)
        .into_iter()
        .find(|provider| canonical_provider_id(&provider.id) == canonical_id)
}

pub fn available_model_options(config: &NaviConfig) -> Vec<ModelOption> {
    provider_catalog(config)
        .into_iter()
        .flat_map(|provider| {
            let desc = provider.description.clone();
            provider
                .models
                .clone()
                .into_iter()
                .map(move |model| ModelOption {
                    name: model.name,
                    provider_id: provider.id.clone(),
                    provider_label: provider.label.clone(),
                    provider_description: desc.clone(),
                    task_size: model.task_size,
                    context_window_tokens: model.context_window_tokens,
                })
        })
        .collect()
}

pub fn effective_context_window(config: &NaviConfig) -> u64 {
    let selected_provider = &config.model.provider;
    let selected_model = &config.model.name;
    available_model_options(config)
        .into_iter()
        .find(|m| m.provider_id == *selected_provider && m.name == *selected_model)
        .and_then(|m| m.context_window_tokens)
        .unwrap_or(DEFAULT_CONTEXT_WINDOW)
}

fn merge_provider_configs(providers: &mut Vec<ProviderConfig>, overrides: Vec<ProviderConfig>) {
    for override_config in overrides {
        if let Some(existing) = providers.iter_mut().find(|provider| {
            canonical_provider_id(&provider.id) == canonical_provider_id(&override_config.id)
        }) {
            *existing = override_config;
            existing.id = canonical_provider_id(&existing.id).to_string();
        } else {
            providers.push(override_config);
        }
    }
}

fn built_in_providers() -> Vec<ProviderConfig> {
    vec![
        // ─── Tier 1: Major cloud providers ─────────────────────────────────────────
        ProviderConfig {
            id: "openai".to_string(),
            label: "OpenAI".to_string(),
            description: "ChatGPT Plus/Pro or API key".to_string(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: "OPENAI_API_KEY".to_string(),
            base_url: Some("https://api.openai.com/v1".to_string()),
            models: vec![
                model_ctx("gpt-5.5", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5.4", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5.4-codex", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5.4-mini", ModelTaskSize::Small, 512_000),
                model_ctx("gpt-5.3-codex", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5.2", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5.1-codex", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5.1-codex-mini", ModelTaskSize::Small, 512_000),
                model_ctx("gpt-5.1", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5.1-mini", ModelTaskSize::Small, 512_000),
                model_ctx("gpt-5", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-5-mini", ModelTaskSize::Small, 512_000),
                model_ctx("gpt-5-nano", ModelTaskSize::Small, 256_000),
                model_ctx("gpt-4.1", ModelTaskSize::Large, 1_000_000),
                model_ctx("gpt-4.1-mini", ModelTaskSize::Small, 512_000),
                model_ctx("gpt-4.1-nano", ModelTaskSize::Small, 256_000),
                model_ctx("gpt-4o", ModelTaskSize::Large, 128_000),
                model_ctx("gpt-4o-mini", ModelTaskSize::Small, 128_000),
                model_ctx("chatgpt-4o-latest", ModelTaskSize::Large, 128_000),
                model_ctx("gpt-4.5-preview", ModelTaskSize::Large, 128_000),
                model_ctx("o3", ModelTaskSize::Large, 200_000),
                model_ctx("o3-pro", ModelTaskSize::Large, 200_000),
                model_ctx("o3-mini", ModelTaskSize::Small, 200_000),
                model_ctx("o4-mini", ModelTaskSize::Small, 200_000),
                model_ctx("o1", ModelTaskSize::Large, 200_000),
                model_ctx("o1-pro", ModelTaskSize::Large, 200_000),
                model_ctx("o1-mini", ModelTaskSize::Small, 128_000),
                model_ctx("gpt-oss-120b", ModelTaskSize::Large, 128_000),
                model_ctx("gpt-oss-20b", ModelTaskSize::Small, 128_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "anthropic".to_string(),
            label: "Anthropic".to_string(),
            description: "Claude models via API key".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
            base_url: Some("https://api.anthropic.com/v1".to_string()),
            models: vec![
                model_ctx("claude-opus-4", ModelTaskSize::Large, 200_000),
                model_ctx("claude-opus-4-1-20250805", ModelTaskSize::Large, 200_000),
                model_ctx("claude-opus-4-20250514", ModelTaskSize::Large, 200_000),
                model_ctx("claude-sonnet-4", ModelTaskSize::Large, 200_000),
                model_ctx("claude-sonnet-4-20250514", ModelTaskSize::Large, 200_000),
                model_ctx("claude-haiku-4", ModelTaskSize::Small, 200_000),
                model_ctx("claude-3.7-sonnet", ModelTaskSize::Large, 200_000),
                model_ctx("claude-3-7-sonnet-20250219", ModelTaskSize::Large, 200_000),
                model_ctx("claude-3.5-sonnet", ModelTaskSize::Large, 200_000),
                model_ctx("claude-3-5-sonnet-20241022", ModelTaskSize::Large, 200_000),
                model_ctx("claude-3-5-sonnet-20240620", ModelTaskSize::Large, 200_000),
                model_ctx("claude-3.5-haiku", ModelTaskSize::Small, 200_000),
                model_ctx("claude-3-5-haiku-20241022", ModelTaskSize::Small, 200_000),
                model_ctx("claude-3-opus-20240229", ModelTaskSize::Large, 200_000),
                model_ctx("claude-3-sonnet-20240229", ModelTaskSize::Large, 200_000),
                model_ctx("claude-3-haiku-20240307", ModelTaskSize::Small, 200_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "github-copilot".to_string(),
            label: "GitHub Copilot".to_string(),
            description: "GitHub Copilot OAuth device sign-in".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "GITHUB_COPILOT_TOKEN".to_string(),
            base_url: Some("https://api.githubcopilot.com".to_string()),
            models: vec![
                model("gpt-5.1-codex", ModelTaskSize::Large),
                model("gpt-5.1", ModelTaskSize::Large),
                model("gpt-5-mini", ModelTaskSize::Small),
                model("claude-sonnet-4.5", ModelTaskSize::Large),
                model("claude-haiku-4.5", ModelTaskSize::Small),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "google-gemini".to_string(),
            label: "Google Gemini".to_string(),
            description: "Gemini API key".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "GEMINI_API_KEY".to_string(),
            base_url: Some("https://generativelanguage.googleapis.com/v1beta/openai/".to_string()),
            models: vec![
                model_ctx("gemini-2.5-pro", ModelTaskSize::Large, 1_000_000),
                model_ctx(
                    "gemini-2.5-pro-preview-06-05",
                    ModelTaskSize::Large,
                    1_000_000,
                ),
                model_ctx("gemini-2.5-flash", ModelTaskSize::Small, 1_000_000),
                model_ctx(
                    "gemini-2.5-flash-preview-05-20",
                    ModelTaskSize::Small,
                    1_000_000,
                ),
                model_ctx("gemini-2.5-flash-lite", ModelTaskSize::Small, 1_000_000),
                model_ctx("gemini-2.0-flash", ModelTaskSize::Small, 1_000_000),
                model_ctx("gemini-2.0-flash-001", ModelTaskSize::Small, 1_000_000),
                model_ctx("gemini-2.0-flash-lite", ModelTaskSize::Small, 1_000_000),
                model_ctx("gemini-1.5-pro", ModelTaskSize::Large, 2_000_000),
                model_ctx("gemini-1.5-pro-002", ModelTaskSize::Large, 2_000_000),
                model_ctx("gemini-1.5-flash", ModelTaskSize::Small, 1_000_000),
                model_ctx("gemini-1.5-flash-002", ModelTaskSize::Small, 1_000_000),
                model_ctx("gemini-1.5-flash-8b", ModelTaskSize::Small, 1_000_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "xai".to_string(),
            label: "xAI".to_string(),
            description: "Grok models via xAI API".to_string(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: "XAI_API_KEY".to_string(),
            base_url: Some("https://api.x.ai/v1".to_string()),
            models: vec![
                model_ctx("grok-4.3", ModelTaskSize::Large, 256_000),
                model_ctx("grok-4", ModelTaskSize::Large, 256_000),
                model_ctx("grok-4-fast", ModelTaskSize::Small, 131_072),
                model_ctx("grok-4-fast-reasoning", ModelTaskSize::Large, 256_000),
                model_ctx("grok-4-fast-non-reasoning", ModelTaskSize::Small, 131_072),
                model_ctx("grok-3", ModelTaskSize::Large, 131_072),
                model_ctx("grok-3-fast", ModelTaskSize::Small, 131_072),
                model_ctx("grok-3-mini", ModelTaskSize::Small, 131_072),
                model_ctx("grok-3-mini-fast", ModelTaskSize::Small, 131_072),
                model_ctx("grok-2-1212", ModelTaskSize::Large, 131_072),
                model_ctx("grok-2-vision-1212", ModelTaskSize::Large, 131_072),
                model_ctx("grok-build-0.1", ModelTaskSize::Large, 256_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "mistral".to_string(),
            label: "Mistral".to_string(),
            description: "Mistral AI API".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "MISTRAL_API_KEY".to_string(),
            base_url: Some("https://api.mistral.ai/v1".to_string()),
            models: vec![
                model("mistral-large-latest", ModelTaskSize::Large),
                model("mistral-large-2411", ModelTaskSize::Large),
                model("mistral-large-2407", ModelTaskSize::Large),
                model("mistral-medium-latest", ModelTaskSize::Large),
                model("mistral-medium-2508", ModelTaskSize::Large),
                model("mistral-small-latest", ModelTaskSize::Small),
                model("mistral-small-2506", ModelTaskSize::Small),
                model("mistral-small-2503", ModelTaskSize::Small),
                model("codestral-latest", ModelTaskSize::Large),
                model("codestral-2508", ModelTaskSize::Large),
                model("codestral-2501", ModelTaskSize::Large),
                model("codestral-2405", ModelTaskSize::Large),
                model("devstral-medium-latest", ModelTaskSize::Large),
                model("devstral-medium-2507", ModelTaskSize::Large),
                model("devstral-small-latest", ModelTaskSize::Small),
                model("devstral-small-2507", ModelTaskSize::Small),
                model("devstral-small-2505", ModelTaskSize::Small),
                model("magistral-medium-latest", ModelTaskSize::Large),
                model("magistral-small-latest", ModelTaskSize::Small),
                model("pixtral-large-latest", ModelTaskSize::Large),
                model("pixtral-12b-2409", ModelTaskSize::Small),
                model("open-mistral-nemo", ModelTaskSize::Small),
                model("open-mixtral-8x22b", ModelTaskSize::Large),
                model("open-mixtral-8x7b", ModelTaskSize::Small),
                model("open-mistral-7b", ModelTaskSize::Small),
                model("ministral-8b-latest", ModelTaskSize::Small),
                model("ministral-3b-latest", ModelTaskSize::Small),
            ],
            ..Default::default()
        },
        // ─── Tier 2: High-quality specialized ─────────────────────────────────────
        ProviderConfig {
            id: "deepseek".to_string(),
            label: "DeepSeek".to_string(),
            description: "DeepSeek API".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            base_url: Some("https://api.deepseek.com".to_string()),
            models: vec![
                model_ctx("deepseek-v4-pro", ModelTaskSize::Large, 128_000),
                model_ctx("deepseek-v4-flash", ModelTaskSize::Small, 128_000),
                model_ctx("deepseek-chat", ModelTaskSize::Large, 128_000),
                model_ctx("deepseek-reasoner", ModelTaskSize::Large, 128_000),
                model_ctx("deepseek-coder", ModelTaskSize::Large, 128_000),
                model_ctx("deepseek-coder-v2", ModelTaskSize::Large, 128_000),
                model_ctx("deepseek-coder-v2-lite", ModelTaskSize::Small, 128_000),
                model_ctx("deepseek-v3", ModelTaskSize::Large, 128_000),
                model_ctx("deepseek-v3.1", ModelTaskSize::Large, 128_000),
                model_ctx("deepseek-v3.2", ModelTaskSize::Large, 128_000),
                model_ctx("deepseek-r1", ModelTaskSize::Large, 128_000),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "moonshot".to_string(),
            label: "Moonshot AI".to_string(),
            description: "Kimi models via API".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "MOONSHOT_API_KEY".to_string(),
            base_url: Some("https://api.moonshot.cn/v1".to_string()),
            models: vec![
                model("kimi-k2.6", ModelTaskSize::Large),
                model("kimi-k2.5", ModelTaskSize::Large),
                model("kimi-k2-thinking", ModelTaskSize::Large),
                model("kimi-k2", ModelTaskSize::Large),
                model("kimi-k2-0711-preview", ModelTaskSize::Large),
                model("kimi-latest", ModelTaskSize::Large),
                model("kimi-thinking-preview", ModelTaskSize::Large),
                model("moonshot-v1-128k", ModelTaskSize::Large),
                model("moonshot-v1-32k", ModelTaskSize::Small),
                model("moonshot-v1-8k", ModelTaskSize::Small),
                model("moonshot-v1-auto", ModelTaskSize::Large),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "zai".to_string(),
            label: "Z.AI".to_string(),
            description: "GLM models by Zhipu AI".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "ZAI_API_KEY".to_string(),
            base_url: Some("https://api.z.ai/api/paas/v4/".to_string()),
            models: vec![
                model("glm-5.1", ModelTaskSize::Large),
                model("glm-5", ModelTaskSize::Large),
                model("glm-5-turbo", ModelTaskSize::Small),
                model("glm-4.7", ModelTaskSize::Large),
                model("glm-4.6", ModelTaskSize::Small),
                model("glm-4.5", ModelTaskSize::Large),
                model("glm-4.5-air", ModelTaskSize::Small),
                model("glm-4.5-x", ModelTaskSize::Large),
                model("glm-4.5-flash", ModelTaskSize::Small),
                model("glm-4-plus", ModelTaskSize::Large),
                model("glm-4-flash", ModelTaskSize::Small),
                model("glm-4-long", ModelTaskSize::Large),
                model("glm-4-air", ModelTaskSize::Small),
                model("glm-4-airx", ModelTaskSize::Small),
                model("glm-4-0520", ModelTaskSize::Large),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "zai-coding".to_string(),
            label: "Z.AI Coding Plan".to_string(),
            description: "Dedicated coding endpoint".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "ZAI_API_KEY".to_string(),
            base_url: Some("https://api.z.ai/api/coding/paas/v4/".to_string()),
            models: vec![
                model("glm-5.1", ModelTaskSize::Large),
                model("glm-5", ModelTaskSize::Large),
                model("glm-5-turbo", ModelTaskSize::Small),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "minimax".to_string(),
            label: "MiniMax".to_string(),
            description: "MiniMax platform".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "MINIMAX_API_KEY".to_string(),
            base_url: Some("https://api.minimax.io/v1".to_string()),
            models: vec![
                model("MiniMax-M2.7", ModelTaskSize::Large),
                model("MiniMax-M2.5", ModelTaskSize::Large),
                model("MiniMax-M2.1", ModelTaskSize::Small),
                model("MiniMax-Text-01", ModelTaskSize::Large),
                model("MiniMax-Text-01-456B", ModelTaskSize::Large),
                model("abab6.5-chat", ModelTaskSize::Large),
                model("abab6.5g-chat", ModelTaskSize::Large),
                model("abab6.5s-chat", ModelTaskSize::Small),
                model("abab6.5t-chat", ModelTaskSize::Small),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "groq".to_string(),
            label: "Groq".to_string(),
            description: "Ultra-fast inference".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "GROQ_API_KEY".to_string(),
            base_url: Some("https://api.groq.com/openai/v1".to_string()),
            models: vec![
                model("llama-3.3-70b-versatile", ModelTaskSize::Large),
                model("llama-3.1-8b-instant", ModelTaskSize::Small),
                model("openai/gpt-oss-120b", ModelTaskSize::Large),
                model("openai/gpt-oss-20b", ModelTaskSize::Small),
                model("qwen/qwen3-32b", ModelTaskSize::Small),
                model("deepseek-r1-distill-llama-70b", ModelTaskSize::Large),
                model("moonshotai/kimi-k2-instruct", ModelTaskSize::Large),
                model(
                    "meta-llama/llama-4-maverick-17b-128e-instruct",
                    ModelTaskSize::Large,
                ),
                model(
                    "meta-llama/llama-4-scout-17b-16e-instruct",
                    ModelTaskSize::Small,
                ),
                model("meta-llama/llama-guard-4-12b", ModelTaskSize::Small),
                model("mistral-saba-24b", ModelTaskSize::Small),
                model("gemma2-9b-it", ModelTaskSize::Small),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "openrouter".to_string(),
            label: "OpenRouter".to_string(),
            description: "Unified API for 300+ models".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "OPENROUTER_API_KEY".to_string(),
            base_url: Some("https://openrouter.ai/api/v1".to_string()),
            models: vec![
                model("anthropic/claude-opus-4", ModelTaskSize::Large),
                model("anthropic/claude-sonnet-4", ModelTaskSize::Large),
                model("openai/gpt-5.5", ModelTaskSize::Large),
                model("openai/gpt-5.4", ModelTaskSize::Large),
                model("openai/gpt-4.1", ModelTaskSize::Large),
                model("google/gemini-2.5-pro", ModelTaskSize::Large),
                model("google/gemini-2.5-flash", ModelTaskSize::Small),
                model("deepseek/deepseek-v4-pro", ModelTaskSize::Large),
                model("deepseek/deepseek-chat", ModelTaskSize::Large),
                model("x-ai/grok-4", ModelTaskSize::Large),
                model("x-ai/grok-3", ModelTaskSize::Large),
                model("meta-llama/llama-3.3-70b", ModelTaskSize::Large),
                model("meta-llama/llama-4-maverick", ModelTaskSize::Large),
                model("meta-llama/llama-4-scout", ModelTaskSize::Small),
                model("mistralai/mistral-large", ModelTaskSize::Large),
                model("mistralai/codestral", ModelTaskSize::Large),
                model("qwen/qwen3-coder", ModelTaskSize::Large),
                model("qwen/qwen3-235b-a22b", ModelTaskSize::Large),
                model("qwen/qwen3-32b", ModelTaskSize::Small),
                model("z-ai/glm-4.5", ModelTaskSize::Large),
                model("moonshotai/kimi-k2", ModelTaskSize::Large),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "stepfun".to_string(),
            label: "StepFun".to_string(),
            description: "StepFun AI models".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "STEP_API_KEY".to_string(),
            base_url: Some("https://api.stepfun.ai/v1".to_string()),
            models: vec![
                model("step-3.5-flash", ModelTaskSize::Large),
                model("step-3", ModelTaskSize::Large),
                model("step-2-16k", ModelTaskSize::Large),
                model("step-2", ModelTaskSize::Large),
                model("step-1-256k", ModelTaskSize::Large),
                model("step-1-128k", ModelTaskSize::Large),
                model("step-1-32k", ModelTaskSize::Small),
                model("step-1-8k", ModelTaskSize::Small),
                model("step-1v", ModelTaskSize::Small),
                model("step-1", ModelTaskSize::Small),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "xiaomi".to_string(),
            label: "Xiaomi".to_string(),
            description: "MiMo models".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "XIAOMI_API_KEY".to_string(),
            base_url: Some("https://api.mimo-v2.com/v1".to_string()),
            models: vec![
                model("mimo-v2.5-pro", ModelTaskSize::Large),
                model("mimo-v2.5", ModelTaskSize::Large),
                model("mimo-v2-omni", ModelTaskSize::Small),
                model("mimo-v2", ModelTaskSize::Small),
                model("mimo-v1", ModelTaskSize::Small),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "nvidia".to_string(),
            label: "Nvidia".to_string(),
            description: "NIM inference microservices".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "NVIDIA_API_KEY".to_string(),
            base_url: Some("https://integrate.api.nvidia.com/v1".to_string()),
            models: vec![
                model("meta/llama-3.3-70b-instruct", ModelTaskSize::Large),
                model("meta/llama-3.1-8b-instruct", ModelTaskSize::Small),
                model("mistralai/mistral-7b-instruct", ModelTaskSize::Small),
                model(
                    "nvidia/llama-3.1-nemotron-70b-instruct",
                    ModelTaskSize::Large,
                ),
                model("qwen/qwen2.5-coder-32b-instruct", ModelTaskSize::Large),
                model("mistralai/mixtral-8x7b-instruct", ModelTaskSize::Small),
                model("mistralai/mistral-large", ModelTaskSize::Large),
                model("deepseek-ai/deepseek-r1", ModelTaskSize::Large),
                model("microsoft/phi-4", ModelTaskSize::Small),
            ],
            ..Default::default()
        },
        // ─── Tier 3: Local / self-hosted ──────────────────────────────────────────
        ProviderConfig {
            id: "ollama".to_string(),
            label: "Ollama".to_string(),
            description: "Local + cloud inference".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "OLLAMA_API_KEY".to_string(),
            base_url: Some("http://localhost:11434/v1".to_string()),
            models: vec![
                model("llama3.1", ModelTaskSize::Large),
                model("llama3.2", ModelTaskSize::Small),
                model("llama3.3", ModelTaskSize::Large),
                model("deepseek-r1", ModelTaskSize::Large),
                model("qwen3", ModelTaskSize::Large),
                model("qwen2.5-coder", ModelTaskSize::Large),
                model("qwen2.5-coder:32b", ModelTaskSize::Large),
                model("qwen2.5-coder:14b", ModelTaskSize::Small),
                model("qwen2.5-coder:7b", ModelTaskSize::Small),
                model("codellama", ModelTaskSize::Large),
                model("starcoder2", ModelTaskSize::Small),
                model("granite-code", ModelTaskSize::Small),
                model("gemma3", ModelTaskSize::Small),
                model("gemma3:27b", ModelTaskSize::Large),
                model("gemma3:12b", ModelTaskSize::Small),
                model("mistral", ModelTaskSize::Small),
                model("devstral", ModelTaskSize::Small),
                model("phi4", ModelTaskSize::Small),
                model("phi4-mini", ModelTaskSize::Small),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "lmstudio".to_string(),
            label: "LMStudio".to_string(),
            description: "Local inference server".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "LMSTUDIO_API_KEY".to_string(),
            base_url: Some("http://localhost:1234/v1".to_string()),
            models: vec![
                model("local-model", ModelTaskSize::Large),
                model("qwen2.5-coder-14b", ModelTaskSize::Small),
                model("qwen2.5-coder-7b", ModelTaskSize::Small),
                model("qwen2.5-coder-32b", ModelTaskSize::Large),
                model("deepseek-r1-distill-qwen-32b", ModelTaskSize::Large),
                model("deepseek-r1-distill-llama-8b", ModelTaskSize::Small),
                model("mistral-small-instruct", ModelTaskSize::Small),
                model("gemma-3-27b-it", ModelTaskSize::Large),
                model("llama-3.2-3b-instruct", ModelTaskSize::Small),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "llamacpp".to_string(),
            label: "Llama.cpp".to_string(),
            description: "Self-hosted GGUF inference".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "LLAMACPP_API_KEY".to_string(),
            base_url: Some("http://localhost:8080/v1".to_string()),
            models: vec![
                model("local-model", ModelTaskSize::Large),
                model("qwen2.5-coder", ModelTaskSize::Large),
                model("deepseek-coder", ModelTaskSize::Large),
                model("starcoder2", ModelTaskSize::Small),
                model("granite-code", ModelTaskSize::Small),
                model("llama3", ModelTaskSize::Large),
                model("mistral", ModelTaskSize::Small),
                model("tinyllama", ModelTaskSize::Small),
            ],
            ..Default::default()
        },
        // ─── Tier 4: Aggregators / value ──────────────────────────────────────────
        ProviderConfig {
            id: "charm-hyper".to_string(),
            label: "Charm Hyper".to_string(),
            description: "Hyper provider".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "CHARM_HYPER_API_KEY".to_string(),
            base_url: None,
            models: vec![
                model("Kimi K2.6", ModelTaskSize::Large),
                model("Kimi K2.5", ModelTaskSize::Large),
                model("DeepSeek V4 Pro", ModelTaskSize::Large),
                model("DeepSeek V4 Flash", ModelTaskSize::Small),
                model("Gemma 4 26B A4B", ModelTaskSize::Small),
                model("GLM-5.1", ModelTaskSize::Large),
                model("GLM-5", ModelTaskSize::Large),
                model("Qwen 3 32B", ModelTaskSize::Small),
                model("MiniMax M2.1", ModelTaskSize::Small),
                model("gpt-oss-120b", ModelTaskSize::Large),
                model("gpt-oss-20b", ModelTaskSize::Small),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "opencode".to_string(),
            label: "OpenCode Zen".to_string(),
            description: "Recommended".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "OPENCODE_API_KEY".to_string(),
            base_url: None,
            models: vec![
                model("big-pickle", ModelTaskSize::Large),
                model("deepseek-v4-flash-free", ModelTaskSize::Small),
                model("nemotron-3-super-free", ModelTaskSize::Small),
                model("qwen3.6-plus", ModelTaskSize::Large),
                model("qwen3.5-plus", ModelTaskSize::Large),
                model("kimi-k2.6", ModelTaskSize::Large),
                model("kimi-k2.5", ModelTaskSize::Large),
                model("glm-5.1", ModelTaskSize::Large),
                model("glm-5", ModelTaskSize::Large),
                model("minimax-m2.7", ModelTaskSize::Small),
                model("minimax-m2.5", ModelTaskSize::Small),
                model("grok-build-0.1", ModelTaskSize::Small),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "opencode-go".to_string(),
            label: "OpenCode Go".to_string(),
            description: "Low cost subscription".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "OPENCODE_GO_API_KEY".to_string(),
            base_url: None,
            models: vec![
                model("deepseek-v4-flash", ModelTaskSize::Small),
                model("deepseek-v4-pro", ModelTaskSize::Large),
                model("qwen3.6-plus", ModelTaskSize::Large),
                model("glm-5", ModelTaskSize::Large),
                model("kimi-k2.5", ModelTaskSize::Large),
                model("minimax-m2.5", ModelTaskSize::Small),
            ],
            ..Default::default()
        },
        ProviderConfig {
            id: "custom".to_string(),
            label: "Custom".to_string(),
            description: "User-configured endpoint".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "CUSTOM_API_KEY".to_string(),
            base_url: None,
            models: vec![model("custom-model", ModelTaskSize::Large)],
            ..Default::default()
        },
    ]
}

fn model(name: &str, task_size: ModelTaskSize) -> ProviderModelConfig {
    ProviderModelConfig {
        name: name.to_string(),
        task_size,
        context_window_tokens: None,
    }
}

fn model_ctx(name: &str, task_size: ModelTaskSize, ctx: u64) -> ProviderModelConfig {
    ProviderModelConfig {
        name: name.to_string(),
        task_size,
        context_window_tokens: Some(ctx),
    }
}

pub const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;

impl PartialEq for ModelConfig {
    fn eq(&self, other: &Self) -> bool {
        self.provider == other.provider && self.name == other.name
    }
}

impl NaviConfig {
    pub fn update_provider_models(&mut self, provider_id: &str, model_names: &[String]) {
        let mut existing_models = std::collections::HashMap::new();

        let provider_id = canonical_provider_id(provider_id).to_string();

        if let Some(built_in) = built_in_providers()
            .into_iter()
            .find(|p| canonical_provider_id(&p.id) == provider_id)
        {
            for m in built_in.models {
                existing_models.insert(m.name.clone(), (m.task_size, m.context_window_tokens));
            }
        }

        if let Some(existing_override) = self
            .providers
            .iter()
            .find(|p| canonical_provider_id(&p.id) == provider_id)
        {
            for m in &existing_override.models {
                existing_models.insert(m.name.clone(), (m.task_size, m.context_window_tokens));
            }
        }

        let mut new_models = Vec::new();
        for name in model_names {
            if let Some(&(size, ctx)) = existing_models.get(name) {
                new_models.push(ProviderModelConfig {
                    name: name.clone(),
                    task_size: size,
                    context_window_tokens: ctx,
                });
            } else {
                new_models.push(ProviderModelConfig {
                    name: name.clone(),
                    task_size: determine_task_size(name),
                    context_window_tokens: None,
                });
            }
        }

        if let Some(p) = self
            .providers
            .iter_mut()
            .find(|p| canonical_provider_id(&p.id) == provider_id)
        {
            p.id = provider_id.clone();
            p.models = new_models;
        } else {
            if let Some(mut resolved) = resolve_provider_config(self, &provider_id) {
                resolved.models = new_models;
                self.providers.push(resolved);
            } else {
                self.providers.push(ProviderConfig {
                    id: provider_id.to_string(),
                    label: provider_id.to_string(),
                    description: "Synced dynamically".to_string(),
                    kind: ProviderKind::OpenAiChatCompletions,
                    api_key_env: format!(
                        "{}_API_KEY",
                        provider_id.to_uppercase().replace('-', "_")
                    ),
                    base_url: None,
                    models: new_models,
                    ..Default::default()
                });
            }
        }
    }
}

fn determine_task_size(name: &str) -> ModelTaskSize {
    let name_lower = name.to_lowercase();
    if name_lower.contains("mini")
        || name_lower.contains("flash")
        || name_lower.contains("haiku")
        || name_lower.contains("nano")
        || name_lower.contains("instant")
        || name_lower.contains("lite")
        || name_lower.contains("scout")
        || name_lower.contains("small")
        || name_lower.contains("8b")
        || name_lower.contains("7b")
        || name_lower.contains("3b")
        || name_lower.contains("12b")
    {
        ModelTaskSize::Small
    } else {
        ModelTaskSize::Large
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_config_overrides_global_settings_and_extends_plugins() {
        let mut global = NaviConfig {
            model: ModelConfig {
                provider: "openai".to_string(),
                name: "gpt-5.5".to_string(),
            },
            harness: HarnessConfig::default(),
            approvals: ApprovalConfig {
                allow_reads: true,
                require_for_writes: true,
                require_for_commands: true,
            },
            security: SecurityConfig::default(),
            logging: LoggingConfig::default(),
            providers: Vec::new(),
            plugins: vec![PluginConfig {
                path: PathBuf::from("/global/plugin.so"),
                enabled: true,
            }],
            memory: MemoryConfig::default(),
        };

        global.merge(NaviConfig {
            model: ModelConfig {
                provider: "openai".to_string(),
                name: "gpt-5.4".to_string(),
            },
            harness: HarnessConfig::default(),
            approvals: ApprovalConfig {
                allow_reads: true,
                require_for_writes: false,
                require_for_commands: true,
            },
            security: SecurityConfig::default(),
            logging: LoggingConfig {
                level: "debug".to_string(),
                ..LoggingConfig::default()
            },
            providers: Vec::new(),
            plugins: vec![PluginConfig {
                path: PathBuf::from("./project-plugin.so"),
                enabled: true,
            }],
            memory: MemoryConfig::default(),
        });

        assert_eq!(global.model.name, "gpt-5.4");
        assert!(!global.approvals.require_for_writes);
        assert_eq!(global.logging.level, "debug");
        assert_eq!(global.plugins.len(), 2);
    }

    #[test]
    fn logging_defaults_are_compact_and_file_backed() {
        let config = NaviConfig::default();

        assert!(config.logging.enabled);
        assert_eq!(config.logging.level, "info");
        assert!(config.logging.file_enabled);
        assert!(!config.logging.stdout_enabled);
        assert!(!config.logging.include_payloads);
    }

    #[test]
    fn memory_defaults_are_disabled_with_max_3_entries() {
        let config = NaviConfig::default();

        assert!(!config.memory.session_memory_enabled);
        assert_eq!(config.memory.max_memory_entries, 3);
    }

    #[test]
    fn built_in_provider_catalog_includes_starting_providers() {
        let config = NaviConfig::default();
        let providers = provider_catalog(&config);

        assert!(providers.iter().any(|provider| provider.id == "openai"));
        assert!(
            providers
                .iter()
                .any(|provider| provider.id == "charm-hyper")
        );
        assert!(providers.iter().any(|provider| provider.id == "opencode"));
        assert_eq!(canonical_provider_id("opencode-zen"), "opencode");
        assert_eq!(
            resolve_provider_config(&config, "opencode-zen")
                .expect("opencode alias")
                .id,
            "opencode"
        );
        let opencode = providers
            .iter()
            .find(|provider| provider.id == "opencode")
            .expect("opencode provider");
        assert_eq!(opencode.api_key_env, "OPENCODE_API_KEY");
        assert!(
            opencode
                .models
                .iter()
                .any(|model| model.name == "big-pickle")
        );
        assert!(
            opencode
                .models
                .iter()
                .any(|model| model.name == "nemotron-3-super-free")
        );
        let nvidia = providers
            .iter()
            .find(|provider| provider.id == "nvidia")
            .expect("nvidia provider");
        assert_eq!(
            nvidia.base_url.as_deref(),
            Some("https://integrate.api.nvidia.com/v1")
        );
        assert!(providers.iter().all(|provider| !provider.models.is_empty()));
        assert!(
            providers
                .iter()
                .map(|provider| provider.models.len())
                .sum::<usize>()
                >= 160
        );
    }

    #[test]
    fn custom_provider_config_overrides_built_in_provider() {
        let mut config = NaviConfig::default();
        config.providers.push(ProviderConfig {
            id: "charm-hyper".to_string(),
            label: "Charm Hyper".to_string(),
            description: "Custom override".to_string(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "CUSTOM_CHARM_KEY".to_string(),
            base_url: Some("https://example.test/v1".to_string()),
            models: vec![model("Custom Model", ModelTaskSize::Large)],
            ..Default::default()
        });

        let provider = resolve_provider_config(&config, "charm-hyper").expect("provider");
        assert_eq!(provider.api_key_env, "CUSTOM_CHARM_KEY");
        assert_eq!(provider.models[0].name, "Custom Model");
    }
}
