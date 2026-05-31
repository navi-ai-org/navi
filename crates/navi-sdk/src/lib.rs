mod engine;
mod host_tool;
mod tooling;
mod types;

pub use engine::{NaviEngine, NaviEngineBuilder, NaviSession};
pub use host_tool::{HostToolDefinition, HostToolHandler, SdkHostTool};
// Re-exported for navi-tui credential flows. Ideally this should be behind a
// generic provider credential API rather than exposing a single provider's OAuth.
pub use navi_providers::{DeviceOAuthStarted, github_copilot_device_oauth};
pub use types::{
    NaviConfigSaveTarget, NaviError, NaviMissingCredentialError, NaviModelInfo,
    NaviModelSelectionRequest, NaviModelSelectionResult, NaviProviderAccountInfo,
    NaviProviderCredentialStatus, NaviProviderSyncFailure, NaviProviderSyncReport,
    NaviProviderSyncSkipped, NaviRuntimeTooling, NaviSavedSessionInfo, NaviSessionInfo,
    NaviSessionRequest, NaviSkillInfo, NaviSyncedProvider, NaviTurnRequest, NaviTurnResponse,
};

// Re-export engine types so TUI/clients can import from navi_sdk instead of navi_core.
pub use navi_core::ProviderId;
// Session utilities
pub use navi_core::session::{
    clean_session_title, current_unix_timestamp, session_title_from_events,
};
// Event/session types
pub use navi_core::{
    AgentEvent, AgentMode, AgentRunState, ModelMessage, ModelRole, RuntimeEvent, RuntimeEventKind,
    SessionId, SessionSnapshot,
};
// Tool/approval types
pub use navi_core::{ApprovalDecision, ApprovalRequest, ToolInvocation, ToolResult};
// Config/provider types
pub use navi_core::{
    CompactState, CompactThreshold, CredentialStore, HarnessPolicy, HarnessProfile, LoadedConfig,
    ModelOption, ModelTaskSize, NaviConfig, ProviderConfig, ProviderKind, ProviderModelConfig,
    SessionStore, ThinkingConfig, select_harness_policy,
};
// Utility functions
pub use navi_core::{
    available_model_options, build_system_prompt, canonical_provider_id, compact_tool_observation,
    effective_context_window, is_free_model_name, log_path, model_can_run_publicly,
    provider_catalog, provider_request_model_name, resolve_provider_config,
    resolve_provider_credential_status, save_global_config,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tooling::build_model_provider;
    use navi_core::config::ModelConfig;
    use navi_core::{NaviConfig, ProviderConfig, ProviderKind};

    #[test]
    fn missing_credential_error_is_structured_and_downcastable() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let loaded_config = LoadedConfig {
            config: NaviConfig {
                model: ModelConfig {
                    provider: "test-provider".to_string(),
                    name: "test-model".to_string(),
                },
                providers: vec![ProviderConfig {
                    id: "test-provider".to_string(),
                    label: "Test Provider".to_string(),
                    description: String::new(),
                    kind: ProviderKind::OpenAiResponses,
                    api_key_env: "NAVI_TEST_MISSING_CREDENTIAL_KEY_98770".to_string(),
                    base_url: Some("https://example.test/v1".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            global_config_path: None,
            project_config_path: None,
            data_dir: tempdir.path().to_path_buf(),
        };

        let error = match build_model_provider(&loaded_config) {
            Ok(_) => panic!("expected missing credential"),
            Err(error) => error,
        };
        let missing = error
            .downcast_ref::<NaviMissingCredentialError>()
            .expect("typed missing credential error");

        assert_eq!(missing.provider_id, "test-provider");
        assert_eq!(missing.env_var, "NAVI_TEST_MISSING_CREDENTIAL_KEY_98770");
        assert_eq!(
            missing.credential_store_path,
            tempdir.path().join("credentials.toml")
        );
        assert!(missing.message().contains("test-provider"));
        assert!(!missing.message().contains("sk-"));
    }
}
