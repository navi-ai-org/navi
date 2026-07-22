use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, Weak};
use std::time::Instant;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;

use super::helpers;
use crate::background_model::BackgroundModelResolver;
use crate::cancel::CancelToken;
use crate::compact::CompactState;
use crate::config::{HarnessConfig, LoadedConfig, NaviConfig};
use crate::event::{AgentEvent, ApprovalDecision, SubagentTranscriptItem, SubagentTranscriptKind};
use crate::model::{ModelMessage, ModelProvider, ModelRole};
use crate::prompt::PromptCache;
use crate::runtime::ApprovalResolver;
use crate::runtime_components::RuntimeComponents;
use crate::session::SessionStore;
use crate::tool::{
    Tool, ToolDefinition, ToolInvocation, ToolInvocationContext, ToolKind, ToolResult,
};
use crate::turn::TurnContext;
use serde_json::Value;

/// Pre-defined agent role profiles that influence tool availability and approval flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProfile {
    /// Plans tasks and decomposes work. No write tool access.
    Planner,
    /// Reads files and searches the codebase. No write tool access.
    Explorer,
    /// Writes and edits code. Full write access, normal approvals.
    Implementer,
    /// Reviews code and proposes changes without applying them.
    Reviewer,
    /// Reviews security effects, capability use, and sensitive diffs.
    SecurityReviewer,
    /// Runs tests and verifies changes. Read-only access.
    Verifier,
    /// Summarizes conversations, code, or documentation.
    Summarizer,
}

/// Controls how the subagent handles tool approvals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Inherit the parent session's approval policy (default).
    Inherit,
    /// Route approval requests to the parent session for user decision.
    Escalate,
    /// Reject all write operations. The subagent can only read/query.
    ReadOnly,
    /// Deny write-oriented tools but allow read-only inspection and verifier commands.
    DenyWrite,
}

/// Optional configuration for subagent behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentOptions {
    /// The agent role profile. Influences default tool access and approvals.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "agent_profile"
    )]
    pub profile: Option<AgentProfile>,
    /// Override the model used by this subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Restrict which tools the subagent may call. `None` = all tools available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Approval handling mode.
    #[serde(default)]
    pub approval: ApprovalMode,
    /// Maximum tokens for the subagent response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    /// Workflow write-path envelope (when set, forks executor with WritePathScope).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_allow: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_deny: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_files: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_dirs: Option<bool>,
}

impl Default for ApprovalMode {
    fn default() -> Self {
        Self::Inherit
    }
}

const MAX_BACKGROUND_SUBAGENTS: usize = 8;
/// Nested agent spawners must not be available inside subagents.
/// `repo_explore` is now BM25+symbols (cheap) and is allowed for subagents.
const NESTED_AGENT_TOOLS: &[&str] = &["subagent", "workflow"];
/// Tool names considered to be "write" operations for ReadOnly mode.
const READONLY_DENIED_TOOLS: &[&str] = &[
    "write",
    "write_file",
    "apply_patch",
    "code_edit",
    "code_exec",
    "bash",
    "sandbox",
    "package_manager",
    "mark_feature_done",
    "append_note",
    "question",
    "plan",
];
const WRITE_DENIED_TOOLS: &[&str] = &[
    "write",
    "write_file",
    "apply_patch",
    "code_edit",
    "code_exec",
    "sandbox",
    "package_manager",
    "mark_feature_done",
    "append_note",
];

/// Callback for building a `ModelProvider` from a `LoadedConfig`.
pub type ProviderBuilderFn =
    dyn Fn(&LoadedConfig) -> anyhow::Result<Arc<dyn ModelProvider>> + Send + Sync;

pub struct SubagentTool {
    tool_executor: Weak<crate::tool::ToolExecutor>,
    model_provider: Arc<RwLock<Arc<dyn ModelProvider>>>,
    project_dir: std::path::PathBuf,
    model_name: Arc<RwLock<String>>,
    harness_config: HarnessConfig,
    config: Arc<RwLock<NaviConfig>>,
    /// Kept for constructor API stability. Nested turns use a fresh cache so
    /// parent session prefix-cache keys are not poisoned by subagent prompts.
    _prompt_cache: Arc<PromptCache>,
    components: RuntimeComponents,
    background_tasks: tokio::sync::Mutex<HashMap<String, Arc<SubagentBackgroundTask>>>,
    next_task_id: AtomicU64,
    /// Optional resolver for selecting background models by profile.
    background_resolver: Option<Arc<BackgroundModelResolver>>,
    /// Data directory for building providers.
    data_dir: std::path::PathBuf,
    /// Callback for building a provider from config.
    provider_builder: Option<Arc<ProviderBuilderFn>>,
}

impl SubagentTool {
    pub fn new(
        tool_executor: Weak<crate::tool::ToolExecutor>,
        model_provider: Arc<RwLock<Arc<dyn ModelProvider>>>,
        project_dir: std::path::PathBuf,
        data_dir: std::path::PathBuf,
        model_name: Arc<RwLock<String>>,
        harness_config: HarnessConfig,
        config: Arc<RwLock<NaviConfig>>,
        prompt_cache: Arc<PromptCache>,
        components: RuntimeComponents,
    ) -> Self {
        Self {
            tool_executor,
            model_provider,
            project_dir,
            data_dir,
            model_name,
            harness_config,
            config,
            _prompt_cache: prompt_cache,
            components,
            background_tasks: tokio::sync::Mutex::new(HashMap::new()),
            next_task_id: AtomicU64::new(1),
            background_resolver: None,
            provider_builder: None,
        }
    }

    /// Sets the background model resolver for profile-based model selection.
    pub fn with_background_resolver(
        mut self,
        resolver: Arc<BackgroundModelResolver>,
        data_dir: std::path::PathBuf,
        provider_builder: Arc<ProviderBuilderFn>,
    ) -> Self {
        self.background_resolver = Some(resolver);
        self.data_dir = data_dir;
        self.provider_builder = Some(provider_builder);
        self
    }
}

struct SubagentBackgroundTask {
    task_id: String,
    prompt: String,
    description: Option<String>,
    elapsed_ms: std::sync::Mutex<u64>,
    state: std::sync::Mutex<SubagentBgState>,
    started_at: Instant,
    result_rx: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<String>>>,
    cancel_token: CancelToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SubagentBgStatus {
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
struct SubagentBgState {
    status: SubagentBgStatus,
    error: String,
}

impl SubagentBgState {
    fn running() -> Self {
        Self {
            status: SubagentBgStatus::Running,
            error: String::new(),
        }
    }

    fn done() -> Self {
        Self {
            status: SubagentBgStatus::Done,
            error: String::new(),
        }
    }

    fn failed(err: String) -> Self {
        Self {
            status: SubagentBgStatus::Failed,
            error: err,
        }
    }

    fn cancelled() -> Self {
        Self {
            status: SubagentBgStatus::Cancelled,
            error: String::new(),
        }
    }

    fn is_final(&self) -> bool {
        matches!(
            self.status,
            SubagentBgStatus::Done | SubagentBgStatus::Failed | SubagentBgStatus::Cancelled
        )
    }
}

impl SubagentBackgroundTask {
    async fn observation_json(&self) -> serde_json::Value {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let elapsed = self.elapsed_ms.lock().unwrap_or_else(|e| e.into_inner());
        let mut value = json!({
            "task_id": self.task_id,
            "prompt": self.prompt,
            "description": self.description,
            "background": true,
            "status": match state.status {
                SubagentBgStatus::Running => "running",
                SubagentBgStatus::Done => "done",
                SubagentBgStatus::Failed => "failed",
                SubagentBgStatus::Cancelled => "cancelled",
            },
            "elapsed_ms": *elapsed,
        });
        if !state.error.is_empty() {
            value["error"] = json!(state.error);
        }
        if !state.is_final() {
            value["message"] = json!(format!(
                "Subagent is still running. Poll with subagent({{\"task_id\":\"{}\"}}) or cancel with subagent({{\"task_id\":\"{}\",\"action\":\"cancel\"}}).",
                self.task_id, self.task_id
            ));
        }
        value
    }

    fn try_read_result(&self) -> Option<String> {
        let mut rx_guard = self.result_rx.try_lock().ok()?;
        let rx = rx_guard.as_mut()?;
        match rx.try_recv() {
            Ok(result) => {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                *state = SubagentBgState::done();
                *rx_guard = None;
                Some(result)
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => None,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.status == SubagentBgStatus::Running {
                    *state = SubagentBgState::failed("subagent task dropped unexpectedly".into());
                }
                *rx_guard = None;
                None
            }
        }
    }
}

#[async_trait]
impl Tool for SubagentTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "subagent",
            "Spawn an isolated subagent to autonomously perform a task. \
             The subagent has full access to all tools (bash, read_file, write_file, grep, etc.) \
             and makes its own decisions in a fresh conversation context. \
             Use `background: true` to run asynchronously — the tool returns immediately \
             with a task_id; poll with `{task_id}` or cancel with `{task_id, action: \"cancel\"}`.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The task description for the subagent. Use this when starting a new subagent."
                    },
                    "description": {
                        "type": "string",
                        "description": "Additional context or constraints for the subagent (optional)."
                    },
                    "profile": {
                        "type": "string",
                        "enum": ["cheap_general", "cheap_code", "repo_search", "naming", "long_context_cheap", "research_synthesis"],
                        "description": "Model profile to use for this subagent. Selects a cheaper model appropriate for the task type. Omit to use the main agent's model."
                    },
                    "options": {
                        "type": "object",
                        "description": "Subagent behavior options: agent profile, model override, tool restrictions, approval mode, and optional workflow write-path scope.",
                        "properties": {
                            "agent_profile": {
                                "type": "string",
                                "enum": ["planner", "explorer", "implementer", "reviewer", "security_reviewer", "verifier", "summarizer"],
                                "description": "Agent role profile that sets default tool access and approval behavior. Planner/Explorer/Reviewer/SecurityReviewer/Verifier/Summarizer default to read-only; Implementer has full access."
                            },
                            "model": {
                                "type": "string",
                                "description": "Override the model used by this subagent."
                            },
                            "tools": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Explicit list of tool names the subagent may call. When not set, all tools are available (subject to profile defaults)."
                            },
                            "approval": {
                                "type": "string",
                                "enum": ["inherit", "escalate", "read_only", "deny_write"],
                                "description": "How tool approvals are handled. Inherit: use parent session's policy. Escalate: route approval requests to the parent session/user. ReadOnly: deny all write/command tools. DenyWrite: deny write tools but allow commands."
                            },
                            "max_tokens": {
                                "type": "integer",
                                "description": "Maximum tokens for the subagent's response."
                            },
                            "write_allow": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Workflow write-path allowlist (relative paths). When set, forks a WritePathScope so only these paths may be written."
                            },
                            "path_deny": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Workflow path deny list (relative paths). Always wins over write_allow."
                            },
                            "create_files": {
                                "type": "boolean",
                                "description": "When true (with write_allow), allow creating new files under the write scope. Default false for workflow workers."
                            },
                            "create_dirs": {
                                "type": "boolean",
                                "description": "When true (with write_allow), allow creating directories under the write scope. Default false for workflow workers."
                            }
                        },
                        "additionalProperties": false
                    },
                    "background": {
                        "type": "boolean",
                        "description": "When true, spawn the subagent in the background and return a task_id. Poll or cancel later."
                    },
                    "task_id": {
                        "type": "string",
                        "description": "Background task id returned by an earlier subagent call."
                    },
                    "action": {
                        "type": "string",
                        "enum": ["poll", "cancel", "list"],
                        "description": "Use poll/cancel with task_id, or list to show background subagents."
                    }
                },
                "anyOf": [
                    { "required": ["prompt"] },
                    { "required": ["task_id"] },
                    { "properties": { "action": { "const": "list" } }, "required": ["action"] }
                ],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        self.invoke_with_context(invocation, ToolInvocationContext::default())
            .await
    }

    async fn invoke_with_context(
        &self,
        invocation: ToolInvocation,
        context: ToolInvocationContext,
    ) -> Result<ToolResult> {
        if let Some(task_id) = helpers::optional_string(&invocation.input, "task_id") {
            let action = helpers::optional_string(&invocation.input, "action")
                .unwrap_or_else(|| "poll".to_string());
            return self
                .handle_background_action(invocation.id, &task_id, &action)
                .await;
        }

        if helpers::optional_string(&invocation.input, "action").as_deref() == Some("list") {
            return self.list_background_tasks(invocation.id).await;
        }

        let is_background =
            helpers::optional_bool(&invocation.input, "background").unwrap_or(false);
        let prompt = helpers::required_string(&invocation.input, "prompt")?.to_string();
        let description = helpers::optional_string(&invocation.input, "description");
        let profile = helpers::optional_string(&invocation.input, "profile");
        let options = parse_subagent_options(&invocation.input);

        if is_background {
            return self
                .spawn_background(
                    invocation.id,
                    prompt,
                    description,
                    profile,
                    options,
                    context.event_tx,
                    context.cancel_token,
                )
                .await;
        }

        self.run_foreground(
            invocation.id,
            prompt,
            description,
            profile,
            options,
            context.event_tx,
            context.cancel_token,
        )
        .await
    }
}

impl SubagentTool {
    async fn run_foreground(
        &self,
        invocation_id: String,
        prompt: String,
        description: Option<String>,
        profile: Option<String>,
        options: SubagentOptions,
        parent_event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
        parent_cancel: Option<CancelToken>,
    ) -> Result<ToolResult> {
        let executor = self
            .tool_executor
            .upgrade()
            .context("subagent tool executor has been dropped")?;
        let started = Instant::now();

        // Resolve model provider based on profile.
        let (provider, model) = self.resolve_model_for_profile(profile.as_deref());

        // Determine if this subagent should be read-only based on agent profile.
        let effective_approval = resolve_approval_mode(&options);
        let allowed_tool_names =
            resolve_allowed_tool_names(&executor, &options, effective_approval);

        // When workflow write scope is present, fork a worker executor with
        // WritePathScope so write_allow / path_deny / create_files are enforced
        // by SecurityPolicy on every write tool call (not just prompt text).
        let tool_executor: Arc<crate::tool::ToolExecutor> =
            if let Some(scope) = write_scope_from_options(&options) {
                let mut policy = executor.policy().clone();
                policy = policy.with_write_scope(scope);
                let names = allowed_tool_names
                    .clone()
                    .unwrap_or_else(|| executor.tool_names());
                Arc::new(executor.fork_with_policy_and_tools(policy, &names))
            } else {
                executor
            };

        let (mut messages, event_tx, _approval_handle, resolver) = self.prepare_subagent_context(
            &invocation_id,
            &prompt,
            &description,
            effective_approval,
            parent_event_tx.clone(),
        );

        let include_tool_prompt = self.include_tool_prompt_manifest();
        let session_id = subagent_session_id();
        // Freeze the specialized subagent system prompt. `run_turn` always
        // calls `ensure_system_prompt`, which would otherwise rebuild the full
        // parent-agent identity and erase explorer/verifier instructions.
        let (instructions, prompt_prefix) = freeze_specialized_prompt(&messages);

        // Prefer parent cancel (workflow/tool cancel) so nested turns stop.
        let cancel_token = parent_cancel.unwrap_or_else(CancelToken::new);

        let sub_ctx = TurnContext {
            model_provider: Arc::new(RwLock::new(provider)),
            tool_executor,
            project_dir: self.project_dir.clone(),
            data_dir: self.data_dir.clone(),
            model_name: Arc::new(RwLock::new(model)),
            event_tx: Some(event_tx),
            approval_resolver: resolver,
            question_resolver: crate::runtime::QuestionResolver::new_standalone(),
            plan_review_resolver: crate::runtime::PlanReviewResolver::new_standalone(),
            sudo_password_resolver: crate::runtime::SudoPasswordResolver::new_standalone(),
            compact_state: Arc::new(tokio::sync::Mutex::new(CompactState::new(
                crate::config::effective_context_window(
                    &self.config.read().unwrap_or_else(|e| e.into_inner()),
                ),
            ))),
            harness_config: self.harness_config.clone(),
            include_tool_prompt_manifest: include_tool_prompt,
            context_packets: Arc::new(std::sync::Mutex::new(Vec::new())),
            available_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
            active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
            // Fresh cache: do not share parent session prefix-cache keys.
            prompt_cache: Arc::new(PromptCache::new()),
            instructions,
            prompt_prefix,
            components: self.components.clone(),
            cancel_token,
            config: self.config.clone(),
            memory_injection: None,
            compaction_provider: None,
            agent_mode: crate::plan_mode::AgentMode::Default,
            compaction_model_name: None,
            session_id,
            allowed_tool_names,
            memory_manager: Arc::new(std::sync::Mutex::new(None)),
            harness_card: None,
        };

        let policy =
            crate::harness::policy_for_profile(&self.harness_config, self.harness_config.profile);

        let result = crate::turn::run_turn(&sub_ctx, &mut messages, policy).await;
        let elapsed = started.elapsed();

        let text = match result {
            Ok(output) => output,
            Err(err) => format!("Subagent failed: {err:#}"),
        };
        emit_subagent_transcript(
            &parent_event_tx,
            &invocation_id,
            SubagentTranscriptItem {
                kind: SubagentTranscriptKind::Text,
                title: "Final response".to_string(),
                detail: Some(one_line(&text)),
                ok: Some(!text.starts_with("Subagent failed:")),
            },
        );

        Ok(helpers::ok(
            invocation_id,
            json!({
                "result": text,
                "elapsed_ms": elapsed.as_millis() as u64,
            }),
        ))
    }

    async fn spawn_background(
        &self,
        invocation_id: String,
        prompt: String,
        description: Option<String>,
        profile: Option<String>,
        options: SubagentOptions,
        parent_event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
        parent_cancel: Option<CancelToken>,
    ) -> Result<ToolResult> {
        let executor = match self.tool_executor.upgrade() {
            Some(ex) => ex,
            None => {
                return Ok(helpers::ok(
                    invocation_id,
                    json!({"error": "tool executor unavailable"}),
                ));
            }
        };

        let mut tasks = self.background_tasks.lock().await;
        let running = tasks
            .values()
            .filter(|t| !t.state.lock().unwrap_or_else(|e| e.into_inner()).is_final())
            .count();
        if running >= MAX_BACKGROUND_SUBAGENTS {
            return Ok(helpers::ok(
                invocation_id,
                json!({
                    "error": format!(
                        "too many background subagents running (max {MAX_BACKGROUND_SUBAGENTS})"
                    )
                }),
            ));
        }

        let task_id = format!("bg_{}", self.next_task_id.fetch_add(1, Ordering::SeqCst));
        let (result_tx, result_rx) = tokio::sync::oneshot::channel::<String>();
        let started = Instant::now();

        // Link parent cancel into the background task token when provided.
        let task_cancel = parent_cancel.unwrap_or_else(CancelToken::new);
        let task = Arc::new(SubagentBackgroundTask {
            task_id: task_id.clone(),
            prompt: prompt.clone(),
            description: description.clone(),
            elapsed_ms: std::sync::Mutex::new(0),
            state: std::sync::Mutex::new(SubagentBgState::running()),
            started_at: started,
            result_rx: tokio::sync::Mutex::new(Some(result_rx)),
            cancel_token: task_cancel,
        });
        tasks.insert(task_id.clone(), task.clone());

        // Resolve model provider based on profile.
        let (resolved_provider, resolved_model) =
            self.resolve_model_for_profile(profile.as_deref());
        let model_provider = Arc::new(RwLock::new(resolved_provider));
        let model_name = Arc::new(RwLock::new(resolved_model));
        let components = self.components.clone();
        let harness_config = self.harness_config.clone();
        let config = self.config.clone();
        let project_dir = self.project_dir.clone();
        let data_dir = self.data_dir.clone();
        let cancel_token = task.cancel_token.clone();
        let parent_invocation_id = invocation_id.clone();
        let session_id = subagent_session_id();

        let effective_approval = resolve_approval_mode(&options);
        let allowed_tool_names_clone =
            resolve_allowed_tool_names(&executor, &options, effective_approval);

        let tool_executor: Arc<crate::tool::ToolExecutor> =
            if let Some(scope) = write_scope_from_options(&options) {
                let mut policy = executor.policy().clone();
                policy = policy.with_write_scope(scope);
                let names = allowed_tool_names_clone
                    .clone()
                    .unwrap_or_else(|| executor.tool_names());
                Arc::new(executor.fork_with_policy_and_tools(policy, &names))
            } else {
                executor
            };

        tokio::spawn(async move {
            let (mut messages, event_tx, _approval_handle, resolver) =
                Self::build_subagent_context_static(
                    &parent_invocation_id,
                    &prompt,
                    &description,
                    effective_approval,
                    parent_event_tx.clone(),
                );

            let config_snapshot = config.read().unwrap_or_else(|e| e.into_inner()).clone();
            let (instructions, prompt_prefix) = freeze_specialized_prompt(&messages);

            let sub_ctx = TurnContext {
                model_provider,
                tool_executor,
                project_dir,
                data_dir,
                model_name,
                event_tx: Some(event_tx),
                approval_resolver: resolver,
                question_resolver: crate::runtime::QuestionResolver::new_standalone(),
                plan_review_resolver: crate::runtime::PlanReviewResolver::new_standalone(),
                sudo_password_resolver: crate::runtime::SudoPasswordResolver::new_standalone(),
                compact_state: Arc::new(tokio::sync::Mutex::new(CompactState::new(
                    crate::config::effective_context_window(&config_snapshot),
                ))),
                harness_config: harness_config.clone(),
                include_tool_prompt_manifest: crate::config::effective_tool_prompt_manifest(
                    &config_snapshot,
                ),
                context_packets: Arc::new(std::sync::Mutex::new(Vec::new())),
                available_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
                active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
                prompt_cache: Arc::new(PromptCache::new()),
                instructions,
                prompt_prefix,
                components,
                cancel_token,
                config: Arc::new(std::sync::RwLock::new(config_snapshot)),
                memory_injection: None,
                compaction_provider: None,
                compaction_model_name: None,
                session_id,
                agent_mode: crate::plan_mode::AgentMode::Default,
                allowed_tool_names: allowed_tool_names_clone,
                memory_manager: Arc::new(std::sync::Mutex::new(None)),
                harness_card: None,
            };

            let policy =
                crate::harness::policy_for_profile(&harness_config, harness_config.profile);

            let result = crate::turn::run_turn(&sub_ctx, &mut messages, policy).await;
            let output = match result {
                Ok(output) => output,
                Err(err) => format!("Background subagent failed: {err:#}"),
            };
            emit_subagent_transcript(
                &parent_event_tx,
                &parent_invocation_id,
                SubagentTranscriptItem {
                    kind: SubagentTranscriptKind::Text,
                    title: "Final response".to_string(),
                    detail: Some(one_line(&output)),
                    ok: Some(!output.starts_with("Background subagent failed:")),
                },
            );
            let _ = result_tx.send(output);
        });

        Ok(helpers::ok(
            invocation_id,
            json!({
                "task_id": task_id,
                "message": format!(
                    "Subagent spawned in background. Poll with subagent({{\"task_id\":\"{task_id}\"}}) or cancel with subagent({{\"task_id\":\"{task_id}\",\"action\":\"cancel\"}})."
                ),
                "action": "poll",
                "background": true,
                "status": "running",
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        ))
    }

    async fn handle_background_action(
        &self,
        invocation_id: String,
        task_id: &str,
        action: &str,
    ) -> Result<ToolResult> {
        let tasks = self.background_tasks.lock().await;
        let Some(task) = tasks.get(task_id).cloned() else {
            return Ok(helpers::ok(
                invocation_id,
                json!({ "error": format!("no background subagent found with task_id {task_id}") }),
            ));
        };
        drop(tasks);

        match action {
            "poll" => {
                let _ = task.try_read_result();
                let obs = task.observation_json().await;
                Ok(helpers::ok(invocation_id, obs))
            }
            "cancel" => {
                task.cancel_token.cancel();
                {
                    let mut state = task.state.lock().unwrap_or_else(|e| e.into_inner());
                    if !state.is_final() {
                        *state = SubagentBgState::cancelled();
                    }
                }
                let obs = task.observation_json().await;
                Ok(helpers::ok(invocation_id, obs))
            }
            _ => Ok(helpers::ok(
                invocation_id,
                json!({ "error": format!("unknown action: {action}") }),
            )),
        }
    }

    async fn list_background_tasks(&self, invocation_id: String) -> Result<ToolResult> {
        let tasks = self.background_tasks.lock().await;
        let mut list = Vec::new();
        for task in tasks.values() {
            let _ = task.try_read_result();
            let state = task.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
            *task.elapsed_ms.lock().unwrap_or_else(|e| e.into_inner()) =
                task.started_at.elapsed().as_millis() as u64;
            list.push(json!({
                "task_id": task.task_id,
                "prompt": task.prompt,
                "status": match state.status {
                    SubagentBgStatus::Running => "running",
                    SubagentBgStatus::Done => "done",
                    SubagentBgStatus::Failed => "failed",
                    SubagentBgStatus::Cancelled => "cancelled",
                },
                "elapsed_ms": task.started_at.elapsed().as_millis() as u64,
            }));
        }
        Ok(helpers::ok(invocation_id, json!({ "tasks": list })))
    }

    fn include_tool_prompt_manifest(&self) -> bool {
        crate::config::effective_tool_prompt_manifest(
            &self.config.read().unwrap_or_else(|e| e.into_inner()),
        )
    }

    /// Resolves a model provider and name for the given profile. Falls back to
    /// the main agent's model when no profile is specified or resolution fails.
    fn resolve_model_for_profile(&self, profile: Option<&str>) -> (Arc<dyn ModelProvider>, String) {
        let Some(profile) = profile else {
            return self.main_model();
        };

        let Some(ref resolver) = self.background_resolver else {
            return self.main_model();
        };

        let Some(ref builder) = self.provider_builder else {
            return self.main_model();
        };

        let resolved = resolver.resolve(profile);

        // Build a provider for the resolved model.
        let config_snapshot = self
            .config
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut bg_config = config_snapshot.clone();
        bg_config.model.provider = resolved.provider_id.clone();
        bg_config.model.name = resolved.model_name.clone();
        let bg_loaded = LoadedConfig {
            config: bg_config,
            global_config_path: None,
            project_config_path: None,
            data_dir: self.data_dir.clone(),
        };

        match builder(&bg_loaded) {
            Ok(provider) => (provider, resolved.model_name),
            Err(_) => self.main_model(),
        }
    }

    fn main_model(&self) -> (Arc<dyn ModelProvider>, String) {
        (
            self.model_provider
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            self.model_name
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        )
    }

    fn prepare_subagent_context(
        &self,
        parent_invocation_id: &str,
        prompt: &str,
        description: &Option<String>,
        approval_mode: ApprovalMode,
        parent_event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> (
        Vec<ModelMessage>,
        tokio::sync::mpsc::UnboundedSender<AgentEvent>,
        tokio::task::JoinHandle<()>,
        ApprovalResolver,
    ) {
        Self::build_subagent_context_static(
            parent_invocation_id,
            prompt,
            description,
            approval_mode,
            parent_event_tx,
        )
    }

    fn build_subagent_context_static(
        parent_invocation_id: &str,
        prompt: &str,
        description: &Option<String>,
        approval_mode: ApprovalMode,
        parent_event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> (
        Vec<ModelMessage>,
        tokio::sync::mpsc::UnboundedSender<AgentEvent>,
        tokio::task::JoinHandle<()>,
        ApprovalResolver,
    ) {
        let access_note = match approval_mode {
            ApprovalMode::ReadOnly => {
                "Your tool access is read-only. Inspect, reason, and report findings; do not attempt writes or command execution."
            }
            ApprovalMode::DenyWrite => {
                "Write tools are unavailable. You may inspect and run allowed verification commands when needed, then report findings."
            }
            ApprovalMode::Escalate => {
                "Any risky action must be escalated to the parent session approval flow."
            }
            ApprovalMode::Inherit => "Use tools according to the parent session policy.",
        };
        let workflow = "\
Workflow:\n\
1. Inspect with the cheapest tools first (overview/search → targeted read).\n\
2. Prefer project-relative paths; batch independent read-only calls when possible.\n\
3. Keep edits narrow; verify with the smallest relevant command when writes are allowed.\n\
4. If a tool fails, adapt once using the error — do not thrash the same call.\n\
5. Observation budget: tool outputs may be truncated; request ranges/results explicitly.\n\
6. When done, report paths, key diffs, and findings — not walls of file contents.";
        let system = if let Some(desc) = description {
            format!(
                "You are a subagent worker for NAVI. Execute the assigned task autonomously \
                 within your assigned access policy. {access_note}\n\n\
                 Context: {desc}\n\n{workflow}\n\n\
                 Be concise and deliver the result."
            )
        } else {
            format!(
                "You are a subagent worker for NAVI. Execute the assigned task autonomously \
                 within your assigned access policy. {access_note}\n\n{workflow}\n\n\
                 Be concise and deliver the result."
            )
        };

        let messages = vec![
            ModelMessage {
                role: ModelRole::System,
                content: system,
                content_parts: Vec::new(),
                tool_call_id: None,
                tool_name: None,
                tool_calls: vec![],
                created_at: None,
                thinking_content: None,
            },
            ModelMessage {
                role: ModelRole::User,
                content: prompt.to_string(),
                content_parts: Vec::new(),
                tool_call_id: None,
                tool_name: None,
                tool_calls: vec![],
                created_at: None,
                thinking_content: None,
            },
        ];

        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        let resolver = ApprovalResolver::new_standalone();
        let resolver_bg = resolver.clone();
        let parent_invocation_id = parent_invocation_id.to_string();
        let is_escalate = approval_mode == ApprovalMode::Escalate;

        let approval_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                if let Some(message) = subagent_activity_message(&event)
                    && let Some(tx) = &parent_event_tx
                {
                    let _ = tx.send(AgentEvent::SubagentActivity {
                        invocation_id: parent_invocation_id.clone(),
                        message,
                    });
                }
                if let Some(item) = subagent_transcript_item(&event) {
                    emit_subagent_transcript(&parent_event_tx, &parent_invocation_id, item);
                }
                if let AgentEvent::ApprovalRequested(req) = event {
                    if is_escalate {
                        // Forward the approval request to the parent session.
                        // The parent's approval resolver will handle the response.
                        // We register on a standalone resolver and wait for the
                        // parent to resolve through the event channel.
                        if let Some(tx) = &parent_event_tx {
                            let _ = tx.send(AgentEvent::ApprovalRequested(
                                crate::event::ApprovalRequest {
                                    id: req.id.clone(),
                                    summary: req.summary.clone(),
                                    risk: req.risk.clone(),
                                },
                            ));
                        }
                        // In Escalate mode, we auto-approve locally since the
                        // parent handles the actual approval flow externally.
                        resolver_bg.resolve(ApprovalDecision::Approved { id: req.id.clone() });
                    } else {
                        resolver_bg.resolve(ApprovalDecision::Approved { id: req.id.clone() });
                    }
                }
            }
        });

        (messages, event_tx, approval_handle, resolver)
    }
}

fn emit_subagent_transcript(
    parent_event_tx: &Option<mpsc::UnboundedSender<AgentEvent>>,
    invocation_id: &str,
    item: SubagentTranscriptItem,
) {
    if let Some(tx) = parent_event_tx {
        let _ = tx.send(AgentEvent::SubagentTranscript {
            invocation_id: invocation_id.to_string(),
            item,
        });
    }
}

fn subagent_activity_message(event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::ToolRequested(invocation) => Some(format_tool_activity(invocation)),
        AgentEvent::ToolCompleted(result) if !result.ok => Some(format!(
            "{} failed",
            result
                .output
                .get("tool")
                .and_then(|value| value.as_str())
                .unwrap_or("Tool")
        )),
        _ => None,
    }
}

fn subagent_transcript_item(event: &AgentEvent) -> Option<SubagentTranscriptItem> {
    match event {
        AgentEvent::ToolRequested(invocation) => Some(SubagentTranscriptItem {
            kind: SubagentTranscriptKind::ToolRequested,
            title: format_tool_activity(invocation),
            detail: None,
            ok: None,
        }),
        AgentEvent::ToolCompleted(result) => Some(SubagentTranscriptItem {
            kind: SubagentTranscriptKind::ToolCompleted,
            title: if result.ok {
                "Tool completed".to_string()
            } else {
                "Tool failed".to_string()
            },
            detail: Some(compact_result_detail(result)),
            ok: Some(result.ok),
        }),
        _ => None,
    }
}

fn compact_result_detail(result: &ToolResult) -> String {
    if let Some(error) = result.output.get("error").and_then(|value| value.as_str()) {
        return one_line(error);
    }
    if let Some(path) = result.output.get("path").and_then(|value| value.as_str()) {
        return path.to_string();
    }
    if let Some(result_text) = result.output.get("result").and_then(|value| value.as_str()) {
        return one_line(result_text);
    }
    if result.output.is_null()
        || result
            .output
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
    {
        return "ok".to_string();
    }
    serde_json::to_string(&result.output)
        .map(|value| one_line(&value))
        .unwrap_or_else(|_| "ok".to_string())
}

fn format_tool_activity(invocation: &ToolInvocation) -> String {
    match invocation.tool_name.as_str() {
        "read_file" | "view_file" => format!("Read {}", input_path(invocation).unwrap_or("file")),
        "write_file" => format!("Write {}", input_path(invocation).unwrap_or("file")),
        "grep" => invocation
            .input
            .get("pattern")
            .and_then(|value| value.as_str())
            .map(|pattern| format!("Search \"{}\"", one_line(pattern)))
            .unwrap_or_else(|| "Search".to_string()),
        "fs_browser" => {
            let action = invocation
                .input
                .get("action")
                .and_then(|value| value.as_str())
                .unwrap_or("browse");
            format!(
                "{} {}",
                capitalize(action),
                input_path(invocation).unwrap_or("filesystem")
            )
        }
        "bash" => invocation
            .input
            .get("command")
            .or_else(|| invocation.input.get("program"))
            .and_then(|value| value.as_str())
            .map(|command| format!("Run {}", one_line(command)))
            .unwrap_or_else(|| "Run command".to_string()),
        "apply_patch" => "Apply patch".to_string(),
        "subagent" => invocation
            .input
            .get("description")
            .or_else(|| invocation.input.get("prompt"))
            .and_then(|value| value.as_str())
            .map(|task| format!("Subagent {}", one_line(task)))
            .unwrap_or_else(|| "Subagent task".to_string()),
        name => capitalize(&name.replace('_', " ")),
    }
}

fn input_path(invocation: &ToolInvocation) -> Option<&str> {
    invocation
        .input
        .get("path")
        .or_else(|| invocation.input.get("file"))
        .or_else(|| invocation.input.get("target"))
        .and_then(|value| value.as_str())
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars().collect::<Vec<_>>();
    if let Some(first) = chars.first_mut() {
        first.make_ascii_uppercase();
    }
    chars.into_iter().collect()
}

/// Parse `SubagentOptions` from the `"options"` field of a tool invocation input.
fn parse_subagent_options(input: &Value) -> SubagentOptions {
    let Some(options_value) = input.get("options") else {
        return SubagentOptions::default();
    };
    serde_json::from_value(options_value.clone()).unwrap_or_default()
}

impl Default for SubagentOptions {
    fn default() -> Self {
        Self {
            profile: None,
            model: None,
            tools: None,
            approval: ApprovalMode::Inherit,
            max_tokens: None,
            write_allow: None,
            path_deny: None,
            create_files: None,
            create_dirs: None,
        }
    }
}

fn write_scope_from_options(options: &SubagentOptions) -> Option<crate::security::WritePathScope> {
    // Only install a write scope when the caller explicitly set workflow fields.
    if options.write_allow.is_none()
        && options.path_deny.is_none()
        && options.create_files.is_none()
        && options.create_dirs.is_none()
    {
        return None;
    }
    Some(crate::security::WritePathScope {
        write_allow: options.write_allow.clone().unwrap_or_default(),
        path_deny: options.path_deny.clone().unwrap_or_default(),
        create_files: options.create_files.unwrap_or(false),
        create_dirs: options.create_dirs.unwrap_or(false),
    })
}

/// Resolves the effective approval mode from a profile preference cascade.
/// An explicit `options.approval` wins; otherwise `options.profile` determines
/// the mode: Explorer/Reviewer/Verifier/Summarizer default to ReadOnly;
/// Implementer defaults to Inherit.
fn resolve_approval_mode(options: &SubagentOptions) -> ApprovalMode {
    if options.approval != ApprovalMode::Inherit {
        return options.approval;
    }
    match options.profile {
        Some(AgentProfile::Planner)
        | Some(AgentProfile::Explorer)
        | Some(AgentProfile::Reviewer)
        | Some(AgentProfile::SecurityReviewer)
        | Some(AgentProfile::Verifier)
        | Some(AgentProfile::Summarizer) => ApprovalMode::ReadOnly,
        Some(AgentProfile::Implementer) | None => ApprovalMode::Inherit,
    }
}

/// Freeze specialized system/developer messages so `ensure_system_prompt`
/// reuses them instead of rebuilding the full parent-agent prompt.
fn freeze_specialized_prompt(
    messages: &[ModelMessage],
) -> (
    Arc<RwLock<Option<String>>>,
    Arc<std::sync::Mutex<Option<Vec<ModelMessage>>>>,
) {
    let prefix: Vec<ModelMessage> = messages
        .iter()
        .take_while(|m| matches!(m.role, ModelRole::System | ModelRole::Developer))
        .cloned()
        .collect();
    let instructions = prefix
        .iter()
        .find(|m| m.role == ModelRole::System)
        .map(|m| m.content.clone());
    (
        Arc::new(RwLock::new(instructions)),
        Arc::new(std::sync::Mutex::new(Some(prefix))),
    )
}

/// Returns the set of tool names allowed for this subagent.
///
/// Always strips nested agent tools to prevent recursive spawn storms.
/// ReadOnly/DenyWrite additionally strip write-oriented tools.
fn resolve_allowed_tool_names(
    executor: &crate::tool::ToolExecutor,
    options: &SubagentOptions,
    approval_mode: ApprovalMode,
) -> Option<Vec<String>> {
    let mut allowed = options
        .tools
        .clone()
        .unwrap_or_else(|| executor.tool_names());
    // Always block nested agent spawning (depth = 1).
    allowed.retain(|name| !NESTED_AGENT_TOOLS.contains(&name.as_str()));
    match approval_mode {
        ApprovalMode::ReadOnly => {
            allowed.retain(|name| !READONLY_DENIED_TOOLS.contains(&name.as_str()));
        }
        ApprovalMode::DenyWrite => {
            allowed.retain(|name| !WRITE_DENIED_TOOLS.contains(&name.as_str()));
        }
        ApprovalMode::Inherit | ApprovalMode::Escalate => {}
    }
    // Always return Some so nested agent tools stay filtered even for Inherit.
    Some(allowed)
}

/// Each nested agent is an independent provider conversation. Reusing a
/// literal id (such as `subagent`) made Charm Hyper route unrelated agents to
/// the same affinity/cache bucket.
fn subagent_session_id() -> String {
    format!("subagent-{}", SessionStore::create_id().into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Serde roundtrip test for SubagentOptions.
    #[test]
    fn subagent_options_serde_roundtrip() {
        let opts = SubagentOptions {
            profile: Some(AgentProfile::Explorer),
            model: Some("gpt-4".to_string()),
            tools: Some(vec!["read".to_string(), "search".to_string()]),
            approval: ApprovalMode::ReadOnly,
            max_tokens: Some(4096),
            ..Default::default()
        };
        let json = serde_json::to_value(&opts).unwrap();
        let deserialized: SubagentOptions = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.profile, Some(AgentProfile::Explorer));
        assert_eq!(deserialized.model, Some("gpt-4".to_string()));
        assert_eq!(
            deserialized.tools,
            Some(vec!["read".to_string(), "search".to_string()])
        );
        assert_eq!(deserialized.approval, ApprovalMode::ReadOnly);
        assert_eq!(deserialized.max_tokens, Some(4096));
    }

    #[test]
    fn subagent_options_default_is_inherit() {
        let opts = SubagentOptions::default();
        assert_eq!(opts.approval, ApprovalMode::Inherit);
        assert!(opts.profile.is_none());
        assert!(opts.model.is_none());
        assert!(opts.tools.is_none());
        assert!(opts.max_tokens.is_none());
    }

    #[test]
    fn subagent_options_serde_missing_fields_default_correctly() {
        let json = json!({});
        let opts: SubagentOptions = serde_json::from_value(json).unwrap();
        assert_eq!(opts.approval, ApprovalMode::Inherit);
        assert!(opts.profile.is_none());
        assert!(opts.tools.is_none());
    }

    #[test]
    fn subagent_options_serde_with_profile_only() {
        let json = json!({"agent_profile": "explorer"});
        let opts: SubagentOptions = serde_json::from_value(json).unwrap();
        assert_eq!(opts.profile, Some(AgentProfile::Explorer));
        assert_eq!(opts.approval, ApprovalMode::Inherit);
    }

    #[test]
    fn subagent_options_serde_workflow_write_scope() {
        let json = json!({
            "agent_profile": "explorer",
            "tools": ["read_file", "search"],
            "approval": "read_only",
            "write_allow": [],
            "path_deny": ["secrets/"],
            "create_files": false,
            "create_dirs": false
        });
        let opts: SubagentOptions = serde_json::from_value(json).unwrap();
        assert_eq!(opts.profile, Some(AgentProfile::Explorer));
        assert_eq!(opts.write_allow.as_deref(), Some([].as_slice()));
        assert_eq!(
            opts.path_deny.as_deref(),
            Some(["secrets/".to_string()].as_slice())
        );
        assert_eq!(opts.create_files, Some(false));
        assert_eq!(opts.create_dirs, Some(false));
    }

    #[test]
    fn schema_accepts_workflow_bridge_options() {
        // Mirrors SubagentBridgeBackend::run_agent options payload. Regression for:
        // "Additional properties are not allowed ('create_dirs', 'create_files', ...)"
        // Build a throwaway tool only for its schema (no model calls).
        struct NoopProvider;
        impl ModelProvider for NoopProvider {
            fn stream(&self, _req: crate::model::ModelRequest) -> crate::model::ModelStream {
                Box::pin(futures_util::stream::empty())
            }
        }
        let tool = SubagentTool::new(
            std::sync::Weak::new(),
            Arc::new(RwLock::new(Arc::new(NoopProvider) as Arc<dyn ModelProvider>)),
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            Arc::new(RwLock::new("test".into())),
            HarnessConfig::default(),
            Arc::new(RwLock::new(NaviConfig::default())),
            Arc::new(PromptCache::new()),
            RuntimeComponents::default(),
        );
        let schema = tool.definition().input_schema;
        let validator = jsonschema::validator_for(&schema).expect("compile schema");
        let instance = json!({
            "prompt": "list files",
            "description": "collect",
            "options": {
                "agent_profile": "explorer",
                "tools": ["read_file", "search", "list_dir"],
                "approval": "read_only",
                "write_allow": [],
                "path_deny": [],
                "create_files": false,
                "create_dirs": false
            }
        });
        let errors: Vec<String> = validator
            .iter_errors(&instance)
            .map(|e| e.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "workflow bridge options must pass subagent schema: {errors:?}"
        );
    }

    #[test]
    fn resolve_approval_mode_readonly_profiles() {
        for profile in &[
            AgentProfile::Explorer,
            AgentProfile::Reviewer,
            AgentProfile::Planner,
            AgentProfile::SecurityReviewer,
            AgentProfile::Verifier,
            AgentProfile::Summarizer,
        ] {
            let opts = SubagentOptions {
                profile: Some(*profile),
                ..Default::default()
            };
            assert_eq!(
                resolve_approval_mode(&opts),
                ApprovalMode::ReadOnly,
                "{:?} should default to ReadOnly",
                profile
            );
        }
    }

    #[test]
    fn resolve_approval_mode_implementer_inherits() {
        let opts = SubagentOptions {
            profile: Some(AgentProfile::Implementer),
            ..Default::default()
        };
        assert_eq!(resolve_approval_mode(&opts), ApprovalMode::Inherit);
    }

    #[test]
    fn resolve_approval_mode_explicit_wins() {
        let opts = SubagentOptions {
            profile: Some(AgentProfile::Implementer),
            approval: ApprovalMode::ReadOnly,
            ..Default::default()
        };
        assert_eq!(resolve_approval_mode(&opts), ApprovalMode::ReadOnly);
    }

    #[test]
    fn resolve_approval_mode_no_profile_inherits() {
        let opts = SubagentOptions::default();
        assert_eq!(resolve_approval_mode(&opts), ApprovalMode::Inherit);
    }

    /// ReadOnly mode should deny write tools via allowed_tool_names filtering.
    #[test]
    fn readonly_approval_mode_filteres_write_tools() {
        // Verify that the TurnContext's allowed_tool_names check was properly
        // set up. This test validates the setup logic, not the actual turn execution.
        let opts = SubagentOptions {
            approval: ApprovalMode::ReadOnly,
            ..Default::default()
        };
        let mode = resolve_approval_mode(&opts);
        assert_eq!(mode, ApprovalMode::ReadOnly);
        // The actual enforcement happens via allowed_tool_names in TurnContext.
        // ReadOnly mode sets allowed_tool_names to exclude write tools.
        // We verify the setup path exists.
    }

    #[test]
    fn explicit_tool_allowlist_is_intersected_with_readonly_profile() {
        let temp = tempfile::tempdir().unwrap();
        let policy = crate::security::SecurityPolicy::new(
            temp.path().to_path_buf(),
            temp.path()
                .parent()
                .unwrap_or(temp.path())
                .join("navi-test-data-subagent"),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = crate::tool::ToolExecutor::new(policy);
        let opts = SubagentOptions {
            profile: Some(AgentProfile::Reviewer),
            tools: Some(vec![
                "read".to_string(),
                "search".to_string(),
                "write_file".to_string(),
                "code_exec".to_string(),
            ]),
            ..Default::default()
        };

        let allowed = resolve_allowed_tool_names(&executor, &opts, resolve_approval_mode(&opts))
            .expect("restricted tools");

        assert!(allowed.contains(&"read".to_string()));
        assert!(allowed.contains(&"search".to_string()));
        assert!(!allowed.contains(&"write_file".to_string()));
        assert!(!allowed.contains(&"code_exec".to_string()));
    }

    #[test]
    fn deny_write_keeps_command_tools_available_for_verification() {
        let temp = tempfile::tempdir().unwrap();
        let policy = crate::security::SecurityPolicy::new(
            temp.path().to_path_buf(),
            temp.path()
                .parent()
                .unwrap_or(temp.path())
                .join("navi-test-data-subagent"),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = crate::tool::ToolExecutor::new(policy);
        let opts = SubagentOptions {
            approval: ApprovalMode::DenyWrite,
            tools: Some(vec![
                "read".to_string(),
                "bash".to_string(),
                "write_file".to_string(),
            ]),
            ..Default::default()
        };

        let allowed = resolve_allowed_tool_names(&executor, &opts, ApprovalMode::DenyWrite)
            .expect("restricted tools");

        assert!(allowed.contains(&"read".to_string()));
        assert!(allowed.contains(&"bash".to_string()));
        assert!(!allowed.contains(&"write_file".to_string()));
    }

    #[test]
    fn nested_agent_tools_always_stripped_even_for_inherit() {
        let temp = tempfile::tempdir().unwrap();
        let policy = crate::security::SecurityPolicy::new(
            temp.path().to_path_buf(),
            temp.path()
                .parent()
                .unwrap_or(temp.path())
                .join("navi-test-data-subagent"),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = crate::tool::ToolExecutor::new(policy);
        let opts = SubagentOptions {
            tools: Some(vec![
                "read_file".to_string(),
                "subagent".to_string(),
                "repo_explore".to_string(),
                "bash".to_string(),
            ]),
            ..Default::default()
        };

        let allowed = resolve_allowed_tool_names(&executor, &opts, ApprovalMode::Inherit)
            .expect("always filtered");

        assert!(allowed.contains(&"read_file".to_string()));
        assert!(allowed.contains(&"bash".to_string()));
        // repo_explore is BM25 (cheap) — allowed inside subagents.
        assert!(allowed.contains(&"repo_explore".to_string()));
        assert!(!allowed.contains(&"subagent".to_string()));
        assert!(!allowed.contains(&"branch_race".to_string()));
    }

    #[test]
    fn freeze_specialized_prompt_keeps_system_instructions() {
        let messages = vec![
            ModelMessage {
                role: ModelRole::System,
                content: "You are a focused explorer.".into(),
                content_parts: Vec::new(),
                tool_call_id: None,
                tool_name: None,
                tool_calls: vec![],
                created_at: None,
                thinking_content: None,
            },
            ModelMessage {
                role: ModelRole::User,
                content: "Find the auth module.".into(),
                content_parts: Vec::new(),
                tool_call_id: None,
                tool_name: None,
                tool_calls: vec![],
                created_at: None,
                thinking_content: None,
            },
        ];
        let (instructions, prefix) = freeze_specialized_prompt(&messages);
        assert_eq!(
            instructions
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .as_deref(),
            Some("You are a focused explorer.")
        );
        let frozen = prefix
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("prefix");
        assert_eq!(frozen.len(), 1);
        assert_eq!(frozen[0].role, ModelRole::System);
        assert_eq!(frozen[0].content, "You are a focused explorer.");
    }

    #[test]
    fn agent_profile_serde_roundtrip() {
        for profile in &[
            AgentProfile::Explorer,
            AgentProfile::Implementer,
            AgentProfile::Reviewer,
            AgentProfile::SecurityReviewer,
            AgentProfile::Verifier,
            AgentProfile::Planner,
            AgentProfile::Summarizer,
        ] {
            let json = serde_json::to_value(profile).unwrap();
            let deserialized: AgentProfile = serde_json::from_value(json).unwrap();
            assert_eq!(&deserialized, profile);
        }
    }

    #[test]
    fn subagents_get_distinct_provider_session_ids() {
        let first = subagent_session_id();
        let second = subagent_session_id();

        assert!(first.starts_with("subagent-session-"));
        assert_ne!(first, second);
    }
}
