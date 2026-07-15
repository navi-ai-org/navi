pub mod benchmark;
pub mod branch_race;
pub mod cancel;
pub mod capability;
pub mod compact;
pub mod config;
pub mod context;
pub mod credentials;
pub mod dataset;
pub mod effect;
pub mod eval;
pub mod event;
mod fs_util;
pub mod goal;
pub mod harness;
pub mod logging;
pub mod mcp_firewall;
pub mod memory;
pub mod model;
pub mod model_router;
pub mod notify;
pub mod operational_memory;
pub mod patch;
pub mod plan_mode;
pub mod plan_store;
pub mod prompt;
mod provider_id;
pub mod recap;
pub mod registry;
pub mod repetition;
pub mod replay_gate;
pub mod repo_intelligence;
pub mod runtime;
pub mod runtime_components;
pub mod sandbox;
pub mod security;
pub mod session;
pub mod session_title;
pub mod setup;
pub mod skill_mining;
pub mod skills;
pub mod tool;
pub mod trace;
pub mod transcription_catalog;
pub mod turn;
pub mod update;
pub mod verifier;

pub mod background_model;

pub use branch_race::{
    BranchCandidate, BranchHypothesis, BranchRacePlanner, BranchRaceReport, BranchRaceRequest,
    BranchStrategy, ScoredBranchCandidate,
};
pub use capability::{
    Capability, CapabilityDecision, CapabilityGrant, CapabilityLedger, CapabilityLedgerEntry,
    CapabilityScope, capabilities_from_tool_metadata,
};
pub use compact::{AUTO_COMPACT_THRESHOLD_PERCENT, CompactState, CompactThreshold};
pub use config::{
    AttachmentModelsConfig, BackgroundModelEntry, BackgroundModelsConfig, GoalsConfig,
    HarnessProfile, LoadedConfig, McpConfig, McpServerConfig, ModelOption, ModelTaskSize,
    NaviConfig, PermissionMode, PluginConfig, ProviderConfig, ProviderKind, ProviderModelConfig,
    ProviderRequestOptions, SecurityConfig, ToolCallingMode, ToolPromptManifest, UpdatesConfig,
    VoiceConfig, WasmPluginConfig, available_model_options, billable_input_split,
    canonical_provider_id, default_request_options_for, effective_context_window,
    effective_tool_calling_mode, estimate_token_cost_usd, estimate_token_cost_usd_with_cache,
    is_free_model_name, model_cache_list_pricing, model_can_run_publicly, model_list_pricing,
    model_supports_attachment, provider_catalog, provider_credit_unit, provider_request_model_name,
    provider_uses_credits, resolve_provider_config, save_global_config, save_project_config,
    set_registry_store, usd_to_provider_credits,
};
pub use context::{ContextPacket, ContextSource};
pub use credentials::{
    CommandCodeCredentialMetadata, CredentialAccountInfo, CredentialSource, CredentialStatus,
    CredentialStore, XAI_GROK_CLI_OAUTH_KIND, is_model_usable_oauth_kind, resolve_provider_api_key,
    resolve_provider_api_key_for_project, resolve_provider_credential_status,
};
pub use dataset::{
    DatasetRow, DatasetRowType, export_jsonl, trace_to_dataset_rows, traces_to_eval_candidates,
};
pub use effect::{BlastRadius, EffectAnalyzer, EffectReport, PostDecision};
pub use eval::{
    EvalCase, EvalCaseMetrics, EvalCaseResult, EvalMode, EvalRun, EvalRunMetrics, EvalRunner,
    EvalSuite, eval_case_from_trace,
};
pub use event::{
    AgentEvent, ApprovalDecision, ApprovalRequest, ApprovalRisk, PlanReviewDecision,
    PlanReviewRequest, PlanReviewResponse, QuestionOption, QuestionRequest, QuestionResponse,
    RuntimeEvent, RuntimeEventKind, SubagentTranscriptItem, SubagentTranscriptKind,
    SudoPasswordRequest, SudoPasswordResponse,
};
pub use goal::{
    CreateGoalTool, GetGoalTool, GoalExtension, GoalId, GoalRuntimeHandle, GoalService, GoalStatus,
    GoalTask, SessionGoal, TaskStatus, UpdateGoalChecklistTool, UpdateGoalTool,
    goal_tool_definitions,
};
pub use harness::{
    AgentRunState, HarnessPolicy, build_system_prompt, build_system_prompt_with_memory,
    compact_tool_observation, record_tool_call, select_harness_policy, tool_error_result,
};
pub use logging::{LoggingGuard, LoggingRuntimeConfig, init_logging, log_dir, log_path};
pub use mcp_firewall::{McpFirewallDecision, McpFirewallPolicy, McpProvenance, McpTaint};
pub use model::{
    AttachmentKind, BINARY_REASONING_LEVELS, ContentPart, DEFAULT_REASONING_LEVELS, ModelMessage,
    ModelProvider, ModelRequest, ModelResponse, ModelRole, ModelStream, ModelStreamEvent,
    ThinkingConfig, ThinkingRequest, effort_display_label, is_binary_effort_model,
    parse_reasoning_level, resolve_effort_label, resolve_model_thinking_level,
    thinking_levels_for_model,
};
pub use model_router::{ModelRoute, ModelRouteRole, ModelRouter, ModelScorecard};
pub use notify::{NotificationUrgency, NotifyRequest, notify_desktop, open_url};
pub use operational_memory::{MemoryScope, OperationalMemoryEntry, OperationalMemoryStore};
pub use patch::PatchProposal;
pub use plan_mode::{AgentMode, ProposedPlan, ProposedPlanParser, is_tool_allowed_in_plan_mode};
pub use plan_store::{
    Plan, PlanLineComment, PlanStatus, PlanStep, PlanStore, format_plan_feedback, plan_view_lines,
};
pub use prompt::{PromptCache, RenderedPrompt, SystemPromptInput, SystemPromptRenderer};
pub use provider_id::ProviderId;
pub use recap::{
    RECAP_LONG_TAIL_CHARS, RECAP_MAX_DISPLAY_CHARS, llm_recap, local_recap, local_recap_with_tools,
    should_suppress_recap,
};
pub use replay_gate::{
    ReplayGateConfig, ReplayGateReport, SuperiorityGateReport, evaluate_replay_gate,
    evaluate_superiority_gate, unsafe_guarded_auto_approval_count,
};
pub use repo_intelligence::{
    ChurnRecord, DependencyEdge, ImportRecord, IndexedFile, RankedSymbolRecord, ReferenceRecord,
    RepoIndex, RepoIntelligenceCache, SymbolRecord, TestTarget, TextMatchRecord, build_index,
    dependency_edges, discover_tests, goto_symbol, ranked_symbol_matches, references,
    search_symbols, search_text_matches,
};
pub use runtime::{
    AgentRuntime, AgentRuntimeOptions, ApprovalResolver, MemoryExtractionModel, PlanReviewResolver,
    QuestionResolver, SudoPasswordResolver, TurnCanceller,
};
pub use runtime_components::{
    CompactionStrategy, DefaultCompactionStrategy, DefaultHarnessDriver, DefaultPromptBuilder,
    DefaultToolSecurityPolicy, HarnessDriver, NoopSessionHooks, PermissiveSecurityPolicy,
    PromptBuilder, RuntimeComponents, SessionHooks, ToolSecurityPolicy,
};
pub use security::{SecurityDecision, SecurityPolicy};
pub use session::{
    SessionId, SessionRuntime, SessionSnapshot, SessionSnapshotInfo, SessionStore,
    SessionUsageSnapshot, clean_session_title, session_title_from_events,
};
pub use session_title::{SessionTitleHandle, SessionTitleTool};
pub use setup::{SETUP_INTERVIEW_COMPLETE_MARKER, SETUP_INTERVIEW_PROMPT};
pub use skill_mining::{
    SkillDraft, SkillReplayReport, activate_skill_after_replay, draft_skill_from_traces,
};
pub use skills::{
    CREATE_SKILL_ID, SkillManifest, SkillSource, SkillStore, SkillWriteRequest, SkillWriteResult,
    SkillWriteScope, active_skills, builtin_skills, delete_skill, discover_configured_skills,
    load_skill_by_id, project_skill_key, skill_is_editable, skill_tool_allowlist, slugify_skill_id,
    write_skill,
};
pub use tool::background::{BackgroundCommandSnapshot, BackgroundTaskStatus};
pub use tool::registry::{ToolRegistry, ToolSet, phases};
pub use tool::{
    AgentProfile, ApprovalMode, ProviderBuilderFn, RepoExploreTool, SubagentTool, Tool,
    ToolDefinition, ToolExecutor, ToolExposure, ToolInvocation, ToolKind, ToolMetadata, ToolResult,
    ToolRisk, capabilities,
};
pub use trace::{
    ApprovalTrace, ToolCallTrace, TraceStore, TurnMetrics, TurnOutcome, TurnTrace, VerifierTrace,
    turn_traces_from_events,
};
pub use update::{
    UpdateInfo, apply_update, check_for_update, current_version, normalize_version,
    version_is_newer,
};

pub use background_model::{BackgroundModelResolver, ResolvedBackgroundModel};
pub use benchmark::{
    BenchAgentConfig, BenchCase, BenchCaseMetrics, BenchCaseResult, BenchCompareConfig,
    BenchComparison, BenchRun, BenchRunMetrics, BenchSuite, aggregate_bench_metrics,
    compare_bench_runs,
};
pub use memory::{HistoryStore, MemoryManager, MemoryStore, SessionCheckpoint};
pub use registry::{
    RegistryTranscriptionModel, RegistryTranscriptionProvider, TranscriptionModelPricing,
    TranscriptionProviderKind, embedded_transcription_provider_schema,
    embedded_transcription_providers,
};
pub use transcription_catalog::{
    find_transcription_provider, resolve_transcription_model, transcription_provider_catalog,
};
pub use verifier::{VerificationStore, VerifierResult, VerifierRunner, VerifierSpec};
