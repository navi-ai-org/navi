use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn default_true() -> bool {
    true
}

/// Top-level NAVI configuration, loaded from TOML and merged across defaults,
/// global config, and project config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NaviConfig {
    /// Selected model provider and name.
    pub model: ModelConfig,
    /// Harness profile and tool-loop limits.
    pub harness: HarnessConfig,
    /// Tool approval behavior.
    pub approvals: ApprovalConfig,
    /// Security constraints (path restrictions, blocked commands, etc.).
    pub security: SecurityConfig,
    /// Structured logging settings.
    pub logging: LoggingConfig,
    /// Provider definitions (built-in overrides and custom providers).
    pub providers: Vec<ProviderConfig>,
    /// Native plugin library paths.
    pub plugins: Vec<PluginConfig>,
    /// Session memory settings.
    pub memory: MemoryConfig,
    /// Skill discovery and activation.
    pub skills: SkillsConfig,
    /// MCP server configuration.
    pub mcp: McpConfig,
}

/// Selected model configuration: provider id and model name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    /// Provider identifier (e.g. `"openai"`, `"anthropic"`).
    pub provider: String,
    /// Model name (e.g. `"gpt-5.5"`, `"claude-sonnet-4-20250514"`).
    pub name: String,
}

/// Harness profile, observation limits, and autocompact thresholds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HarnessConfig {
    /// Selected harness profile (auto/small/medium).
    pub profile: HarnessProfile,
    /// When to include the tool prompt manifest.
    pub tool_prompt_manifest: ToolPromptManifest,
    /// Max observation bytes for the `small` profile.
    pub observation_bytes_small: usize,
    /// Max observation bytes for the `medium` profile.
    pub observation_bytes_medium: usize,
    /// Minutes of idle time before a micro-compact is triggered.
    pub micro_compact_gap_minutes: u64,
    /// Token buffer reserved below the context limit for autocompact.
    pub autocompact_buffer_tokens: u64,
    /// Token buffer at which a compact warning is emitted.
    pub autocompact_warning_buffer_tokens: u64,
    /// Token buffer at which a compact error is emitted.
    pub autocompact_error_buffer_tokens: u64,
    /// Max tokens allowed for a single compact summary output.
    pub autocompact_max_output_tokens: u64,
    /// Max consecutive compact failures before giving up.
    pub autocompact_max_consecutive_failures: u32,
}

/// Harness profile that controls observation limits and prompt complexity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessProfile {
    /// Infer from the selected model's task size.
    Auto,
    /// Constrained limits for small-context models.
    Small,
    /// Standard limits for capable models.
    Medium,
}

/// Controls when the tool prompt manifest is appended to the system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolPromptManifest {
    /// Include when the provider does not support native tool calling.
    Auto,
    /// Always include the manifest.
    Always,
    /// Never include the manifest.
    Never,
}

/// Tool approval behavior: which tool kinds require user confirmation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApprovalConfig {
    /// Whether read-only tools are allowed without approval.
    pub allow_reads: bool,
    /// Whether write tools require explicit approval.
    pub require_for_writes: bool,
    /// Whether command tools require explicit approval.
    pub require_for_commands: bool,
}

/// Security constraints for tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    /// Restrict file tool paths to the project directory.
    pub restrict_paths_to_project: bool,
    /// Deny writes to `.git` and other version-control metadata.
    pub protect_git_metadata: bool,
    /// Redact secrets (API keys, tokens) from saved session events.
    pub redact_secrets_in_sessions: bool,
    /// Allow loading native plugins from configured paths.
    pub allow_external_plugins: bool,
    /// Commands that are always denied (e.g. `"rm -rf /"`).
    pub blocked_commands: Vec<String>,
}

/// Structured logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    /// Master switch for logging.
    pub enabled: bool,
    /// Log level filter (e.g. `"info"`, `"debug"`).
    pub level: String,
    /// Whether to write logs to a file.
    pub file_enabled: bool,
    /// Whether to write logs to stdout.
    pub stdout_enabled: bool,
    /// Number of days to keep old log files.
    pub retention_days: u64,
    /// Maximum number of log files to retain.
    pub max_files: usize,
    /// Whether to include raw payloads in logs (debug only).
    pub include_payloads: bool,
}

/// Configuration for a single model provider, including its API kind, auth
/// env var, base URL, and available models.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Unique provider identifier (e.g. `"openai"`, `"anthropic"`).
    pub id: String,
    /// Human-readable label for display.
    pub label: String,
    /// Optional description of the provider.
    #[serde(default)]
    pub description: String,
    /// API protocol kind.
    pub kind: ProviderKind,
    /// Environment variable name that holds the API key.
    pub api_key_env: String,
    /// Optional custom base URL override.
    pub base_url: Option<String>,
    /// Explicit model list for this provider.
    #[serde(default)]
    pub models: Vec<ProviderModelConfig>,
    /// Request timeout in milliseconds.
    #[serde(default)]
    pub request_timeout_ms: Option<u64>,
    /// Stream idle timeout in milliseconds.
    #[serde(default)]
    pub stream_idle_timeout_ms: Option<u64>,
    /// Max retries for failed requests.
    #[serde(default)]
    pub request_max_retries: Option<u32>,
    /// Max retries for failed stream reads.
    #[serde(default)]
    pub stream_max_retries: Option<u32>,
    /// WebSocket connect timeout in milliseconds.
    #[serde(default)]
    pub websocket_connect_timeout_ms: Option<u64>,
    /// Whether to retry on HTTP 429 (rate limit).
    #[serde(default)]
    pub retry_429: Option<bool>,
    /// Whether to force-include the tool prompt manifest for this provider.
    #[serde(default)]
    pub tool_prompt_manifest: Option<bool>,
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
            tool_prompt_manifest: None,
        }
    }
}

impl ProviderConfig {
    /// Returns the request timeout, defaulting to 120 seconds.
    pub fn request_timeout_ms(&self) -> u64 {
        self.request_timeout_ms.unwrap_or(120_000)
    }

    /// Returns the stream idle timeout, defaulting to 300 seconds.
    pub fn stream_idle_timeout_ms(&self) -> u64 {
        self.stream_idle_timeout_ms.unwrap_or(300_000)
    }

    /// Returns the max request retries, defaulting to 4.
    pub fn request_max_retries(&self) -> u32 {
        self.request_max_retries.unwrap_or(4)
    }

    /// Returns the max stream retries, defaulting to 5.
    pub fn stream_max_retries(&self) -> u32 {
        self.stream_max_retries.unwrap_or(5)
    }

    /// Returns the WebSocket connect timeout, defaulting to 15 seconds.
    pub fn websocket_connect_timeout_ms(&self) -> u64 {
        self.websocket_connect_timeout_ms.unwrap_or(15_000)
    }

    /// Whether to retry on HTTP 429 (rate limit), defaulting to `false`.
    pub fn retry_429(&self) -> bool {
        self.retry_429.unwrap_or(false)
    }
}

/// API protocol kind for a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    /// OpenAI Responses API (streaming with `response.create`).
    OpenAiResponses,
    /// OpenAI Chat Completions API (`/v1/chat/completions`).
    OpenAiChatCompletions,
}

/// A single model entry within a provider's configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModelConfig {
    /// Model name (e.g. `"gpt-5.5"`).
    pub name: String,
    /// Task size classification for harness profile inference.
    pub task_size: ModelTaskSize,
    /// Context window size in tokens, if known.
    #[serde(default)]
    pub context_window_tokens: Option<u64>,
    /// Whether to force-include the tool prompt manifest for this model.
    #[serde(default)]
    pub tool_prompt_manifest: Option<bool>,
}

/// Task size classification used to infer the harness profile in `Auto` mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelTaskSize {
    /// Large-context model; uses the `medium` harness profile.
    Large,
    /// Small-context model; uses the `small` harness profile.
    Small,
}

/// A resolved model option shown in the model picker, combining a model name
/// with its provider metadata.
#[derive(Debug, Clone)]
pub struct ModelOption {
    /// Model name.
    pub name: String,
    /// Provider identifier.
    pub provider_id: String,
    /// Human-readable provider label.
    pub provider_label: String,
    /// Provider description.
    pub provider_description: String,
    /// Task size classification.
    pub task_size: ModelTaskSize,
    /// Context window size in tokens, if known.
    pub context_window_tokens: Option<u64>,
}

/// A native plugin library path with an enable toggle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Path to the `.so` or `.dylib` plugin library.
    pub path: PathBuf,
    /// Whether this plugin is loaded.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Skill discovery and activation settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillsConfig {
    /// Whether skill discovery is enabled.
    pub enabled: bool,
    /// Additional directories to scan for `SKILL.md` folders.
    pub dirs: Vec<PathBuf>,
    /// Skill names that are always active (not just discovered).
    pub active: Vec<String>,
}

/// MCP (Model Context Protocol) client configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    /// Whether MCP integration is enabled.
    pub enabled: bool,
    /// Configured MCP server entries.
    pub servers: Vec<McpServerConfig>,
}

/// Configuration for a single MCP stdio server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Unique server identifier.
    pub id: String,
    /// Command to launch the server (e.g. `"npx"`).
    pub command: String,
    /// Arguments passed to the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables for the server process.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Working directory for the server process.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Whether this server is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Optional prefix added to remote tool names to avoid collisions.
    #[serde(default)]
    pub tool_prefix: Option<String>,
    /// Request timeout in milliseconds.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Session memory settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// Whether to inject past session memory into new sessions.
    pub session_memory_enabled: bool,
    /// Maximum number of memory entries to inject.
    pub max_memory_entries: usize,
}

/// A fully resolved configuration with paths and merged config layers.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    /// The merged configuration.
    pub config: NaviConfig,
    /// Path to the global config file, if it existed.
    pub global_config_path: Option<PathBuf>,
    /// Path to the project config file, if it existed.
    pub project_config_path: Option<PathBuf>,
    /// NAVI data directory (sessions, logs, credentials).
    pub data_dir: PathBuf,
}
