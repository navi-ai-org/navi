mod attachment_tool;
mod auth_ops;
mod credentials;
mod engine;
mod engine_api;
mod engine_driver;
mod host_tool;
mod mcp_ops;
mod memory_ops;
mod notify_ops;
mod plugins;
mod routing_ops;
mod tooling;
mod tui_extensions;
mod types;
mod voice;

pub use credentials::{
    CommandCodeUsageData, CredentialAccountInfo, CredentialSource, CredentialStatus,
    DeviceOAuthStarted, commandcode_remote_models, commandcode_usage_data, provider_api_key,
    provider_credential_accounts, provider_credential_status, provider_supports_device_oauth,
    start_provider_device_oauth,
};
pub use engine::{NaviEngine, NaviEngineBuilder, NaviSession};
pub use engine_api::{NAVI_ENGINE_API_METHODS, NAVI_NAPI_BOUND_METHODS};
pub use engine_driver::EngineDriver;
pub use host_tool::{
    HostToolDefinition, HostToolHandler, HostToolInvocation, SdkHostTool, SdkHostToolResult,
};
/// Deprecated: use [`start_provider_device_oauth`] instead.
pub use navi_providers::github_copilot_device_oauth;
pub use navi_providers::{
    format_hypercredits, hyper_base_url, hypercredits_to_usd, peek_hypercredit_balance,
    set_hypercredit_balance, set_hypercredit_balance_authoritative, take_hypercredit_balance,
    usd_to_hypercredits,
};
pub use tooling::reload_wasm_plugins_on_executor;
pub use tooling::{build_provider_for_config, build_provider_for_project_config};
pub use types::{
    NaviConfigSaveTarget, NaviEffortOption, NaviError, NaviMissingCredentialError, NaviModelInfo,
    NaviModelSelectionRequest, NaviModelSelectionResult, NaviProviderAccountInfo,
    NaviProviderCredentialStatus, NaviProviderSyncFailure, NaviProviderSyncReport,
    NaviProviderSyncSkipped, NaviRuntimeTooling, NaviSavedSessionInfo, NaviSessionInfo,
    NaviSessionRequest, NaviSkillInfo, NaviSyncedProvider, NaviTurnRequest, NaviTurnResponse,
    NaviUsageDetail, NaviUsageLimitSnapshot, NaviUsageReport, NaviUsageWindow,
    effort_options_for_model,
};

// Re-export engine types so TUI/clients can import from navi_sdk instead of navi_core.
pub use navi_core::ProviderId;
pub use navi_core::{AgentMode, AttachmentKind, ContentPart, ProposedPlan, ProposedPlanParser};
// Session utilities
pub use navi_core::session::{
    clean_session_title, current_unix_timestamp, session_title_from_events,
};
// Event/session types
pub use navi_core::{
    AgentEvent, AgentRunState, Capability, CapabilityDecision, CapabilityGrant, CapabilityLedger,
    CapabilityLedgerEntry, CapabilityScope, GoalStatus, GoalTask, ModelMessage, ModelRole,
    PlanReviewDecision, PlanReviewRequest, PlanReviewResponse, QuestionOption, QuestionRequest,
    QuestionResponse, RuntimeEvent, RuntimeEventKind, SessionGoal, SessionId, SessionSnapshot,
    SessionSnapshotInfo, SessionUsageSnapshot, SubagentTranscriptItem, SubagentTranscriptKind,
    SudoPasswordRequest, SudoPasswordResponse, TaskStatus,
};
// Event auxiliary types
pub use navi_core::event::RepetitionWarningKind;
// Tool/approval types
pub use navi_core::{
    ApprovalDecision, ApprovalRequest, ApprovalRisk, BackgroundCommandSnapshot,
    BackgroundTaskStatus, PatchProposal, ToolInvocation, ToolResult,
};
// Config/provider types
pub use navi_core::{
    AttachmentModelsConfig, BINARY_REASONING_LEVELS, BackgroundModelEntry, BackgroundModelsConfig,
    CompactState, CompactThreshold, CredentialStore, HarnessPolicy, HarnessProfile, LoadedConfig,
    ModelOption, ModelTaskSize, NaviConfig, NotificationUrgency, NotifyRequest, PermissionMode,
    ProviderConfig, ProviderKind, ProviderModelConfig, SessionStore, ThinkingConfig, UpdateInfo,
    UpdatesConfig, effort_display_label, is_binary_effort_model, parse_reasoning_level,
    resolve_effort_label, resolve_model_thinking_level, select_harness_policy,
    thinking_levels_for_model,
};
// Utility functions
pub use navi_core::{
    available_model_options, billable_input_split, build_system_prompt, canonical_provider_id,
    compact_tool_observation, effective_context_window, estimate_token_cost_usd,
    estimate_token_cost_usd_with_cache, is_free_model_name, log_path, model_cache_list_pricing,
    model_can_run_publicly, model_list_pricing, provider_catalog, provider_credit_unit,
    provider_request_model_name, provider_uses_credits, resolve_provider_api_key_for_project,
    resolve_provider_config, resolve_provider_credential_status, save_global_config,
    set_registry_store, usd_to_provider_credits,
};
// Registry
pub use navi_core::registry::{RegistryFetcher, RegistryStore, load_registry};
pub use navi_mcp::{LoadedMcpServers, McpServerInfo, load_configured_mcp_servers};

// Auto-memory
pub use navi_core::memory::{
    AutoMemoryStore, MemoryEntry, MemoryStatus, MemorySummary, MemoryType,
};

// Local voice / dictation (navi-voice)
pub use navi_voice::{
    AsrEngineId, CHUNK_SAMPLES, DoctorReport, SAMPLE_RATE, TranscribeResult, VoiceEvent,
    VoiceInstallOptions, VoiceRecorderInfo, VoiceStatus,
};

// Extended memory ops DTOs
pub use memory_ops::{MemoryDoctorReport, MemoryDreamReport, MemoryInitReport, MemoryStatusReport};

// MCP config management
pub use mcp_ops::McpConfigSnapshot;
pub use navi_core::{McpConfig, McpServerConfig};

// Voice config updates
pub use voice::VoiceConfigUpdate;

// Plugin lifecycle DTOs
pub use plugins::{
    KindSideEffectOptions, PluginInfo, PluginInstallResult, PluginMarketplaceEntry,
    apply_kind_side_effects_at, apply_kind_side_effects_with_options, detect_package_kind,
    merge_mcp_from_package, package_has_mcp_json,
};
pub use tui_extensions::{
    InstalledTuiExtension, TuiExtensionCommand, TuiExtensionPanel, TuiExtensionSpec,
    list_installed_tui_extensions, load_tui_extension_from_dir, parse_tui_extension_spec,
};

// OAuth / registry DTOs
pub use auth_ops::{DeviceOAuthStartedInfo, RegistryProviderSummary};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tooling::build_provider_for_config;
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

        let error = match build_provider_for_config(&loaded_config) {
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
