pub mod cancel;
pub mod compact;
pub mod config;
pub mod context;
pub mod credentials;
pub mod effect;
pub mod event;
pub mod file_lock;
mod fs_util;
pub mod harness;
pub mod logging;
pub mod memory;
pub mod model;
pub mod patch;
pub mod prompt;
mod provider_id;
pub mod registry;
pub mod repetition;
pub mod runtime;
pub mod sandbox;
pub mod security;
pub mod session;
pub mod setup;
pub mod skills;
pub mod tool;
pub mod trace;
pub mod turn;
pub mod verifier;

pub mod background_model;

pub use compact::{CompactState, CompactThreshold};
pub use config::{
    BackgroundModelEntry, BackgroundModelsConfig, HarnessProfile, LoadedConfig, McpConfig,
    McpServerConfig, ModelOption, ModelTaskSize, NaviConfig, PluginConfig, ProviderConfig,
    ProviderKind, ProviderModelConfig, ProviderRequestOptions, SecurityConfig, ToolCallingMode,
    ToolPromptManifest, WasmPluginConfig, available_model_options, canonical_provider_id,
    default_request_options_for, effective_context_window, effective_tool_calling_mode,
    is_free_model_name, model_can_run_publicly, provider_catalog, provider_request_model_name,
    resolve_provider_config, save_global_config, save_project_config, set_registry_store,
};
pub use context::{ContextPacket, ContextSource};
pub use credentials::{
    CommandCodeCredentialMetadata, CredentialAccountInfo, CredentialSource, CredentialStatus,
    CredentialStore, resolve_provider_api_key, resolve_provider_api_key_for_project,
    resolve_provider_credential_status,
};
pub use effect::{BlastRadius, EffectAnalyzer, EffectReport, PostDecision};
pub use event::{
    AgentEvent, ApprovalDecision, ApprovalRequest, ApprovalRisk, QuestionOption, QuestionRequest,
    QuestionResponse, RuntimeEvent, RuntimeEventKind, SubagentTranscriptItem,
    SubagentTranscriptKind,
};
pub use file_lock::{FileLockInfo, FileLockManager, LockGuard};
pub use harness::{
    AgentRunState, HarnessPolicy, build_system_prompt, build_system_prompt_with_memory,
    compact_tool_observation, record_tool_call, select_harness_policy, tool_error_result,
};
pub use logging::{LoggingGuard, LoggingRuntimeConfig, init_logging, log_dir, log_path};
pub use model::{
    ContentPart, ModelMessage, ModelProvider, ModelRequest, ModelResponse, ModelRole, ModelStream,
    ModelStreamEvent, ThinkingConfig, ThinkingRequest,
};
pub use patch::PatchProposal;
pub use prompt::{PromptCache, SystemPromptInput, SystemPromptRenderer};
pub use provider_id::ProviderId;
pub use runtime::{
    AgentRuntime, AgentRuntimeOptions, ApprovalResolver, QuestionResolver, TurnCanceller,
};
pub use security::{SecurityDecision, SecurityPolicy};
pub use session::{
    SessionId, SessionRuntime, SessionSnapshot, SessionStore, clean_session_title,
    session_title_from_events,
};
pub use setup::{SETUP_INTERVIEW_COMPLETE_MARKER, SETUP_INTERVIEW_PROMPT};
pub use skills::{SkillManifest, active_skills, discover_configured_skills};
pub use tool::background::{BackgroundCommandSnapshot, BackgroundTaskStatus};
pub use tool::registry::{ToolRegistry, ToolSet, phases};
pub use tool::{
    AgentProfile, ApprovalMode, ProviderBuilderFn, RepoExploreTool, SubagentTool, Tool,
    ToolDefinition, ToolExecutor, ToolExposure, ToolInvocation, ToolKind, ToolMetadata, ToolResult,
    ToolRisk, capabilities,
};

pub use background_model::{BackgroundModelResolver, ResolvedBackgroundModel};
pub use memory::{HistoryStore, MemoryManager, MemoryStore, SessionCheckpoint};
pub use verifier::{VerificationStore, VerifierResult, VerifierRunner, VerifierSpec};
