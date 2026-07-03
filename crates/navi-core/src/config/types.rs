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
    /// WASM plugin directory paths.
    pub wasm_plugins: Vec<WasmPluginConfig>,
    /// Plugin marketplace registry (catalog repository).
    #[serde(default)]
    pub plugin_marketplace: PluginMarketplaceConfig,
    /// Provider registry update settings.
    #[serde(default)]
    pub registry: RegistryConfig,
    /// Terminal UI preferences.
    #[serde(default)]
    pub tui: TuiConfig,
    /// Background model routing configuration.
    #[serde(default)]
    pub background_models: BackgroundModelsConfig,
}

/// TUI-specific settings (global config).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TuiConfig {
    /// Color theme id: `default`, `lain`, `terminal`, `slate`, `ember`, `paper`, `oscura-night`.
    pub theme: String,
    /// Whether assistant thinking text is shown in the chat view.
    #[serde(default = "default_true")]
    pub show_thinking: bool,
    /// Whether tool rows show full input/output instead of compact lines.
    pub full_tool_view: bool,
    /// Number of most-recent tool rows shown in compact tool groups.
    pub compact_tool_visible_limit: usize,
    /// Thinking effort: `max`, `high`, `medium`, `low`, `off`.
    pub thinking_level: String,
    /// Auto-approve tools without confirmation (YOLO mode).
    pub yolo_mode: bool,
    /// Most-recently used provider ids, ordered newest first.
    /// Capped (see `navi-tui::providers::push_recent_provider`).
    pub recent_provider_ids: Vec<String>,
    /// Most-recently used model keys in `provider:model` form, ordered newest first.
    /// Capped (see `navi-tui::providers::push_recent_model`).
    pub recent_model_ids: Vec<String>,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: "default".to_string(),
            show_thinking: true,
            full_tool_view: false,
            compact_tool_visible_limit: 5,
            thinking_level: "adaptive".to_string(),
            yolo_mode: false,
            recent_provider_ids: Vec::new(),
            recent_model_ids: Vec::new(),
        }
    }
}

/// Plugin marketplace / registry repository settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginMarketplaceConfig {
    /// URL to `catalog.json` in the registry repository (`https://` or `file://`).
    pub registry_url: Option<String>,
}

/// Provider registry update settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RegistryConfig {
    /// Whether remote registry update checks are enabled.
    pub update_enabled: bool,
    /// Minimum interval between registry update checks, in hours.
    pub check_interval_hours: u64,
    /// Random jitter added to the interval, in hours.
    pub check_jitter_hours: u64,
    /// HTTP request timeout for registry update checks, in seconds.
    pub request_timeout_seconds: u64,
    /// Max retries for failed registry update requests.
    pub max_retries: u32,
    /// Update mode: `background` or `foreground`.
    pub update_mode: String,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            update_enabled: true,
            check_interval_hours: 24,
            check_jitter_hours: 6,
            request_timeout_seconds: 5,
            max_retries: 1,
            update_mode: "background".to_string(),
        }
    }
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
    /// Legacy max model/tool loop iterations for the `small` profile.
    /// Retained for config compatibility; hard turn-loop caps are not enforced.
    pub max_turn_loops_small: usize,
    /// Legacy max model/tool loop iterations for the `medium` profile.
    /// Retained for config compatibility; hard turn-loop caps are not enforced.
    pub max_turn_loops_medium: usize,
    /// Legacy max model/tool loop iterations for the `long-running` profile.
    /// Retained for config compatibility; hard turn-loop caps are not enforced.
    pub max_turn_loops_long_running: usize,
    /// Legacy global override for max turn loop iterations.
    /// Retained for config compatibility; hard turn-loop caps are not enforced.
    pub turn_loop_limit: Option<usize>,
    /// Max total tool calls in one turn for the `small` profile.
    pub max_tool_calls_small: usize,
    /// Max total tool calls in one turn for the `medium` profile.
    pub max_tool_calls_medium: usize,
    /// Max tool calls executed in parallel for the `small` profile.
    pub max_parallel_tool_calls_small: usize,
    /// Max tool calls executed in parallel for the `medium` profile.
    pub max_parallel_tool_calls_medium: usize,
    /// Max tool calls executed in parallel for the `long-running` profile.
    pub max_parallel_tool_calls_long_running: usize,
    /// Max consecutive tool failures before stopping a turn.
    pub max_consecutive_tool_errors: usize,
    /// Max consecutive schema-invalid tool calls before stopping a turn.
    pub max_consecutive_invalid_arguments: usize,
    /// Max consecutive malformed-JSON tool calls before stopping a turn.
    pub max_consecutive_malformed_arguments: usize,
    /// Max consecutive unknown-tool calls before stopping a turn.
    pub max_consecutive_unknown_tools: usize,
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
    /// Fraction of recent turns to keep intact during autocompact (0.0–1.0).
    /// Default 0.25 keeps the most recent 25% of turns unsummarized.
    pub autocompact_keep_ratio: f64,
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
    /// One-feature-at-a-time workflow with persistent sprint artifacts.
    LongRunning,
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
    /// Paths that are always denied for reads. Supports glob patterns and
    /// directory prefixes. Lines referencing denied paths in grep/fs_browser
    /// output are filtered before entering context.
    pub deny_paths: Vec<String>,
    /// MCP server allowlist. When non-empty, only MCP servers whose `id`
    /// appears in this list may be loaded. Empty means all servers are allowed.
    #[serde(default)]
    pub allowed_mcp_servers: Vec<String>,
}

impl SecurityConfig {
    /// Returns `true` if the given MCP server id is allowed by the allowlist.
    /// An empty allowlist means all servers are allowed.
    pub fn is_mcp_server_allowed(&self, server_id: &str) -> bool {
        if self.allowed_mcp_servers.is_empty() {
            return true;
        }
        self.allowed_mcp_servers.iter().any(|id| id == server_id)
    }
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
    /// How NAVI should expose tools to this provider.
    #[serde(default)]
    pub tool_calling_mode: Option<ToolCallingMode>,
    /// Provider-specific request fields that are not universally supported by
    /// OpenAI-compatible APIs. `None` means "not specified" so the catalog
    /// can fill in the canonical defaults; `Some(opts)` honors the user's
    /// explicit configuration even when the value is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_options: Option<ProviderRequestOptions>,
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
            tool_calling_mode: None,
            request_options: None,
        }
    }
}

/// Tool calling compatibility mode for a provider or model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolCallingMode {
    /// Send native tool definitions and expect provider-native tool calls.
    Native,
    /// Include a textual manifest and extract tool calls from model text.
    TextExtracted,
    /// Include a textual manifest but do not send native tool definitions.
    ManifestOnly,
    /// Do not expose NAVI tools to the provider.
    Disabled,
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

/// Optional request features that individual providers may opt into.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderRequestOptions {
    /// OpenAI `prompt_cache_key` routing hint. Omit to disable for
    /// OpenAI-compatible providers that reject this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    /// OpenAI `prompt_cache_retention` value, for example `"in_memory"` or `"24h"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_retention: Option<String>,
    /// Anthropic `cache_control` object, for example `{ "type": "ephemeral" }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_cache_control: Option<serde_json::Value>,
}

impl ProviderRequestOptions {
    pub fn is_empty(&self) -> bool {
        self.prompt_cache_key.is_none()
            && self.prompt_cache_retention.is_none()
            && self.anthropic_cache_control.is_none()
    }
}

/// API protocol kind for a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    #[serde(rename = "openai-responses", alias = "open-ai-responses")]
    OpenAiResponses,
    #[serde(rename = "openai-chat-completions", alias = "open-ai-chat-completions")]
    OpenAiChatCompletions,
    AnthropicMessages,
    GeminiGenerateContent,
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
    /// Maximum tokens the model can generate in a single response.
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    /// Recommended temperature for the model (0.0–2.0).
    #[serde(default)]
    pub recommended_temperature: Option<f64>,
    /// Whether the model supports extended thinking / reasoning mode.
    #[serde(default)]
    pub supports_thinking: Option<bool>,
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

/// A WASM plugin directory path with an enable toggle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmPluginConfig {
    /// Path to the WASM plugin directory (containing plugin.toml and .wasm binary).
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
    #[serde(default)]
    pub command: Option<String>,
    /// URL to connect to the server (e.g. `"http://localhost:8080/sse"`).
    #[serde(default)]
    pub url: Option<String>,
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

fn default_memory_enabled() -> bool {
    true
}
fn default_memory_root() -> String {
    "memory/projects".to_string()
}
fn default_global_memory_path() -> String {
    "~/.code-agent/global-memory.md".to_string()
}
fn default_checkpoint_thresholds() -> Vec<f64> {
    vec![0.20, 0.45, 0.70]
}
fn default_rebuild_threshold() -> f64 {
    0.85
}
fn default_injected_context_token_budget() -> usize {
    65000
}
fn default_dream_interval_days() -> u64 {
    7
}
fn default_distill_interval_days() -> u64 {
    30
}

/// SQLite history store settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    #[serde(default = "default_memory_enabled")]
    pub enabled: bool,
    #[serde(default = "default_history_sqlite_path")]
    pub sqlite_path: String,
}

fn default_history_sqlite_path() -> String {
    "history.sqlite".to_string()
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sqlite_path: default_history_sqlite_path(),
        }
    }
}

/// Session memory settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// Whether to inject past session memory into new sessions.
    pub session_memory_enabled: bool,
    /// Maximum number of memory entries to inject.
    pub max_memory_entries: usize,
    /// Whether long-horizon memory is enabled.
    #[serde(default = "default_memory_enabled")]
    pub enabled: bool,
    /// Root directory for memory files.
    #[serde(default = "default_memory_root")]
    pub root: String,
    /// Path to the user-level global memory file.
    #[serde(default = "default_global_memory_path")]
    pub global_memory_path: String,
    /// Context utilization thresholds that trigger checkpoints.
    #[serde(default = "default_checkpoint_thresholds")]
    pub checkpoint_thresholds: Vec<f64>,
    /// Context utilization threshold that triggers a rebuild.
    #[serde(default = "default_rebuild_threshold")]
    pub rebuild_threshold: f64,
    /// Maximum token budget for the injected rebuild context.
    #[serde(default = "default_injected_context_token_budget")]
    pub injected_context_token_budget: usize,
    /// Interval in days for the dream/compaction maintenance job.
    #[serde(default = "default_dream_interval_days")]
    pub dream_interval_days: u64,
    /// Interval in days for the distill/SOPS maintenance job.
    #[serde(default = "default_distill_interval_days")]
    pub distill_interval_days: u64,
    /// History database configuration.
    #[serde(default)]
    pub history: HistoryConfig,
}

/// Background model configuration — maps task types to model profiles or explicit overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BackgroundModelsConfig {
    /// Default background model entry (used when no task-specific entry is set).
    pub default: Option<BackgroundModelEntry>,
    /// Model for session title generation.
    pub naming: Option<BackgroundModelEntry>,
    /// Model for repository exploration subagent.
    pub repo_search: Option<BackgroundModelEntry>,
    /// Model for conversation compaction/summarization.
    pub compaction: Option<BackgroundModelEntry>,
    /// Model for research-oriented subagents.
    pub subagent_research: Option<BackgroundModelEntry>,
    /// Model for simple code edit subagents.
    pub simple_code_edit: Option<BackgroundModelEntry>,
}

impl BackgroundModelsConfig {
    /// Resolves the entry for a given task key, falling back to `default`.
    pub fn resolve(&self, task: &str) -> Option<&BackgroundModelEntry> {
        let entry = match task {
            "naming" => self.naming.as_ref(),
            "repo_search" => self.repo_search.as_ref(),
            "compaction" => self.compaction.as_ref(),
            "subagent_research" => self.subagent_research.as_ref(),
            "simple_code_edit" => self.simple_code_edit.as_ref(),
            _ => None,
        };
        entry.or(self.default.as_ref())
    }
}

/// A single background model entry: either a profile name or an explicit provider+model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackgroundModelEntry {
    /// Profile identifier (e.g. "cheap_general", "naming").
    pub profile: Option<String>,
    /// Explicit provider override (used when profile is None).
    pub provider: Option<String>,
    /// Explicit model override (used when profile is None).
    pub model: Option<String>,
    /// Fallback strategy: "main_model" or an explicit "provider:model".
    pub fallback: Option<String>,
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

impl Default for LoadedConfig {
    fn default() -> Self {
        let data_dir = directories::ProjectDirs::from("dev", "navi", "navi")
            .map(|dirs| dirs.data_local_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(""));
        Self {
            config: NaviConfig::default(),
            global_config_path: None,
            project_config_path: None,
            data_dir,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_serde_roundtrip() {
        let variants = [
            (ProviderKind::OpenAiResponses, "openai-responses"),
            (
                ProviderKind::OpenAiChatCompletions,
                "openai-chat-completions",
            ),
            (ProviderKind::AnthropicMessages, "anthropic-messages"),
            (
                ProviderKind::GeminiGenerateContent,
                "gemini-generate-content",
            ),
        ];
        for (kind, expected_str) in variants {
            let serialized = serde_json::to_string(&kind).unwrap();
            assert_eq!(serialized, format!("\"{}\"", expected_str));
            let deserialized: ProviderKind = serde_json::from_str(&serialized).unwrap();
            assert_eq!(deserialized, kind);
        }
    }

    #[test]
    fn provider_kind_from_toml_string() {
        let toml_str = "kind = \"anthropic-messages\"";
        #[derive(Deserialize)]
        struct Wrapper {
            kind: ProviderKind,
        }
        let wrapper: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(wrapper.kind, ProviderKind::AnthropicMessages);
    }
}
