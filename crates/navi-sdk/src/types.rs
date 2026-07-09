use navi_core::{AgentEvent, ContentPart, ContextPacket, LoadedConfig, ModelMessage, ToolExecutor};
use navi_plugin_host::LoadedPlugin;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

/// Parameters for creating a new NAVI agent session.
///
/// All fields are optional except those implied by the engine configuration.
/// Provide `project_dir` to override the default working directory and
/// `context_packets` to seed the session with external context.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviSessionRequest {
    #[serde(default)]
    pub project_dir: Option<PathBuf>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub context_packets: Vec<ContextPacket>,
    #[serde(default)]
    pub active_skills: Vec<String>,
    #[serde(default)]
    pub initial_messages: Vec<ModelMessage>,
    #[serde(default)]
    pub initial_events: Vec<AgentEvent>,
    #[serde(default)]
    pub initial_created_at: Option<u64>,
    #[serde(default)]
    pub initial_updated_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_goal: Option<navi_core::SessionGoal>,
}

/// Summary returned after a session is started.
///
/// Contains the session identifier, resolved project directory, and the
/// active model/provider pair at session creation time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviSessionInfo {
    pub id: String,
    pub project_dir: PathBuf,
    pub model: String,
    pub provider: String,
}

/// A user message to send to an active NAVI session.
///
/// `session_id` must match an existing session. `message` is the user text.
/// Optionally attach additional `context_packets` for this turn only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviTurnRequest {
    pub session_id: String,
    pub message: String,
    /// Optional multimodal content parts (images + text) for this turn.
    /// When non-empty, the engine creates a [`ModelMessage::user_multimodal`]
    /// instead of a plain text message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_parts: Vec<ContentPart>,
    #[serde(default)]
    pub context_packets: Vec<ContextPacket>,
    /// Optional thinking/reasoning configuration for this turn.
    /// When set, overrides the session-level thinking setting.
    /// When `None`, the session-level (frozen) config is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<navi_core::ThinkingConfig>,
}

/// The assistant's reply after a turn completes.
///
/// Contains the session id and the full response text produced by the model
/// (including any tool-use loop output that was synthesized into the final answer).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviTurnResponse {
    pub session_id: String,
    pub text: String,
}

/// A model available in the current configuration.
///
/// `id` uses the `provider:model` format (e.g. `openai:gpt-5.5`). `task_size`
/// indicates the recommended harness budget class. `context_window_tokens` is
/// the effective window after system-prompt overhead, if known.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviModelInfo {
    pub id: String,
    pub name: String,
    pub provider_id: String,
    pub provider_label: String,
    pub task_size: String,
    pub context_window_tokens: Option<u64>,
    /// Whether the model supports extended thinking / reasoning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_thinking: Option<bool>,
    /// Registry-supported reasoning effort levels for this model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_levels: Vec<String>,
    /// Default reasoning effort when the user has not picked one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<String>,
}

/// A discovered skill (SKILL.md) available for activation.
///
/// Skills are local prompt-injection folders discovered from configured paths.
/// `id` is the stable identifier; `description` comes from the skill frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviSkillInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub tags: Vec<String>,
    pub requires: Vec<String>,
}

/// Credential resolution status for a single provider.
///
/// Reports whether an API key was found, where it came from (environment
/// variable, credential store, or public access), and paths to fix missing keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviProviderCredentialStatus {
    pub provider_id: String,
    pub configured: bool,
    pub source: Option<String>,
    pub label: String,
    pub detail: Option<String>,
    pub env_var: String,
    pub credential_store_path: PathBuf,
}

/// Full account overview for a configured provider.
///
/// Combines provider metadata with its credential status and whether a key
/// is stored in the local credential store.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviProviderAccountInfo {
    pub provider_id: String,
    pub provider_label: String,
    pub env_var: String,
    pub has_stored_key: bool,
    pub status: NaviProviderCredentialStatus,
}

/// Normalized provider account usage and rate-limit windows.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviUsageReport {
    pub provider_id: String,
    pub provider_label: String,
    pub plan_type: Option<String>,
    pub limit_reached_kind: Option<String>,
    pub limits: Vec<NaviUsageLimitSnapshot>,
    /// Where account limits came from (e.g. `openai-oauth`, `openrouter`, `xai-oauth`, `session`).
    #[serde(default)]
    pub source: String,
    /// Human-readable note (auth type, missing remote API, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Optional free-form account metrics (spend, credits, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<NaviUsageDetail>,
}

/// A single labeled metric line for the Usage modal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviUsageDetail {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviUsageLimitSnapshot {
    pub limit_id: Option<String>,
    pub limit_name: Option<String>,
    pub metered_feature: Option<String>,
    pub limit_reached: bool,
    pub primary: Option<NaviUsageWindow>,
    pub secondary: Option<NaviUsageWindow>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviUsageWindow {
    pub used_percent: i32,
    pub limit_window_seconds: i32,
    pub reset_after_seconds: i32,
    pub reset_at: i32,
}

/// Where to persist a configuration change.
///
/// `Auto` prefers the project config when one exists, otherwise global.
/// `Project` and `Global` write explicitly. `None` skips persistence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NaviConfigSaveTarget {
    Auto,
    Project,
    Global,
    None,
}

/// A provider whose model list was successfully synced.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviSyncedProvider {
    pub provider_id: String,
    pub model_count: usize,
}

/// A provider whose model-list sync request failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviProviderSyncFailure {
    pub provider_id: String,
    pub message: String,
}

/// A provider that was skipped during model sync (typically due to a missing credential).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviProviderSyncSkipped {
    pub provider_id: String,
    pub reason: String,
}

/// Aggregate result of a model-list sync operation.
///
/// Contains the updated config, which providers were synced, which failed,
/// and which were skipped. If any models were updated and a save target was
/// specified, `saved_to` records the config file that was written.
#[derive(Debug, Clone)]
pub struct NaviProviderSyncReport {
    pub loaded_config: LoadedConfig,
    pub saved_to: Option<PathBuf>,
    pub updated: Vec<NaviSyncedProvider>,
    pub failed: Vec<NaviProviderSyncFailure>,
    pub skipped: Vec<NaviProviderSyncSkipped>,
}

/// Request to switch the active model for the engine or a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviModelSelectionRequest {
    pub provider_id: String,
    pub model: String,
    pub save_target: NaviConfigSaveTarget,
}

/// Result of switching the active model.
///
/// Reports the resolved config, whether the provider has credentials, the
/// effective context window, and the config file path if the change was persisted.
#[derive(Debug, Clone)]
pub struct NaviModelSelectionResult {
    pub loaded_config: LoadedConfig,
    pub saved_to: Option<PathBuf>,
    pub provider_id: String,
    pub model: String,
    pub context_window_tokens: Option<u64>,
    pub provider_configured: bool,
}

/// Metadata for a previously saved session, suitable for listing in a UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviSavedSessionInfo {
    pub id: String,
    pub title: Option<String>,
    pub project: PathBuf,
    pub created_at: u64,
    pub updated_at: u64,
}

/// Tool executor and plugin warnings assembled for a session.
///
/// This is an intermediate type produced during session setup. The `tool_executor`
/// is passed into the agent runtime; `warnings` reports any plugin load failures
/// that occurred.
pub struct NaviRuntimeTooling {
    pub tool_executor: Arc<ToolExecutor>,
    pub warnings: Vec<String>,
    pub agent_policies: Vec<String>,
    pub tui_components: Vec<String>,
    pub tui_panels: Vec<Box<dyn navi_plugin_api::TuiComponent>>,
    pub(crate) _plugins: Vec<LoadedPlugin>,
}

/// Structured error when a provider's API key cannot be resolved.
///
/// Implements `Display` and `Error` so it can be used with `anyhow` and
/// downcasted to extract the provider id, env var name, and credential store
/// path for targeted error handling in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviMissingCredentialError {
    pub provider_id: String,
    pub env_var: String,
    pub credential_store_path: PathBuf,
}

impl NaviMissingCredentialError {
    /// Returns a human-readable message describing the missing credential and
    /// how to resolve it.
    pub fn message(&self) -> String {
        format!(
            "missing API key for provider '{}'. Set {} or add a key to {}",
            self.provider_id,
            self.env_var,
            self.credential_store_path.display()
        )
    }
}

impl fmt::Display for NaviMissingCredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl Error for NaviMissingCredentialError {}

/// Errors returned by [`NaviEngine`](crate::NaviEngine) operations.
///
/// Provides typed variants for common failure modes while falling back
/// to [`Other`](Self::Other) for unexpected errors.
#[derive(Debug)]
pub enum NaviError {
    /// A required credential (API key) was not found.
    MissingCredential(NaviMissingCredentialError),
    /// The requested session was not found.
    SessionNotFound(String),
    /// A configuration error occurred.
    Config(String),
    /// A provider-level error occurred.
    Provider(String),
    /// An unexpected error occurred.
    Other(anyhow::Error),
}

impl fmt::Display for NaviError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCredential(e) => write!(f, "{e}"),
            Self::SessionNotFound(id) => write!(f, "session not found: {id}"),
            Self::Config(msg) => write!(f, "config error: {msg}"),
            Self::Provider(msg) => write!(f, "provider error: {msg}"),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

impl Error for NaviError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Other(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

impl From<anyhow::Error> for NaviError {
    fn from(e: anyhow::Error) -> Self {
        match e.downcast::<NaviError>() {
            Ok(error) => error,
            Err(e) => match e.downcast::<NaviMissingCredentialError>() {
                Ok(error) => Self::MissingCredential(error),
                Err(e) => Self::Other(e),
            },
        }
    }
}

impl From<NaviMissingCredentialError> for NaviError {
    fn from(e: NaviMissingCredentialError) -> Self {
        Self::MissingCredential(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anyhow_missing_credential_downcasts_to_typed_sdk_error() {
        let missing = NaviMissingCredentialError {
            provider_id: "test-provider".to_string(),
            env_var: "TEST_API_KEY".to_string(),
            credential_store_path: PathBuf::from("/tmp/credentials.toml"),
        };

        let error = NaviError::from(anyhow::Error::new(missing));

        match error {
            NaviError::MissingCredential(error) => {
                assert_eq!(error.provider_id, "test-provider");
                assert_eq!(error.env_var, "TEST_API_KEY");
            }
            other => panic!("expected MissingCredential, got {other:?}"),
        }
    }

    #[test]
    fn anyhow_sdk_error_downcasts_to_original_sdk_error() {
        let error = NaviError::from(anyhow::Error::new(NaviError::SessionNotFound(
            "session-1".to_string(),
        )));

        match error {
            NaviError::SessionNotFound(id) => assert_eq!(id, "session-1"),
            other => panic!("expected SessionNotFound, got {other:?}"),
        }
    }
}
