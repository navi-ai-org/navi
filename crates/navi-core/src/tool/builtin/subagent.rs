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
use crate::cancel::CancelToken;
use crate::compact::CompactState;
use crate::config::{HarnessConfig, NaviConfig};
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

/// Optional configuration for subagent behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentOptions {
    /// Override the model used by this subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Additional relative path patterns to deny for file writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_deny: Option<Vec<String>>,
}

const MAX_BACKGROUND_SUBAGENTS: usize = 8;
/// Nested agent spawners must not be available inside subagents.
/// `repo_explore` is now BM25+symbols (cheap) and is allowed for subagents.
const NESTED_AGENT_TOOLS: &[&str] = &["subagent", "workflow"];

pub struct SubagentTool {
    tool_executor: Weak<crate::tool::ToolExecutor>,
    model_provider: Arc<RwLock<Arc<dyn ModelProvider>>>,
    project_dir: std::path::PathBuf,
    model_name: Arc<RwLock<String>>,
    harness_config: HarnessConfig,
    config: Arc<RwLock<NaviConfig>>,
    components: RuntimeComponents,
    background_tasks: tokio::sync::Mutex<HashMap<String, Arc<SubagentBackgroundTask>>>,
    next_task_id: AtomicU64,
    /// Data directory used when building subagent context.
    data_dir: std::path::PathBuf,
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
            components,
            background_tasks: tokio::sync::Mutex::new(HashMap::new()),
            next_task_id: AtomicU64::new(1),
        }
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
                    "options": {
                        "type": "object",
                        "description": "Subagent behavior options: model override and optional path deny list.",
                        "properties": {
                            "model": {
                                "type": "string",
                                "description": "Override the model used by this subagent."
                            },
                            "path_deny": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Additional relative path patterns to deny for file writes."
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
        let options = parse_subagent_options(&invocation.input);

        if is_background {
            return self
                .spawn_background(
                    invocation.id,
                    prompt,
                    description,
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
        options: SubagentOptions,
        parent_event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
        parent_cancel: Option<CancelToken>,
    ) -> Result<ToolResult> {
        let executor = self
            .tool_executor
            .upgrade()
            .context("subagent tool executor has been dropped")?;
        let started = Instant::now();

        // Resolve model provider based on options.
        let (provider, model) = self.resolve_model(&options);

        // Subagents run in yolo mode with all parent tools except nested spawners.
        let tool_executor = build_subagent_executor(&executor, options.path_deny.as_deref())?;

        let (mut messages, event_tx, _approval_handle, resolver) = self.prepare_subagent_context(
            &invocation_id,
            &prompt,
            &description,
            parent_event_tx.clone(),
        );

        let include_tool_prompt = self.include_tool_prompt_manifest();
        let session_id = subagent_session_id();
        // Freeze the specialized subagent system prompt. `run_turn` always
        // calls `ensure_system_prompt`, which would otherwise rebuild the full
        // parent-agent identity and erase explorer/verifier instructions.
        let (instructions, prompt_prefix) = freeze_specialized_prompt(&messages);

        // Prefer parent cancel (workflow/tool cancel) so nested turns stop.
        let cancel_token = parent_cancel.unwrap_or_default();

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
            skill_pools: Arc::new(std::sync::Mutex::new(Vec::new())),
            active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
            // Fresh cache: do not share parent session prefix-cache keys.
            prompt_cache: Arc::new(PromptCache::new()),
            instructions,
            prompt_prefix,
            components: self.components.clone(),
            cancel_token,
            config: self.config.clone(),
            memory_injection: None,
            agent_mode: crate::plan_mode::AgentMode::Default,
            session_id,
            allowed_tool_names: None,
            is_subagent: true,
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
        let failed = text.starts_with("Subagent failed:");
        emit_subagent_transcript(
            &parent_event_tx,
            &invocation_id,
            SubagentTranscriptItem {
                kind: SubagentTranscriptKind::Text,
                title: "Final response".to_string(),
                detail: Some(one_line(&text)),
                ok: Some(!failed),
                invocation: None,
                result: None,
                text: Some(text.clone()),
                status: Some(if failed { "failed" } else { "done" }.to_string()),
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
        let task_cancel = parent_cancel.unwrap_or_default();
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

        // Resolve model provider based on options.
        let (resolved_provider, resolved_model) = self.resolve_model(&options);
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

        // Subagents run in yolo mode with all parent tools except nested spawners.
        let tool_executor = match build_subagent_executor(&executor, options.path_deny.as_deref()) {
            Ok(ex) => ex,
            Err(err) => {
                return Ok(helpers::ok(
                    invocation_id,
                    json!({"error": format!("failed to build subagent executor: {err:#}") }),
                ));
            }
        };

        tokio::spawn(async move {
            let (mut messages, event_tx, _approval_handle, resolver) =
                Self::build_subagent_context_static(
                    &parent_invocation_id,
                    &prompt,
                    &description,
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
                skill_pools: Arc::new(std::sync::Mutex::new(Vec::new())),
                active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
                prompt_cache: Arc::new(PromptCache::new()),
                instructions,
                prompt_prefix,
                components,
                cancel_token,
                config: Arc::new(std::sync::RwLock::new(config_snapshot)),
                memory_injection: None,
                session_id,
                agent_mode: crate::plan_mode::AgentMode::Default,
                allowed_tool_names: None,
                is_subagent: true,
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
            let failed = output.starts_with("Background subagent failed:");
            emit_subagent_transcript(
                &parent_event_tx,
                &parent_invocation_id,
                SubagentTranscriptItem {
                    kind: SubagentTranscriptKind::Text,
                    title: "Final response".to_string(),
                    detail: Some(one_line(&output)),
                    ok: Some(!failed),
                    invocation: None,
                    result: None,
                    text: Some(output.clone()),
                    status: Some(if failed { "failed" } else { "done" }.to_string()),
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

    /// Resolves a model provider and name for the subagent.
    ///
    /// If `options.model` is set, the main agent's provider is reused with the
    /// requested model name. Otherwise the main agent's model is used.
    fn resolve_model(&self, options: &SubagentOptions) -> (Arc<dyn ModelProvider>, String) {
        let (provider, model) = self.main_model();
        if let Some(override_model) = options.model.as_deref() {
            (provider, override_model.to_string())
        } else {
            (provider, model)
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
            parent_event_tx,
        )
    }

    fn build_subagent_context_static(
        parent_invocation_id: &str,
        prompt: &str,
        description: &Option<String>,
        parent_event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> (
        Vec<ModelMessage>,
        tokio::sync::mpsc::UnboundedSender<AgentEvent>,
        tokio::task::JoinHandle<()>,
        ApprovalResolver,
    ) {
        let workflow = "\
Workflow:\n\
1. Inspect with the cheapest tools first (overview/search → targeted read).\n\
2. Prefer project-relative paths; batch independent read-only calls when possible.\n\
3. Keep edits narrow; verify with the smallest relevant command when writes are needed.\n\
4. If a tool fails, adapt once using the error — do not thrash the same call.\n\
5. Observation budget: tool outputs may be truncated; request ranges/results explicitly.\n\
6. When done, report paths, key diffs, and findings — not walls of file contents.";
        let system = if let Some(desc) = description {
            format!(
                "You are a subagent worker for NAVI. Execute the assigned task autonomously \
                 using all available tools.\n\n\
                 Context: {desc}\n\n{workflow}\n\n\
                 Be concise and deliver the result."
            )
        } else {
            format!(
                "You are a subagent worker for NAVI. Execute the assigned task autonomously \
                 using all available tools.\n\n{workflow}\n\n\
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
                    // Subagents run in yolo mode; auto-approve any residual approval events.
                    resolver_bg.resolve(ApprovalDecision::Approved { id: req.id.clone() });
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
            invocation: Some(invocation.clone()),
            result: None,
            text: None,
            status: None,
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
            invocation: None,
            result: Some(result.clone()),
            text: None,
            status: None,
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
            model: None,
            path_deny: None,
        }
    }
}

/// Build a yolo-mode executor fork for a subagent.
///
/// The fork inherits all tools from the parent executor except `subagent` and
/// `workflow` (to prevent recursive spawn storms). An optional `path_deny` list
/// is applied as a workflow write-path deny scope with universal write_allow.
fn build_subagent_executor(
    executor: &crate::tool::ToolExecutor,
    path_deny: Option<&[String]>,
) -> Result<Arc<crate::tool::ToolExecutor>> {
    let policy = build_subagent_policy(executor.policy(), path_deny)?;
    let mut allowed: Vec<String> = executor
        .tool_names()
        .into_iter()
        .filter(|name| !NESTED_AGENT_TOOLS.contains(&name.as_str()))
        .collect();
    allowed.sort();
    allowed.dedup();
    Ok(Arc::new(
        executor.fork_with_policy_and_tools(policy, &allowed),
    ))
}

fn build_subagent_policy(
    base: &crate::security::SecurityPolicy,
    path_deny: Option<&[String]>,
) -> Result<crate::security::SecurityPolicy> {
    let mut config = base.config().clone();
    config.permission_mode = crate::config::PermissionMode::Yolo;
    let mut policy = crate::security::SecurityPolicy::new(
        base.project_root().to_path_buf(),
        base.data_dir().to_path_buf(),
        config,
    )?;
    if let Some(deny) = path_deny.filter(|d| !d.is_empty()) {
        policy = policy.with_write_scope(crate::security::WritePathScope {
            write_allow: vec!["**".into()],
            path_deny: deny.to_vec(),
            create_files: true,
            create_dirs: true,
        });
    }
    Ok(policy)
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
            model: Some("gpt-4".to_string()),
            path_deny: Some(vec!["secrets/".to_string()]),
        };
        let json = serde_json::to_value(&opts).unwrap();
        let deserialized: SubagentOptions = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.model, Some("gpt-4".to_string()));
        assert_eq!(
            deserialized.path_deny.as_deref(),
            Some(["secrets/".to_string()].as_slice())
        );
    }

    #[test]
    fn subagent_options_default_is_empty() {
        let opts = SubagentOptions::default();
        assert!(opts.model.is_none());
        assert!(opts.path_deny.is_none());
    }

    #[test]
    fn subagent_options_serde_missing_fields_default_correctly() {
        let json = json!({});
        let opts: SubagentOptions = serde_json::from_value(json).unwrap();
        assert!(opts.model.is_none());
        assert!(opts.path_deny.is_none());
    }

    #[test]
    fn subagent_options_serde_path_deny_only() {
        let json = json!({"path_deny": ["secrets/"]});
        let opts: SubagentOptions = serde_json::from_value(json).unwrap();
        assert_eq!(
            opts.path_deny.as_deref(),
            Some(["secrets/".to_string()].as_slice())
        );
    }

    #[test]
    fn schema_allows_model_and_path_deny() {
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
            RuntimeComponents::default(),
        );
        let schema = tool.definition().input_schema;
        let validator = jsonschema::validator_for(&schema).expect("compile schema");

        let valid = json!({
            "prompt": "list files",
            "description": "collect",
            "options": {
                "model": "claude-sonnet",
                "path_deny": ["secrets/"]
            }
        });
        let errors: Vec<String> = validator
            .iter_errors(&valid)
            .map(|e| e.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "valid options must pass schema: {errors:?}"
        );

        let invalid = json!({
            "prompt": "list files",
            "options": {
                "tools": ["read_file"]
            }
        });
        let errors: Vec<String> = validator
            .iter_errors(&invalid)
            .map(|e| e.to_string())
            .collect();
        assert!(!errors.is_empty(), "removed options must fail schema");
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
    fn subagent_executor_strips_nested_tools() {
        struct NoopProvider;
        impl ModelProvider for NoopProvider {
            fn stream(&self, _req: crate::model::ModelRequest) -> crate::model::ModelStream {
                Box::pin(futures_util::stream::empty())
            }
        }

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
        let provider = Arc::new(RwLock::new(Arc::new(NoopProvider) as Arc<dyn ModelProvider>));
        let mut executor = crate::tool::ToolExecutor::new(policy);
        executor.register_tool(Arc::new(crate::tool::builtin::SubagentTool::new(
            std::sync::Weak::new(),
            provider,
            temp.path().to_path_buf(),
            temp.path()
                .parent()
                .unwrap_or(temp.path())
                .join("navi-test-data-subagent"),
            Arc::new(RwLock::new("test".into())),
            HarnessConfig::default(),
            Arc::new(RwLock::new(NaviConfig::default())),
            RuntimeComponents::default(),
        )));
        // Simulated names to verify filtering.
        let names: Vec<String> = executor.tool_names();
        let forked = build_subagent_executor(&executor, None).unwrap();
        let forked_names = forked.tool_names();
        assert!(!forked_names.contains(&"subagent".to_string()));
        assert!(!forked_names.contains(&"workflow".to_string()));
        // Other registered names remain.
        for name in names
            .iter()
            .filter(|n| !NESTED_AGENT_TOOLS.contains(&n.as_str()))
        {
            assert!(forked_names.contains(name), "{name} should be inherited");
        }
    }

    #[test]
    fn subagent_policy_is_yolo() {
        let temp = tempfile::tempdir().unwrap();
        let base = crate::security::SecurityPolicy::new(
            temp.path().to_path_buf(),
            temp.path()
                .parent()
                .unwrap_or(temp.path())
                .join("navi-test-data-subagent"),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let policy = build_subagent_policy(&base, None).unwrap();
        assert!(matches!(
            policy.config().permission_mode,
            crate::config::PermissionMode::Yolo
        ));
    }

    #[test]
    fn subagents_get_distinct_provider_session_ids() {
        let first = subagent_session_id();
        let second = subagent_session_id();

        assert!(first.starts_with("subagent-session-"));
        assert_ne!(first, second);
    }
}
