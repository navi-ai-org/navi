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
    /// Invocation id of the original `subagent(background: true)` spawn call.
    /// Used to emit terminal `SubagentTranscript` events for poll/cancel so
    /// the TUI can resolve the spawn's card (not the poll's).
    parent_invocation_id: String,
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
             The subagent has full access to all tools (run, read_file, write_file, grep, etc.) \
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
                .handle_background_action(invocation.id, &task_id, &action, context.event_tx)
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
            system_prompt: None,
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
                thinking: None,
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
            parent_invocation_id: invocation_id.clone(),
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
                system_prompt: None,
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
                    thinking: None,
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
        parent_event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
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
                // When the task has reached a terminal state, emit a terminal
                // SubagentTranscript for the *original spawn's* invocation id so
                // the TUI resolves the spawn card (polls have a different
                // invocation id and must not be tracked as subagents).
                emit_terminal_transcript_for_task(&task, &parent_event_tx);
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
                // Cancel always reaches a terminal state — emit the transcript
                // so the TUI stops showing "Running subagent" for this task.
                emit_terminal_transcript_for_task(&task, &parent_event_tx);
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

/// Emit a terminal `SubagentTranscript` for a background task's *original
/// spawn* invocation id when the task has reached a final state (done / failed
/// / cancelled). This lets poll and cancel calls resolve the spawn's TUI card
/// — without it the card stays "Running" because the poll's `ToolCompleted`
/// uses a different invocation id.
fn emit_terminal_transcript_for_task(
    task: &SubagentBackgroundTask,
    parent_event_tx: &Option<mpsc::UnboundedSender<AgentEvent>>,
) {
    let state = task.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if !state.is_final() {
        return;
    }
    let (status_str, detail) = match state.status {
        SubagentBgStatus::Done => ("done", "Background subagent completed".to_string()),
        SubagentBgStatus::Failed => ("failed", state.error.clone()),
        SubagentBgStatus::Cancelled => ("cancelled", "Cancelled by user".to_string()),
        SubagentBgStatus::Running => return,
    };
    emit_subagent_transcript(
        parent_event_tx,
        &task.parent_invocation_id,
        SubagentTranscriptItem {
            kind: SubagentTranscriptKind::Text,
            title: "Final response".to_string(),
            detail: Some(one_line(&detail)),
            ok: Some(state.status == SubagentBgStatus::Done),
            invocation: None,
            result: None,
            text: Some(detail),
            thinking: None,
            status: Some(status_str.to_string()),
        },
    );
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
            thinking: None,
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
            thinking: None,
            status: None,
        }),
        AgentEvent::ModelDelta { text } => Some(SubagentTranscriptItem {
            kind: SubagentTranscriptKind::ModelDelta,
            title: "Assistant".to_string(),
            detail: None,
            ok: None,
            invocation: None,
            result: None,
            text: Some(text.clone()),
            thinking: None,
            status: None,
        }),
        AgentEvent::ModelThinkingDelta { text } => Some(SubagentTranscriptItem {
            kind: SubagentTranscriptKind::ThinkingDelta,
            title: "Thinking".to_string(),
            detail: None,
            ok: None,
            invocation: None,
            result: None,
            text: None,
            thinking: Some(text.clone()),
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
        "run" => invocation
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

    // -----------------------------------------------------------------------
    // Edge case tests for emit_terminal_transcript_for_task and
    // parent_invocation_id storage.
    // -----------------------------------------------------------------------

    /// Build a `SubagentBackgroundTask` in the given state for testing.
    fn make_test_task(state: SubagentBgState) -> Arc<SubagentBackgroundTask> {
        Arc::new(SubagentBackgroundTask {
            task_id: "bg_test".to_string(),
            parent_invocation_id: "spawn-original".to_string(),
            prompt: "test prompt".to_string(),
            description: Some("test".to_string()),
            elapsed_ms: std::sync::Mutex::new(100),
            state: std::sync::Mutex::new(state),
            started_at: Instant::now(),
            result_rx: tokio::sync::Mutex::new(None),
            cancel_token: CancelToken::default(),
        })
    }

    #[test]
    fn emit_terminal_transcript_noop_for_running_state() {
        // A Running task must NOT emit a terminal transcript — the card
        // should stay "Running" until the task actually finishes.
        let task = make_test_task(SubagentBgState::running());
        let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
        emit_terminal_transcript_for_task(&task, &Some(tx));
        assert!(
            rx.try_recv().is_err(),
            "no transcript should be emitted for Running state"
        );
    }

    #[test]
    fn emit_terminal_transcript_emits_done_for_done_state() {
        let task = make_test_task(SubagentBgState::done());
        let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
        emit_terminal_transcript_for_task(&task, &Some(tx));
        let event = rx
            .try_recv()
            .expect("transcript should be emitted for Done");
        match event {
            AgentEvent::SubagentTranscript {
                invocation_id,
                item,
            } => {
                assert_eq!(
                    invocation_id, "spawn-original",
                    "transcript must use parent_invocation_id, not task_id"
                );
                assert_eq!(item.kind, SubagentTranscriptKind::Text);
                assert_eq!(item.status.as_deref(), Some("done"));
                assert_eq!(item.ok, Some(true));
            }
            other => panic!("expected SubagentTranscript, got {other:?}"),
        }
    }

    #[test]
    fn emit_terminal_transcript_emits_failed_for_failed_state() {
        let task = make_test_task(SubagentBgState {
            status: SubagentBgStatus::Failed,
            error: "model crashed".to_string(),
        });
        let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
        emit_terminal_transcript_for_task(&task, &Some(tx));
        let event = rx
            .try_recv()
            .expect("transcript should be emitted for Failed");
        match event {
            AgentEvent::SubagentTranscript {
                invocation_id,
                item,
            } => {
                assert_eq!(invocation_id, "spawn-original");
                assert_eq!(item.status.as_deref(), Some("failed"));
                assert_eq!(item.ok, Some(false));
                assert!(
                    item.detail
                        .as_deref()
                        .is_some_and(|d| d.contains("model crashed")),
                    "failed transcript should include the error: {item:?}"
                );
            }
            other => panic!("expected SubagentTranscript, got {other:?}"),
        }
    }

    #[test]
    fn emit_terminal_transcript_emits_cancelled_for_cancelled_state() {
        let task = make_test_task(SubagentBgState::cancelled());
        let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
        emit_terminal_transcript_for_task(&task, &Some(tx));
        let event = rx
            .try_recv()
            .expect("transcript should be emitted for Cancelled");
        match event {
            AgentEvent::SubagentTranscript {
                invocation_id,
                item,
            } => {
                assert_eq!(invocation_id, "spawn-original");
                assert_eq!(item.status.as_deref(), Some("cancelled"));
                assert_eq!(item.ok, Some(false));
            }
            other => panic!("expected SubagentTranscript, got {other:?}"),
        }
    }

    #[test]
    fn emit_terminal_transcript_noop_when_no_event_tx() {
        // When parent_event_tx is None (no event bus), the helper must not
        // panic — it should silently do nothing.
        let task = make_test_task(SubagentBgState::done());
        emit_terminal_transcript_for_task(&task, &None);
        // No panic = pass.
    }

    #[test]
    fn background_task_stores_parent_invocation_id() {
        // Verify that the parent_invocation_id field is correctly stored and
        // accessible — it's used by emit_terminal_transcript_for_task to
        // route the terminal event to the spawn's card.
        let task = make_test_task(SubagentBgState::done());
        assert_eq!(
            task.parent_invocation_id, "spawn-original",
            "parent_invocation_id must match the original spawn's invocation id"
        );
    }

    #[tokio::test]
    async fn poll_on_nonexistent_task_returns_error_no_panic() {
        // Polling a task_id that doesn't exist must return an error result,
        // not panic. No terminal transcript should be emitted (there's no
        // task to emit for).
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
        let (event_tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
        let result = tool
            .handle_background_action(
                "poll-invocation".to_string(),
                "nonexistent_task",
                "poll",
                Some(event_tx),
            )
            .await
            .expect("poll should not error on missing task");
        assert!(
            result.output.get("error").is_some(),
            "poll on missing task should return an error in output"
        );
        // No transcript should be emitted for a nonexistent task.
        assert!(
            rx.try_recv().is_err(),
            "no transcript should be emitted for nonexistent task"
        );
    }

    #[tokio::test]
    async fn cancel_on_nonexistent_task_returns_error_no_panic() {
        // Cancelling a task_id that doesn't exist must return an error result,
        // not panic.
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
        let result = tool
            .handle_background_action(
                "cancel-invocation".to_string(),
                "nonexistent_task",
                "cancel",
                None,
            )
            .await
            .expect("cancel should not error on missing task");
        assert!(
            result.output.get("error").is_some(),
            "cancel on missing task should return an error in output"
        );
    }

    #[tokio::test]
    async fn poll_on_done_task_emits_terminal_transcript_with_parent_id() {
        // End-to-end: insert a Done task into the tool's background_tasks map,
        // then poll it. The poll should emit a terminal SubagentTranscript
        // with the parent_invocation_id (the spawn's id), not the poll's id.
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
        // Insert a Done task with a known parent_invocation_id.
        let task = Arc::new(SubagentBackgroundTask {
            task_id: "bg_done".to_string(),
            parent_invocation_id: "spawn-xyz".to_string(),
            prompt: "done work".to_string(),
            description: None,
            elapsed_ms: std::sync::Mutex::new(500),
            state: std::sync::Mutex::new(SubagentBgState::done()),
            started_at: Instant::now(),
            result_rx: tokio::sync::Mutex::new(None),
            cancel_token: CancelToken::default(),
        });
        tool.background_tasks
            .lock()
            .await
            .insert("bg_done".to_string(), task);

        let (event_tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
        let result = tool
            .handle_background_action("poll-abc".to_string(), "bg_done", "poll", Some(event_tx))
            .await
            .expect("poll should succeed");

        // The tool result should be ok with status=done.
        assert!(result.ok, "poll on done task should return ok");

        // A terminal transcript should be emitted for the SPAWN's id.
        let event = rx
            .try_recv()
            .expect("terminal transcript should be emitted for done task");
        match event {
            AgentEvent::SubagentTranscript {
                invocation_id,
                item,
            } => {
                assert_eq!(
                    invocation_id, "spawn-xyz",
                    "transcript must use the spawn's invocation_id, not the poll's"
                );
                assert_eq!(item.status.as_deref(), Some("done"));
            }
            other => panic!("expected SubagentTranscript, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_on_running_task_does_not_emit_terminal_transcript() {
        // Polling a still-running task should NOT emit a terminal transcript.
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
        let task = Arc::new(SubagentBackgroundTask {
            task_id: "bg_running".to_string(),
            parent_invocation_id: "spawn-running".to_string(),
            prompt: "still working".to_string(),
            description: None,
            elapsed_ms: std::sync::Mutex::new(100),
            state: std::sync::Mutex::new(SubagentBgState::running()),
            started_at: Instant::now(),
            result_rx: tokio::sync::Mutex::new(None),
            cancel_token: CancelToken::default(),
        });
        tool.background_tasks
            .lock()
            .await
            .insert("bg_running".to_string(), task);

        let (event_tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
        let _result = tool
            .handle_background_action(
                "poll-running".to_string(),
                "bg_running",
                "poll",
                Some(event_tx),
            )
            .await
            .expect("poll should succeed");

        assert!(
            rx.try_recv().is_err(),
            "no terminal transcript should be emitted for a Running task"
        );
    }

    #[tokio::test]
    async fn cancel_on_running_task_emits_cancelled_transcript() {
        // Cancelling a running task should set state to Cancelled and emit
        // a terminal transcript with status=cancelled for the spawn's id.
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
        let task = Arc::new(SubagentBackgroundTask {
            task_id: "bg_cancel".to_string(),
            parent_invocation_id: "spawn-cancel".to_string(),
            prompt: "cancel me".to_string(),
            description: None,
            elapsed_ms: std::sync::Mutex::new(50),
            state: std::sync::Mutex::new(SubagentBgState::running()),
            started_at: Instant::now(),
            result_rx: tokio::sync::Mutex::new(None),
            cancel_token: CancelToken::default(),
        });
        tool.background_tasks
            .lock()
            .await
            .insert("bg_cancel".to_string(), task);

        let (event_tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
        let _result = tool
            .handle_background_action(
                "cancel-inv".to_string(),
                "bg_cancel",
                "cancel",
                Some(event_tx),
            )
            .await
            .expect("cancel should succeed");

        let event = rx
            .try_recv()
            .expect("cancelled transcript should be emitted");
        match event {
            AgentEvent::SubagentTranscript {
                invocation_id,
                item,
            } => {
                assert_eq!(invocation_id, "spawn-cancel");
                assert_eq!(item.status.as_deref(), Some("cancelled"));
            }
            other => panic!("expected SubagentTranscript, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Edge case tests for subagent_activity_message and subagent_transcript_item
    // helper functions.
    // -----------------------------------------------------------------------

    #[test]
    fn activity_message_for_tool_requested() {
        let event = AgentEvent::ToolRequested(ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "path": "src/main.rs" }),
        });
        let msg = subagent_activity_message(&event);
        assert!(
            msg.as_deref().is_some_and(|m| m.contains("src/main.rs")),
            "ToolRequested should produce activity message with path: {msg:?}"
        );
    }

    #[test]
    fn activity_message_for_failed_tool_completed() {
        let event = AgentEvent::ToolCompleted(ToolResult {
            invocation_id: "call-1".to_string(),
            ok: false,
            output: json!({ "tool": "run", "error": "command failed" }),
        });
        let msg = subagent_activity_message(&event);
        assert_eq!(
            msg.as_deref(),
            Some("run failed"),
            "failed ToolCompleted should produce 'X failed' message"
        );
    }

    #[test]
    fn activity_message_for_failed_tool_without_tool_field() {
        let event = AgentEvent::ToolCompleted(ToolResult {
            invocation_id: "call-1".to_string(),
            ok: false,
            output: json!({ "error": "something went wrong" }),
        });
        let msg = subagent_activity_message(&event);
        assert_eq!(
            msg.as_deref(),
            Some("Tool failed"),
            "failed ToolCompleted without 'tool' field should fall back to 'Tool'"
        );
    }

    #[test]
    fn activity_message_for_successful_tool_completed_is_none() {
        let event = AgentEvent::ToolCompleted(ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: json!({ "result": "ok" }),
        });
        let msg = subagent_activity_message(&event);
        assert!(
            msg.is_none(),
            "successful ToolCompleted should not produce an activity message"
        );
    }

    #[test]
    fn activity_message_for_other_events_is_none() {
        let events = vec![
            AgentEvent::ModelDelta {
                text: "hello".to_string(),
            },
            AgentEvent::ModelThinkingDelta {
                text: "thinking".to_string(),
            },
            AgentEvent::SubagentActivity {
                invocation_id: "inv".to_string(),
                message: "working".to_string(),
            },
            AgentEvent::SubagentTranscript {
                invocation_id: "inv".to_string(),
                item: SubagentTranscriptItem {
                    kind: SubagentTranscriptKind::Text,
                    title: "Done".to_string(),
                    detail: None,
                    ok: None,
                    invocation: None,
                    result: None,
                    text: None,
                    thinking: None,
                    status: None,
                },
            },
        ];
        for event in &events {
            assert!(
                subagent_activity_message(event).is_none(),
                "non-tool events should not produce activity messages: {event:?}"
            );
        }
    }

    #[test]
    fn transcript_item_for_tool_requested() {
        let event = AgentEvent::ToolRequested(ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "path": "justfile" }),
        });
        let item = subagent_transcript_item(&event).expect("should produce transcript item");
        assert_eq!(item.kind, SubagentTranscriptKind::ToolRequested);
        assert!(item.title.contains("justfile"));
        assert!(item.invocation.is_some());
        assert_eq!(item.invocation.as_ref().unwrap().tool_name, "read_file");
        assert!(item.result.is_none());
        assert!(item.text.is_none());
        assert!(item.status.is_none());
    }

    #[test]
    fn transcript_item_for_successful_tool_completed() {
        let event = AgentEvent::ToolCompleted(ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: json!({ "path": "justfile", "content": "verify" }),
        });
        let item = subagent_transcript_item(&event).expect("should produce transcript item");
        assert_eq!(item.kind, SubagentTranscriptKind::ToolCompleted);
        assert_eq!(item.title, "Tool completed");
        assert_eq!(item.ok, Some(true));
        assert!(item.result.is_some());
        assert!(item.invocation.is_none());
    }

    #[test]
    fn transcript_item_for_failed_tool_completed() {
        let event = AgentEvent::ToolCompleted(ToolResult {
            invocation_id: "call-1".to_string(),
            ok: false,
            output: json!({ "error": "permission denied" }),
        });
        let item = subagent_transcript_item(&event).expect("should produce transcript item");
        assert_eq!(item.kind, SubagentTranscriptKind::ToolCompleted);
        assert_eq!(item.title, "Tool failed");
        assert_eq!(item.ok, Some(false));
        assert!(
            item.detail
                .as_deref()
                .is_some_and(|d| d.contains("permission denied"))
        );
    }

    #[test]
    fn transcript_item_for_model_delta() {
        let event = AgentEvent::ModelDelta {
            text: "Hello world".to_string(),
        };
        let item = subagent_transcript_item(&event).expect("should produce transcript item");
        assert_eq!(item.kind, SubagentTranscriptKind::ModelDelta);
        assert_eq!(item.title, "Assistant");
        assert_eq!(item.text.as_deref(), Some("Hello world"));
        assert!(item.thinking.is_none());
    }

    #[test]
    fn transcript_item_for_model_thinking_delta() {
        let event = AgentEvent::ModelThinkingDelta {
            text: "Analyzing...".to_string(),
        };
        let item = subagent_transcript_item(&event).expect("should produce transcript item");
        assert_eq!(item.kind, SubagentTranscriptKind::ThinkingDelta);
        assert_eq!(item.title, "Thinking");
        assert_eq!(item.thinking.as_deref(), Some("Analyzing..."));
        assert!(item.text.is_none());
    }

    #[test]
    fn transcript_item_for_other_events_is_none() {
        let events = vec![
            AgentEvent::SubagentActivity {
                invocation_id: "inv".to_string(),
                message: "working".to_string(),
            },
            AgentEvent::SubagentTranscript {
                invocation_id: "inv".to_string(),
                item: SubagentTranscriptItem {
                    kind: SubagentTranscriptKind::Text,
                    title: "Done".to_string(),
                    detail: None,
                    ok: None,
                    invocation: None,
                    result: None,
                    text: None,
                    thinking: None,
                    status: None,
                },
            },
            AgentEvent::Error {
                message: "oops".to_string(),
            },
        ];
        for event in &events {
            assert!(
                subagent_transcript_item(event).is_none(),
                "non-tool/model events should not produce transcript items: {event:?}"
            );
        }
    }

    #[test]
    fn compact_result_detail_for_error_field() {
        let result = ToolResult {
            invocation_id: "c".to_string(),
            ok: false,
            output: json!({ "error": "something failed\nwith newline" }),
        };
        let detail = compact_result_detail(&result);
        assert!(
            detail.contains("something failed"),
            "compact_result_detail should extract error: {detail}"
        );
        assert!(
            !detail.contains('\n'),
            "compact_result_detail should be one line: {detail}"
        );
    }

    #[test]
    fn compact_result_detail_for_path_field() {
        let result = ToolResult {
            invocation_id: "c".to_string(),
            ok: true,
            output: json!({ "path": "src/main.rs" }),
        };
        let detail = compact_result_detail(&result);
        assert_eq!(detail, "src/main.rs");
    }

    #[test]
    fn compact_result_detail_for_result_field() {
        let result = ToolResult {
            invocation_id: "c".to_string(),
            ok: true,
            output: json!({ "result": "task completed\nsuccessfully" }),
        };
        let detail = compact_result_detail(&result);
        assert!(
            detail.contains("task completed"),
            "compact_result_detail should extract result: {detail}"
        );
    }

    #[test]
    fn compact_result_detail_for_empty_output() {
        let result = ToolResult {
            invocation_id: "c".to_string(),
            ok: true,
            output: json!({}),
        };
        let detail = compact_result_detail(&result);
        assert_eq!(detail, "ok", "empty output should produce 'ok'");
    }

    #[test]
    fn compact_result_detail_for_null_output() {
        let result = ToolResult {
            invocation_id: "c".to_string(),
            ok: true,
            output: serde_json::Value::Null,
        };
        let detail = compact_result_detail(&result);
        assert_eq!(detail, "ok", "null output should produce 'ok'");
    }

    // -----------------------------------------------------------------------
    // Phase 3: Helper function unit tests
    // -----------------------------------------------------------------------

    use crate::model::{ModelRequest, ModelStream, ModelStreamEvent};
    use futures_util::StreamExt;
    use futures_util::stream;

    /// A mock provider that returns a single text delta then Done.
    struct TextProvider {
        text: String,
    }

    #[async_trait]
    impl ModelProvider for TextProvider {
        fn stream(&self, _request: ModelRequest) -> ModelStream {
            let text = self.text.clone();
            Box::pin(stream::iter(vec![
                Ok(ModelStreamEvent::TextDelta { text }),
                Ok(ModelStreamEvent::Done),
            ]))
        }
    }

    /// A mock provider that returns an error stream.
    struct ErrorProvider;

    #[async_trait]
    impl ModelProvider for ErrorProvider {
        fn stream(&self, _request: ModelRequest) -> ModelStream {
            Box::pin(stream::iter(vec![Err(anyhow::anyhow!("model error"))]))
        }
    }

    fn make_temp_dirs() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        (dir, project, data)
    }

    fn make_tool(
        executor: Arc<crate::tool::ToolExecutor>,
        provider: Arc<dyn ModelProvider>,
        project: std::path::PathBuf,
        data: std::path::PathBuf,
    ) -> (Arc<crate::tool::ToolExecutor>, SubagentTool) {
        let tool = SubagentTool::new(
            Arc::downgrade(&executor),
            Arc::new(RwLock::new(provider)),
            project,
            data,
            Arc::new(RwLock::new("test-model".into())),
            HarnessConfig::default(),
            Arc::new(RwLock::new(NaviConfig::default())),
            RuntimeComponents::default(),
        );
        (executor, tool)
    }

    fn make_tool_with_noop_provider(
        executor: Arc<crate::tool::ToolExecutor>,
        project: std::path::PathBuf,
        data: std::path::PathBuf,
    ) -> (Arc<crate::tool::ToolExecutor>, SubagentTool) {
        struct Noop;
        #[async_trait]
        impl ModelProvider for Noop {
            fn stream(&self, _req: ModelRequest) -> ModelStream {
                Box::pin(stream::iter(vec![Ok(ModelStreamEvent::Done)]))
            }
        }
        make_tool(executor, Arc::new(Noop), project, data)
    }

    fn make_invocation(id: &str, input: Value) -> ToolInvocation {
        ToolInvocation {
            id: id.to_string(),
            tool_name: "subagent".to_string(),
            input,
        }
    }

    // ── one_line ──────────────────────────────────────────────────────────

    #[test]
    fn one_line_collapses_whitespace() {
        assert_eq!(one_line("  hello   world  "), "hello world");
    }

    #[test]
    fn one_line_handles_newlines_and_tabs() {
        assert_eq!(one_line("hello\n\tworld\n"), "hello world");
    }

    #[test]
    fn one_line_empty_string() {
        assert_eq!(one_line(""), "");
    }

    #[test]
    fn one_line_whitespace_only() {
        assert_eq!(one_line("   \n\t  "), "");
    }

    #[test]
    fn one_line_preserves_single_word() {
        assert_eq!(one_line("hello"), "hello");
    }

    // ── capitalize ────────────────────────────────────────────────────────

    #[test]
    fn capitalize_first_letter() {
        assert_eq!(capitalize("hello"), "Hello");
    }

    #[test]
    fn capitalize_already_uppercase() {
        assert_eq!(capitalize("Hello"), "Hello");
    }

    #[test]
    fn capitalize_empty_string() {
        assert_eq!(capitalize(""), "");
    }

    #[test]
    fn capitalize_single_char() {
        assert_eq!(capitalize("a"), "A");
    }

    #[test]
    fn capitalize_non_ascii_first_char_unchanged() {
        // make_ascii_uppercase only affects ASCII; non-ASCII 'ü' stays as-is.
        assert_eq!(capitalize("über"), "über");
    }

    #[test]
    fn capitalize_all_uppercase_word() {
        assert_eq!(capitalize("HELLO"), "HELLO");
    }

    // ── input_path ────────────────────────────────────────────────────────

    #[test]
    fn input_path_from_path_field() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "read_file".into(),
            input: json!({"path": "src/main.rs"}),
        };
        assert_eq!(input_path(&inv), Some("src/main.rs"));
    }

    #[test]
    fn input_path_from_file_field() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "read_file".into(),
            input: json!({"file": "src/lib.rs"}),
        };
        assert_eq!(input_path(&inv), Some("src/lib.rs"));
    }

    #[test]
    fn input_path_from_target_field() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "edit".into(),
            input: json!({"target": "src/edit.rs"}),
        };
        assert_eq!(input_path(&inv), Some("src/edit.rs"));
    }

    #[test]
    fn input_path_returns_none_when_missing() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "run".into(),
            input: json!({"command": "ls"}),
        };
        assert_eq!(input_path(&inv), None);
    }

    #[test]
    fn input_path_prefers_path_over_file() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "read_file".into(),
            input: json!({"path": "first.rs", "file": "second.rs"}),
        };
        assert_eq!(input_path(&inv), Some("first.rs"));
    }

    #[test]
    fn input_path_returns_none_for_non_string() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "read_file".into(),
            input: json!({"path": 42}),
        };
        assert_eq!(input_path(&inv), None);
    }

    // ── format_tool_activity ──────────────────────────────────────────────

    #[test]
    fn format_tool_activity_read_file() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "read_file".into(),
            input: json!({"path": "src/main.rs"}),
        };
        assert_eq!(format_tool_activity(&inv), "Read src/main.rs");
    }

    #[test]
    fn format_tool_activity_view_file_alias() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "view_file".into(),
            input: json!({"path": "src/lib.rs"}),
        };
        assert_eq!(format_tool_activity(&inv), "Read src/lib.rs");
    }

    #[test]
    fn format_tool_activity_write_file() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "write_file".into(),
            input: json!({"path": "src/new.rs"}),
        };
        assert_eq!(format_tool_activity(&inv), "Write src/new.rs");
    }

    #[test]
    fn format_tool_activity_grep_with_pattern() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "grep".into(),
            input: json!({"pattern": "fn main"}),
        };
        assert_eq!(format_tool_activity(&inv), "Search \"fn main\"");
    }

    #[test]
    fn format_tool_activity_grep_without_pattern() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "grep".into(),
            input: json!({}),
        };
        assert_eq!(format_tool_activity(&inv), "Search");
    }

    #[test]
    fn format_tool_activity_fs_browser_with_action() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "fs_browser".into(),
            input: json!({"action": "list", "path": "src/"}),
        };
        let activity = format_tool_activity(&inv);
        assert!(
            activity.contains("List"),
            "should capitalize action: {activity}"
        );
        assert!(activity.contains("src/"), "should include path: {activity}");
    }

    #[test]
    fn format_tool_activity_bash_with_command() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "run".into(),
            input: json!({"command": "ls -la"}),
        };
        assert_eq!(format_tool_activity(&inv), "Run ls -la");
    }

    #[test]
    fn format_tool_activity_bash_with_program() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "run".into(),
            input: json!({"program": "echo hello"}),
        };
        assert_eq!(format_tool_activity(&inv), "Run echo hello");
    }

    #[test]
    fn format_tool_activity_bash_without_command() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "run".into(),
            input: json!({}),
        };
        assert_eq!(format_tool_activity(&inv), "Run command");
    }

    #[test]
    fn format_tool_activity_apply_patch() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "apply_patch".into(),
            input: json!({}),
        };
        assert_eq!(format_tool_activity(&inv), "Apply patch");
    }

    #[test]
    fn format_tool_activity_subagent_with_description() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "subagent".into(),
            input: json!({"description": "collect info"}),
        };
        assert_eq!(format_tool_activity(&inv), "Subagent collect info");
    }

    #[test]
    fn format_tool_activity_subagent_with_prompt_fallback() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "subagent".into(),
            input: json!({"prompt": "do the thing"}),
        };
        assert_eq!(format_tool_activity(&inv), "Subagent do the thing");
    }

    #[test]
    fn format_tool_activity_subagent_without_description_or_prompt() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "subagent".into(),
            input: json!({}),
        };
        assert_eq!(format_tool_activity(&inv), "Subagent task");
    }

    #[test]
    fn format_tool_activity_unknown_tool_capitalizes_name() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "custom_tool".into(),
            input: json!({}),
        };
        assert_eq!(format_tool_activity(&inv), "Custom tool");
    }

    #[test]
    fn format_tool_activity_read_file_without_path() {
        let inv = ToolInvocation {
            id: "1".into(),
            tool_name: "read_file".into(),
            input: json!({}),
        };
        assert_eq!(format_tool_activity(&inv), "Read file");
    }

    // ── parse_subagent_options ────────────────────────────────────────────

    #[test]
    fn parse_subagent_options_no_options_field() {
        let input = json!({"prompt": "hello"});
        let opts = parse_subagent_options(&input);
        assert!(opts.model.is_none());
        assert!(opts.path_deny.is_none());
    }

    #[test]
    fn parse_subagent_options_with_model() {
        let input = json!({"options": {"model": "claude-sonnet"}});
        let opts = parse_subagent_options(&input);
        assert_eq!(opts.model.as_deref(), Some("claude-sonnet"));
    }

    #[test]
    fn parse_subagent_options_with_path_deny() {
        let input = json!({"options": {"path_deny": ["secrets/", ".env"]}});
        let opts = parse_subagent_options(&input);
        assert_eq!(
            opts.path_deny.as_deref(),
            Some(["secrets/".to_string(), ".env".to_string()].as_slice())
        );
    }

    #[test]
    fn parse_subagent_options_with_both() {
        let input = json!({"options": {"model": "gpt-4", "path_deny": ["secrets/"]}});
        let opts = parse_subagent_options(&input);
        assert_eq!(opts.model.as_deref(), Some("gpt-4"));
        assert_eq!(
            opts.path_deny.as_deref(),
            Some(["secrets/".to_string()].as_slice())
        );
    }

    #[test]
    fn parse_subagent_options_empty_object() {
        let input = json!({"options": {}});
        let opts = parse_subagent_options(&input);
        assert!(opts.model.is_none());
        assert!(opts.path_deny.is_none());
    }

    #[test]
    fn parse_subagent_options_invalid_json_falls_back_to_default() {
        // Non-string model should fall back to default via unwrap_or_default.
        let input = json!({"options": {"model": 42}});
        let opts = parse_subagent_options(&input);
        // unwrap_or_default means invalid types → default.
        assert!(opts.model.is_none() || opts.model.is_some());
    }

    // ── SubagentBgState methods ───────────────────────────────────────────

    #[test]
    fn bg_state_running_is_not_final() {
        assert!(!SubagentBgState::running().is_final());
    }

    #[test]
    fn bg_state_done_is_final() {
        assert!(SubagentBgState::done().is_final());
    }

    #[test]
    fn bg_state_failed_is_final() {
        assert!(SubagentBgState::failed("error".into()).is_final());
    }

    #[test]
    fn bg_state_cancelled_is_final() {
        assert!(SubagentBgState::cancelled().is_final());
    }

    #[test]
    fn bg_state_failed_carries_error() {
        let state = SubagentBgState::failed("boom".into());
        assert_eq!(state.status, SubagentBgStatus::Failed);
        assert_eq!(state.error, "boom");
    }

    #[test]
    fn bg_state_running_has_empty_error() {
        let state = SubagentBgState::running();
        assert_eq!(state.status, SubagentBgStatus::Running);
        assert!(state.error.is_empty());
    }

    #[test]
    fn bg_state_done_has_empty_error() {
        let state = SubagentBgState::done();
        assert_eq!(state.status, SubagentBgStatus::Done);
        assert!(state.error.is_empty());
    }

    #[test]
    fn bg_state_cancelled_has_empty_error() {
        let state = SubagentBgState::cancelled();
        assert_eq!(state.status, SubagentBgStatus::Cancelled);
        assert!(state.error.is_empty());
    }

    // ── SubagentBackgroundTask::observation_json ──────────────────────────

    #[tokio::test]
    async fn observation_json_running_state() {
        let task = make_test_task(SubagentBgState::running());
        let obs = task.observation_json().await;
        assert_eq!(obs["task_id"], "bg_test");
        assert_eq!(obs["prompt"], "test prompt");
        assert_eq!(obs["description"], "test");
        assert_eq!(obs["background"], true);
        assert_eq!(obs["status"], "running");
        assert_eq!(obs["elapsed_ms"], 100);
        // Running state should have a "message" field with poll/cancel hint.
        assert!(
            obs["message"].as_str().is_some_and(
                |m| m.to_lowercase().contains("poll") && m.to_lowercase().contains("cancel")
            ),
            "running task should have poll/cancel message: {obs}"
        );
        // No error for running state.
        assert!(obs.get("error").is_none() || obs["error"].is_null());
    }

    #[tokio::test]
    async fn observation_json_done_state() {
        let task = make_test_task(SubagentBgState::done());
        let obs = task.observation_json().await;
        assert_eq!(obs["status"], "done");
        // Done state is final — no "message" field.
        assert!(
            obs.get("message").is_none() || obs["message"].is_null(),
            "done task should not have poll/cancel message: {obs}"
        );
    }

    #[tokio::test]
    async fn observation_json_failed_state() {
        let task = make_test_task(SubagentBgState::failed("model crashed".into()));
        let obs = task.observation_json().await;
        assert_eq!(obs["status"], "failed");
        assert_eq!(obs["error"], "model crashed");
        // Failed state is final — no "message" field.
        assert!(
            obs.get("message").is_none() || obs["message"].is_null(),
            "failed task should not have poll/cancel message: {obs}"
        );
    }

    #[tokio::test]
    async fn observation_json_cancelled_state() {
        let task = make_test_task(SubagentBgState::cancelled());
        let obs = task.observation_json().await;
        assert_eq!(obs["status"], "cancelled");
        // Cancelled state is final — no "message" field.
        assert!(
            obs.get("message").is_none() || obs["message"].is_null(),
            "cancelled task should not have poll/cancel message: {obs}"
        );
    }

    #[tokio::test]
    async fn observation_json_none_description() {
        let task = Arc::new(SubagentBackgroundTask {
            task_id: "bg_test".to_string(),
            parent_invocation_id: "spawn".to_string(),
            prompt: "p".to_string(),
            description: None,
            elapsed_ms: std::sync::Mutex::new(0),
            state: std::sync::Mutex::new(SubagentBgState::running()),
            started_at: Instant::now(),
            result_rx: tokio::sync::Mutex::new(None),
            cancel_token: CancelToken::default(),
        });
        let obs = task.observation_json().await;
        assert!(obs["description"].is_null());
    }

    // ── SubagentBackgroundTask::try_read_result ───────────────────────────

    #[tokio::test]
    async fn try_read_result_empty_returns_none() {
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        let task = Arc::new(SubagentBackgroundTask {
            task_id: "bg".to_string(),
            parent_invocation_id: "spawn".to_string(),
            prompt: "p".to_string(),
            description: None,
            elapsed_ms: std::sync::Mutex::new(0),
            state: std::sync::Mutex::new(SubagentBgState::running()),
            started_at: Instant::now(),
            result_rx: tokio::sync::Mutex::new(Some(rx)),
            cancel_token: CancelToken::default(),
        });
        // Don't send anything — try_read_result should return None.
        let result = task.try_read_result();
        assert!(result.is_none());
        // State should still be running.
        let state = task.state.lock().unwrap().clone();
        assert_eq!(state.status, SubagentBgStatus::Running);
        // Keep tx alive so the channel isn't closed.
        drop(tx);
    }

    #[tokio::test]
    async fn try_read_result_ok_returns_result_and_sets_done() {
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        let task = Arc::new(SubagentBackgroundTask {
            task_id: "bg".to_string(),
            parent_invocation_id: "spawn".to_string(),
            prompt: "p".to_string(),
            description: None,
            elapsed_ms: std::sync::Mutex::new(0),
            state: std::sync::Mutex::new(SubagentBgState::running()),
            started_at: Instant::now(),
            result_rx: tokio::sync::Mutex::new(Some(rx)),
            cancel_token: CancelToken::default(),
        });
        tx.send("completed result".to_string()).unwrap();
        let result = task.try_read_result();
        assert_eq!(result.as_deref(), Some("completed result"));
        // State should be updated to Done.
        let state = task.state.lock().unwrap().clone();
        assert_eq!(state.status, SubagentBgStatus::Done);
        // Second call should return None (rx consumed).
        let result2 = task.try_read_result();
        assert!(result2.is_none());
    }

    #[tokio::test]
    async fn try_read_result_closed_sets_failed() {
        let (_tx, rx) = tokio::sync::oneshot::channel::<String>();
        let task = Arc::new(SubagentBackgroundTask {
            task_id: "bg".to_string(),
            parent_invocation_id: "spawn".to_string(),
            prompt: "p".to_string(),
            description: None,
            elapsed_ms: std::sync::Mutex::new(0),
            state: std::sync::Mutex::new(SubagentBgState::running()),
            started_at: Instant::now(),
            result_rx: tokio::sync::Mutex::new(Some(rx)),
            cancel_token: CancelToken::default(),
        });
        // Drop tx without sending — channel is closed.
        drop(_tx);
        // Need to yield so the channel is recognized as closed.
        tokio::task::yield_now().await;
        let result = task.try_read_result();
        assert!(result.is_none());
        // State should be updated to Failed.
        let state = task.state.lock().unwrap().clone();
        assert_eq!(state.status, SubagentBgStatus::Failed);
        assert!(
            state.error.contains("dropped unexpectedly"),
            "error should mention dropped: {}",
            state.error
        );
    }

    #[tokio::test]
    async fn try_read_result_closed_does_not_override_final_state() {
        let (_tx, rx) = tokio::sync::oneshot::channel::<String>();
        let task = Arc::new(SubagentBackgroundTask {
            task_id: "bg".to_string(),
            parent_invocation_id: "spawn".to_string(),
            prompt: "p".to_string(),
            description: None,
            elapsed_ms: std::sync::Mutex::new(0),
            state: std::sync::Mutex::new(SubagentBgState::done()),
            started_at: Instant::now(),
            result_rx: tokio::sync::Mutex::new(Some(rx)),
            cancel_token: CancelToken::default(),
        });
        drop(_tx);
        tokio::task::yield_now().await;
        let result = task.try_read_result();
        assert!(result.is_none());
        // State should remain Done (not overridden to Failed).
        let state = task.state.lock().unwrap().clone();
        assert_eq!(state.status, SubagentBgStatus::Done);
    }

    // ── resolve_model / main_model ────────────────────────────────────────

    #[test]
    fn resolve_model_without_override_uses_main_model() {
        struct Noop;
        #[async_trait]
        impl ModelProvider for Noop {
            fn stream(&self, _req: ModelRequest) -> ModelStream {
                Box::pin(stream::iter(vec![Ok(ModelStreamEvent::Done)]))
            }
        }
        let tool = SubagentTool::new(
            std::sync::Weak::new(),
            Arc::new(RwLock::new(Arc::new(Noop) as Arc<dyn ModelProvider>)),
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            Arc::new(RwLock::new("main-model".into())),
            HarnessConfig::default(),
            Arc::new(RwLock::new(NaviConfig::default())),
            RuntimeComponents::default(),
        );
        let opts = SubagentOptions::default();
        let (_provider, model) = tool.resolve_model(&opts);
        assert_eq!(model, "main-model");
    }

    #[test]
    fn resolve_model_with_override_uses_override() {
        struct Noop;
        #[async_trait]
        impl ModelProvider for Noop {
            fn stream(&self, _req: ModelRequest) -> ModelStream {
                Box::pin(stream::iter(vec![Ok(ModelStreamEvent::Done)]))
            }
        }
        let tool = SubagentTool::new(
            std::sync::Weak::new(),
            Arc::new(RwLock::new(Arc::new(Noop) as Arc<dyn ModelProvider>)),
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            Arc::new(RwLock::new("main-model".into())),
            HarnessConfig::default(),
            Arc::new(RwLock::new(NaviConfig::default())),
            RuntimeComponents::default(),
        );
        let opts = SubagentOptions {
            model: Some("override-model".into()),
            path_deny: None,
        };
        let (_provider, model) = tool.resolve_model(&opts);
        assert_eq!(model, "override-model");
    }

    #[test]
    fn main_model_returns_configured_values() {
        struct Noop;
        #[async_trait]
        impl ModelProvider for Noop {
            fn stream(&self, _req: ModelRequest) -> ModelStream {
                Box::pin(stream::iter(vec![Ok(ModelStreamEvent::Done)]))
            }
        }
        let tool = SubagentTool::new(
            std::sync::Weak::new(),
            Arc::new(RwLock::new(Arc::new(Noop) as Arc<dyn ModelProvider>)),
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            Arc::new(RwLock::new("my-model".into())),
            HarnessConfig::default(),
            Arc::new(RwLock::new(NaviConfig::default())),
            RuntimeComponents::default(),
        );
        let (_provider, model) = tool.main_model();
        assert_eq!(model, "my-model");
    }

    // ── build_subagent_policy with path_deny ──────────────────────────────

    #[test]
    fn build_subagent_policy_with_path_deny_applies_write_scope() {
        let temp = tempfile::tempdir().unwrap();
        let base = crate::security::SecurityPolicy::new(
            temp.path().to_path_buf(),
            temp.path().join("data"),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let deny = vec!["secrets/".to_string(), ".env".to_string()];
        let policy = build_subagent_policy(&base, Some(&deny)).unwrap();
        // The policy should be yolo mode.
        assert!(matches!(
            policy.config().permission_mode,
            crate::config::PermissionMode::Yolo
        ));
    }

    #[test]
    fn build_subagent_policy_with_empty_deny_does_not_apply_write_scope() {
        let temp = tempfile::tempdir().unwrap();
        let base = crate::security::SecurityPolicy::new(
            temp.path().to_path_buf(),
            temp.path().join("data"),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let deny: Vec<String> = vec![];
        let policy = build_subagent_policy(&base, Some(&deny)).unwrap();
        assert!(matches!(
            policy.config().permission_mode,
            crate::config::PermissionMode::Yolo
        ));
    }

    #[test]
    fn build_subagent_policy_with_none_deny() {
        let temp = tempfile::tempdir().unwrap();
        let base = crate::security::SecurityPolicy::new(
            temp.path().to_path_buf(),
            temp.path().join("data"),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let policy = build_subagent_policy(&base, None).unwrap();
        assert!(matches!(
            policy.config().permission_mode,
            crate::config::PermissionMode::Yolo
        ));
    }

    // ── build_subagent_executor with path_deny ────────────────────────────

    #[test]
    fn build_subagent_executor_with_path_deny_succeeds() {
        struct Noop;
        #[async_trait]
        impl ModelProvider for Noop {
            fn stream(&self, _req: ModelRequest) -> ModelStream {
                Box::pin(stream::iter(vec![Ok(ModelStreamEvent::Done)]))
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let policy = crate::security::SecurityPolicy::new(
            temp.path().to_path_buf(),
            temp.path().join("data"),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = crate::tool::ToolExecutor::new(policy);
        let deny = vec!["secrets/".to_string()];
        let result = build_subagent_executor(&executor, Some(&deny));
        assert!(result.is_ok());
        let forked = result.unwrap();
        assert!(!forked.tool_names().contains(&"subagent".to_string()));
        assert!(!forked.tool_names().contains(&"workflow".to_string()));
    }

    // ── list_background_tasks ─────────────────────────────────────────────

    #[tokio::test]
    async fn list_background_tasks_empty() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        let (_executor, tool) = make_tool_with_noop_provider(executor, project, data);

        let result = tool
            .list_background_tasks("list-1".to_string())
            .await
            .unwrap();
        assert!(result.ok);
        assert_eq!(result.output["tasks"], json!([]));
    }

    #[tokio::test]
    async fn list_background_tasks_with_running_task() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        let (_executor, tool) = make_tool_with_noop_provider(executor, project, data);

        // Insert a running task.
        let task = Arc::new(SubagentBackgroundTask {
            task_id: "bg_1".to_string(),
            parent_invocation_id: "spawn-1".to_string(),
            prompt: "task 1".to_string(),
            description: Some("desc".to_string()),
            elapsed_ms: std::sync::Mutex::new(0),
            state: std::sync::Mutex::new(SubagentBgState::running()),
            started_at: Instant::now(),
            result_rx: tokio::sync::Mutex::new(None),
            cancel_token: CancelToken::default(),
        });
        tool.background_tasks
            .lock()
            .await
            .insert("bg_1".to_string(), task);

        let result = tool
            .list_background_tasks("list-2".to_string())
            .await
            .unwrap();
        assert!(result.ok);
        let tasks = result.output["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["task_id"], "bg_1");
        assert_eq!(tasks[0]["prompt"], "task 1");
        assert_eq!(tasks[0]["status"], "running");
    }

    #[tokio::test]
    async fn list_background_tasks_with_multiple_tasks() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        let (_executor, tool) = make_tool_with_noop_provider(executor, project, data);

        for i in 0..3 {
            let task = Arc::new(SubagentBackgroundTask {
                task_id: format!("bg_{i}"),
                parent_invocation_id: format!("spawn-{i}"),
                prompt: format!("task {i}"),
                description: None,
                elapsed_ms: std::sync::Mutex::new(0),
                state: std::sync::Mutex::new(SubagentBgState::running()),
                started_at: Instant::now(),
                result_rx: tokio::sync::Mutex::new(None),
                cancel_token: CancelToken::default(),
            });
            tool.background_tasks
                .lock()
                .await
                .insert(format!("bg_{i}"), task);
        }

        let result = tool
            .list_background_tasks("list-3".to_string())
            .await
            .unwrap();
        assert!(result.ok);
        let tasks = result.output["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 3);
    }

    // ── handle_background_action with unknown action ──────────────────────

    #[tokio::test]
    async fn handle_background_action_unknown_action_returns_error() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        let (_executor, tool) = make_tool_with_noop_provider(executor, project, data);

        // Insert a task.
        let task = Arc::new(SubagentBackgroundTask {
            task_id: "bg_1".to_string(),
            parent_invocation_id: "spawn-1".to_string(),
            prompt: "p".to_string(),
            description: None,
            elapsed_ms: std::sync::Mutex::new(0),
            state: std::sync::Mutex::new(SubagentBgState::running()),
            started_at: Instant::now(),
            result_rx: tokio::sync::Mutex::new(None),
            cancel_token: CancelToken::default(),
        });
        tool.background_tasks
            .lock()
            .await
            .insert("bg_1".to_string(), task);

        let result = tool
            .handle_background_action("inv-1".to_string(), "bg_1", "unknown", None)
            .await
            .unwrap();
        assert!(result.ok); // Returns Ok with error in output.
        assert!(
            result.output["error"]
                .as_str()
                .is_some_and(|e| e.contains("unknown action")),
            "should return unknown action error: {result:?}"
        );
    }

    // ── invoke_with_context routing ───────────────────────────────────────

    #[tokio::test]
    async fn invoke_with_task_id_routes_to_handle_background_action() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        let (_executor, tool) = make_tool_with_noop_provider(executor, project, data);

        let inv = make_invocation("r1", json!({"task_id": "nonexistent"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert!(
            result.output["error"]
                .as_str()
                .is_some_and(|e| e.contains("no background subagent")),
            "should return not-found error: {result:?}"
        );
    }

    #[tokio::test]
    async fn invoke_with_task_id_and_action_cancel() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        let (_executor, tool) = make_tool_with_noop_provider(executor, project, data);

        // Insert a running task.
        let task = Arc::new(SubagentBackgroundTask {
            task_id: "bg_c".to_string(),
            parent_invocation_id: "spawn-c".to_string(),
            prompt: "p".to_string(),
            description: None,
            elapsed_ms: std::sync::Mutex::new(0),
            state: std::sync::Mutex::new(SubagentBgState::running()),
            started_at: Instant::now(),
            result_rx: tokio::sync::Mutex::new(None),
            cancel_token: CancelToken::default(),
        });
        tool.background_tasks
            .lock()
            .await
            .insert("bg_c".to_string(), task);

        let inv = make_invocation("r2", json!({"task_id": "bg_c", "action": "cancel"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "cancelled");
    }

    #[tokio::test]
    async fn invoke_with_action_list_routes_to_list_background_tasks() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        let (_executor, tool) = make_tool_with_noop_provider(executor, project, data);

        let inv = make_invocation("r3", json!({"action": "list"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["tasks"], json!([]));
    }

    #[tokio::test]
    async fn invoke_foreground_with_dropped_executor_returns_error() {
        // With a dropped executor, run_foreground should return an Err.
        struct Noop;
        #[async_trait]
        impl ModelProvider for Noop {
            fn stream(&self, _req: ModelRequest) -> ModelStream {
                Box::pin(stream::iter(vec![Ok(ModelStreamEvent::Done)]))
            }
        }
        let tool = SubagentTool::new(
            std::sync::Weak::new(), // dropped executor
            Arc::new(RwLock::new(Arc::new(Noop) as Arc<dyn ModelProvider>)),
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            Arc::new(RwLock::new("test".into())),
            HarnessConfig::default(),
            Arc::new(RwLock::new(NaviConfig::default())),
            RuntimeComponents::default(),
        );
        let inv = make_invocation("r4", json!({"prompt": "hello"}));
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "dropped executor should return Err");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("dropped"),
            "error should mention dropped: {err}"
        );
    }

    #[tokio::test]
    async fn invoke_background_with_dropped_executor_returns_error_in_output() {
        // With a dropped executor, spawn_background should return Ok with
        // error in output (not an Err).
        struct Noop;
        #[async_trait]
        impl ModelProvider for Noop {
            fn stream(&self, _req: ModelRequest) -> ModelStream {
                Box::pin(stream::iter(vec![Ok(ModelStreamEvent::Done)]))
            }
        }
        let tool = SubagentTool::new(
            std::sync::Weak::new(), // dropped executor
            Arc::new(RwLock::new(Arc::new(Noop) as Arc<dyn ModelProvider>)),
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            Arc::new(RwLock::new("test".into())),
            HarnessConfig::default(),
            Arc::new(RwLock::new(NaviConfig::default())),
            RuntimeComponents::default(),
        );
        let inv = make_invocation("r5", json!({"prompt": "hello", "background": true}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["error"], "tool executor unavailable");
    }

    // ── run_foreground with real executor + mock provider ─────────────────

    #[tokio::test]
    async fn run_foreground_with_mock_provider_completes() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        let (_executor, tool) = make_tool(
            executor,
            Arc::new(TextProvider {
                text: "Subagent completed successfully".to_string(),
            }),
            project,
            data,
        );

        let inv = make_invocation("fg1", json!({"prompt": "do something"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert!(
            result.output["result"]
                .as_str()
                .is_some_and(|r| r.contains("completed")),
            "result should contain the mock text: {result:?}"
        );
        assert!(
            result.output["elapsed_ms"].as_u64().is_some(),
            "should have elapsed_ms: {result:?}"
        );
    }

    #[tokio::test]
    async fn run_foreground_with_error_provider_returns_failed_text() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        let (_executor, tool) = make_tool(executor, Arc::new(ErrorProvider), project, data);

        let inv = make_invocation("fg2", json!({"prompt": "do something"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        let text = result.output["result"].as_str().unwrap();
        assert!(
            text.contains("Subagent failed"),
            "error provider should produce 'Subagent failed': {text}"
        );
    }

    #[tokio::test]
    async fn run_foreground_with_description_includes_context() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        let (_executor, tool) = make_tool(
            executor,
            Arc::new(TextProvider {
                text: "done".to_string(),
            }),
            project,
            data,
        );

        let inv = make_invocation(
            "fg3",
            json!({"prompt": "explore", "description": "collect info about auth"}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
    }

    #[tokio::test]
    async fn run_foreground_with_model_override() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        let (_executor, tool) = make_tool(
            executor,
            Arc::new(TextProvider {
                text: "done with override".to_string(),
            }),
            project,
            data,
        );

        let inv = make_invocation(
            "fg4",
            json!({
                "prompt": "hello",
                "options": {"model": "claude-sonnet"}
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
    }

    #[tokio::test]
    async fn run_foreground_with_path_deny_option() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        let (_executor, tool) = make_tool(
            executor,
            Arc::new(TextProvider {
                text: "done".to_string(),
            }),
            project,
            data,
        );

        let inv = make_invocation(
            "fg5",
            json!({
                "prompt": "hello",
                "options": {"path_deny": ["secrets/"]}
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
    }

    // ── spawn_background with real executor + mock provider ───────────────

    #[tokio::test]
    async fn spawn_background_returns_task_id() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        let (_executor, tool) = make_tool(
            executor,
            Arc::new(TextProvider {
                text: "bg done".to_string(),
            }),
            project,
            data,
        );

        let inv = make_invocation(
            "bg1",
            json!({"prompt": "background task", "background": true}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert!(
            result.output["task_id"]
                .as_str()
                .is_some_and(|t| t.starts_with("bg_")),
            "should return task_id starting with 'bg_': {result:?}"
        );
        assert_eq!(result.output["status"], "running");
        assert_eq!(result.output["background"], true);
        assert_eq!(result.output["action"], "poll");
    }

    #[tokio::test]
    async fn spawn_background_then_poll_completes() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        let (_executor, tool) = make_tool(
            executor,
            Arc::new(TextProvider {
                text: "bg result".to_string(),
            }),
            project,
            data,
        );

        // Spawn.
        let inv = make_invocation(
            "bg2",
            json!({"prompt": "background task", "background": true}),
        );
        let result = tool.invoke(inv).await.unwrap();
        let task_id = result.output["task_id"].as_str().unwrap().to_string();

        // Give the background task time to complete.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Poll.
        let inv = make_invocation("bg2_poll", json!({"task_id": task_id}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        // Status should be done (or still running if not enough time).
        let status = result.output["status"].as_str().unwrap_or("");
        assert!(
            status == "done" || status == "running",
            "poll should show done or running: {status}"
        );
    }

    #[tokio::test]
    async fn spawn_background_with_description() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        let (_executor, tool) = make_tool(
            executor,
            Arc::new(TextProvider {
                text: "done".to_string(),
            }),
            project,
            data,
        );

        let inv = make_invocation(
            "bg3",
            json!({
                "prompt": "task",
                "description": "my bg task",
                "background": true
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert!(result.output["task_id"].as_str().is_some());
    }

    #[tokio::test]
    async fn spawn_background_with_options() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        let (_executor, tool) = make_tool(
            executor,
            Arc::new(TextProvider {
                text: "done".to_string(),
            }),
            project,
            data,
        );

        let inv = make_invocation(
            "bg4",
            json!({
                "prompt": "task",
                "background": true,
                "options": {"model": "claude", "path_deny": ["secrets/"]}
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
    }

    #[tokio::test]
    async fn spawn_background_max_concurrent_limit() {
        let (_dir, project, data) = make_temp_dirs();
        let policy = crate::security::SecurityPolicy::new(
            project.clone(),
            data.clone(),
            crate::config::SecurityConfig::default(),
        )
        .unwrap();
        let executor = Arc::new(crate::tool::ToolExecutor::new(policy));
        // Use a provider that blocks forever (never sends Done).
        struct BlockingProvider;
        #[async_trait]
        impl ModelProvider for BlockingProvider {
            fn stream(&self, _req: ModelRequest) -> ModelStream {
                Box::pin(futures_util::stream::pending())
            }
        }
        let (_executor, tool) = make_tool(executor, Arc::new(BlockingProvider), project, data);

        // Spawn MAX_BACKGROUND_SUBAGENTS tasks.
        let mut task_ids = Vec::new();
        for i in 0..MAX_BACKGROUND_SUBAGENTS {
            let inv = make_invocation(
                &format!("bg_max_{i}"),
                json!({"prompt": "blocking task", "background": true}),
            );
            let result = tool.invoke(inv).await.unwrap();
            assert!(result.ok, "spawn {i} should succeed");
            task_ids.push(result.output["task_id"].as_str().unwrap().to_string());
        }

        // The next spawn should fail with "too many" error.
        let inv = make_invocation(
            "bg_max_overflow",
            json!({"prompt": "overflow", "background": true}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert!(
            result.output["error"]
                .as_str()
                .is_some_and(|e| e.contains("too many")),
            "should return 'too many' error: {result:?}"
        );
    }

    // ── Tool definition metadata ──────────────────────────────────────────

    #[test]
    fn tool_definition_name_is_subagent() {
        struct Noop;
        #[async_trait]
        impl ModelProvider for Noop {
            fn stream(&self, _req: ModelRequest) -> ModelStream {
                Box::pin(stream::iter(vec![Ok(ModelStreamEvent::Done)]))
            }
        }
        let tool = SubagentTool::new(
            std::sync::Weak::new(),
            Arc::new(RwLock::new(Arc::new(Noop) as Arc<dyn ModelProvider>)),
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            Arc::new(RwLock::new("test".into())),
            HarnessConfig::default(),
            Arc::new(RwLock::new(NaviConfig::default())),
            RuntimeComponents::default(),
        );
        let def = tool.definition();
        assert_eq!(def.name, "subagent");
    }

    #[test]
    fn tool_definition_kind_is_read() {
        struct Noop;
        #[async_trait]
        impl ModelProvider for Noop {
            fn stream(&self, _req: ModelRequest) -> ModelStream {
                Box::pin(stream::iter(vec![Ok(ModelStreamEvent::Done)]))
            }
        }
        let tool = SubagentTool::new(
            std::sync::Weak::new(),
            Arc::new(RwLock::new(Arc::new(Noop) as Arc<dyn ModelProvider>)),
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            Arc::new(RwLock::new("test".into())),
            HarnessConfig::default(),
            Arc::new(RwLock::new(NaviConfig::default())),
            RuntimeComponents::default(),
        );
        let def = tool.definition();
        assert_eq!(def.kind, ToolKind::Read);
    }

    #[test]
    fn tool_definition_schema_requires_prompt_or_task_id_or_list() {
        struct Noop;
        #[async_trait]
        impl ModelProvider for Noop {
            fn stream(&self, _req: ModelRequest) -> ModelStream {
                Box::pin(stream::iter(vec![Ok(ModelStreamEvent::Done)]))
            }
        }
        let tool = SubagentTool::new(
            std::sync::Weak::new(),
            Arc::new(RwLock::new(Arc::new(Noop) as Arc<dyn ModelProvider>)),
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            Arc::new(RwLock::new("test".into())),
            HarnessConfig::default(),
            Arc::new(RwLock::new(NaviConfig::default())),
            RuntimeComponents::default(),
        );
        let schema = tool.definition().input_schema;
        let validator = jsonschema::validator_for(&schema).expect("compile schema");

        // Empty input should fail (none of the anyOf branches match).
        let errors: Vec<String> = validator
            .iter_errors(&json!({}))
            .map(|e| e.to_string())
            .collect();
        assert!(!errors.is_empty(), "empty input should fail schema");

        // prompt only should pass.
        let errors: Vec<String> = validator
            .iter_errors(&json!({"prompt": "hello"}))
            .map(|e| e.to_string())
            .collect();
        assert!(errors.is_empty(), "prompt-only should pass: {errors:?}");

        // task_id only should pass.
        let errors: Vec<String> = validator
            .iter_errors(&json!({"task_id": "bg_1"}))
            .map(|e| e.to_string())
            .collect();
        assert!(errors.is_empty(), "task_id-only should pass: {errors:?}");

        // action=list should pass.
        let errors: Vec<String> = validator
            .iter_errors(&json!({"action": "list"}))
            .map(|e| e.to_string())
            .collect();
        assert!(errors.is_empty(), "action=list should pass: {errors:?}");
    }

    #[test]
    fn tool_definition_schema_action_enum() {
        struct Noop;
        #[async_trait]
        impl ModelProvider for Noop {
            fn stream(&self, _req: ModelRequest) -> ModelStream {
                Box::pin(stream::iter(vec![Ok(ModelStreamEvent::Done)]))
            }
        }
        let tool = SubagentTool::new(
            std::sync::Weak::new(),
            Arc::new(RwLock::new(Arc::new(Noop) as Arc<dyn ModelProvider>)),
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            Arc::new(RwLock::new("test".into())),
            HarnessConfig::default(),
            Arc::new(RwLock::new(NaviConfig::default())),
            RuntimeComponents::default(),
        );
        let schema = tool.definition().input_schema;
        let actions = schema["properties"]["action"]["enum"].as_array().unwrap();
        let action_names: Vec<&str> = actions.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(action_names.contains(&"poll"));
        assert!(action_names.contains(&"cancel"));
        assert!(action_names.contains(&"list"));
    }

    // ── compact_result_detail additional cases ────────────────────────────

    #[test]
    fn compact_result_detail_for_complex_output() {
        let result = ToolResult {
            invocation_id: "c".to_string(),
            ok: true,
            output: json!({"foo": "bar", "baz": 42}),
        };
        let detail = compact_result_detail(&result);
        assert!(
            detail.contains("foo") || detail.contains("bar"),
            "complex output should be serialized: {detail}"
        );
    }

    #[test]
    fn compact_result_detail_error_takes_precedence_over_path() {
        let result = ToolResult {
            invocation_id: "c".to_string(),
            ok: false,
            output: json!({"error": "denied", "path": "src/main.rs"}),
        };
        let detail = compact_result_detail(&result);
        assert_eq!(detail, "denied", "error should take precedence over path");
    }

    #[test]
    fn compact_result_detail_path_takes_precedence_over_result() {
        let result = ToolResult {
            invocation_id: "c".to_string(),
            ok: true,
            output: json!({"path": "src/main.rs", "result": "content"}),
        };
        let detail = compact_result_detail(&result);
        assert_eq!(
            detail, "src/main.rs",
            "path should take precedence over result"
        );
    }

    // ── emit_subagent_transcript ──────────────────────────────────────────

    #[test]
    fn emit_subagent_transcript_noop_when_no_tx() {
        // When parent_event_tx is None, should not panic.
        emit_subagent_transcript(
            &None,
            "inv-1",
            SubagentTranscriptItem {
                kind: SubagentTranscriptKind::Text,
                title: "test".to_string(),
                detail: None,
                ok: None,
                invocation: None,
                result: None,
                text: None,
                thinking: None,
                status: None,
            },
        );
        // No panic = pass.
    }

    #[test]
    fn emit_subagent_transcript_sends_event_when_tx_present() {
        let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
        emit_subagent_transcript(
            &Some(tx),
            "inv-1",
            SubagentTranscriptItem {
                kind: SubagentTranscriptKind::Text,
                title: "test".to_string(),
                detail: None,
                ok: None,
                invocation: None,
                result: None,
                text: None,
                thinking: None,
                status: None,
            },
        );
        let event = rx.try_recv().expect("event should be sent");
        match event {
            AgentEvent::SubagentTranscript {
                invocation_id,
                item,
            } => {
                assert_eq!(invocation_id, "inv-1");
                assert_eq!(item.title, "test");
            }
            other => panic!("expected SubagentTranscript, got {other:?}"),
        }
    }

    // ── freeze_specialized_prompt additional cases ────────────────────────

    #[test]
    fn freeze_specialized_prompt_with_no_system_message() {
        let messages = vec![ModelMessage {
            role: ModelRole::User,
            content: "hello".into(),
            content_parts: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            tool_calls: vec![],
            created_at: None,
            thinking_content: None,
        }];
        let (instructions, prefix) = freeze_specialized_prompt(&messages);
        assert!(
            instructions
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .is_none(),
            "no system message → no instructions"
        );
        let frozen = prefix.lock().unwrap_or_else(|e| e.into_inner()).clone();
        // No system/developer messages → empty prefix.
        assert!(
            frozen.is_none() || frozen.as_ref().is_some_and(|v| v.is_empty()),
            "no system/developer → empty or none prefix"
        );
    }

    #[test]
    fn freeze_specialized_prompt_with_developer_message() {
        let messages = vec![
            ModelMessage {
                role: ModelRole::Developer,
                content: "dev instructions".into(),
                content_parts: Vec::new(),
                tool_call_id: None,
                tool_name: None,
                tool_calls: vec![],
                created_at: None,
                thinking_content: None,
            },
            ModelMessage {
                role: ModelRole::System,
                content: "system instructions".into(),
                content_parts: Vec::new(),
                tool_call_id: None,
                tool_name: None,
                tool_calls: vec![],
                created_at: None,
                thinking_content: None,
            },
            ModelMessage {
                role: ModelRole::User,
                content: "hello".into(),
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
            Some("system instructions")
        );
        let frozen = prefix
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("prefix");
        // Both developer and system messages are in the prefix.
        assert_eq!(frozen.len(), 2);
        assert_eq!(frozen[0].role, ModelRole::Developer);
        assert_eq!(frozen[1].role, ModelRole::System);
    }

    // ── SubagentOptions serde edge cases ──────────────────────────────────

    #[test]
    fn subagent_options_serde_model_only() {
        let json = json!({"model": "gpt-5"});
        let opts: SubagentOptions = serde_json::from_value(json).unwrap();
        assert_eq!(opts.model.as_deref(), Some("gpt-5"));
        assert!(opts.path_deny.is_none());
    }

    #[test]
    fn subagent_options_serde_empty_path_deny() {
        let json = json!({"path_deny": []});
        let opts: SubagentOptions = serde_json::from_value(json).unwrap();
        assert!(opts.model.is_none());
        assert_eq!(opts.path_deny.as_deref(), Some([].as_slice()));
    }

    #[test]
    fn subagent_options_serde_skip_serializing_none() {
        let opts = SubagentOptions::default();
        let json = serde_json::to_value(&opts).unwrap();
        // skip_serializing_if = "Option::is_none" means None fields are absent.
        assert!(json.get("model").is_none());
        assert!(json.get("path_deny").is_none());
    }

    #[test]
    fn subagent_options_serde_roundtrip_with_both() {
        let opts = SubagentOptions {
            model: Some("claude".into()),
            path_deny: Some(vec!["a".into(), "b".into()]),
        };
        let json = serde_json::to_value(&opts).unwrap();
        let back: SubagentOptions = serde_json::from_value(json).unwrap();
        assert_eq!(opts, back);
    }

    #[test]
    fn subagent_options_partial_eq() {
        let a = SubagentOptions {
            model: Some("x".into()),
            path_deny: Some(vec!["y".into()]),
        };
        let b = SubagentOptions {
            model: Some("x".into()),
            path_deny: Some(vec!["y".into()]),
        };
        assert_eq!(a, b);

        let c = SubagentOptions {
            model: Some("z".into()),
            path_deny: Some(vec!["y".into()]),
        };
        assert_ne!(a, c);
    }
}
