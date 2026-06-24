use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, Weak};
use std::time::Instant;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;

use super::helpers;
use crate::background_model::BackgroundModelResolver;
use crate::cancel::CancelToken;
use crate::compact::CompactState;
use crate::config::{HarnessConfig, LoadedConfig, NaviConfig};
use crate::event::{AgentEvent, ApprovalDecision};
use crate::model::{ModelMessage, ModelProvider, ModelRole};
use crate::prompt::PromptCache;
use crate::runtime::ApprovalResolver;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};
use crate::turn::TurnContext;

const MAX_BACKGROUND_SUBAGENTS: usize = 8;

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
    prompt_cache: Arc<PromptCache>,
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
        model_name: Arc<RwLock<String>>,
        harness_config: HarnessConfig,
        config: Arc<RwLock<NaviConfig>>,
        prompt_cache: Arc<PromptCache>,
    ) -> Self {
        Self {
            tool_executor,
            model_provider,
            project_dir,
            model_name,
            harness_config,
            config,
            prompt_cache,
            background_tasks: tokio::sync::Mutex::new(HashMap::new()),
            next_task_id: AtomicU64::new(1),
            background_resolver: None,
            data_dir: std::path::PathBuf::new(),
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

        if is_background {
            return self
                .spawn_background(invocation.id, prompt, description, profile)
                .await;
        }

        self.run_foreground(invocation.id, prompt, description, profile)
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
    ) -> Result<ToolResult> {
        let executor = self
            .tool_executor
            .upgrade()
            .context("subagent tool executor has been dropped")?;
        let started = Instant::now();

        // Resolve model provider based on profile.
        let (provider, model) = self.resolve_model_for_profile(profile.as_deref());

        let (mut messages, event_tx, _approval_handle, resolver) =
            self.prepare_subagent_context(&prompt, &description);

        let include_tool_prompt = self.include_tool_prompt_manifest();

        let sub_ctx = TurnContext {
            model_provider: Arc::new(RwLock::new(provider)),
            tool_executor: executor,
            project_dir: self.project_dir.clone(),
            model_name: Arc::new(RwLock::new(model)),
            event_tx: Some(event_tx),
            approval_resolver: resolver,
            question_resolver: crate::runtime::QuestionResolver::new_standalone(),
            compact_state: Arc::new(tokio::sync::Mutex::new(CompactState::new(
                crate::config::effective_context_window(
                    &self.config.read().unwrap_or_else(|e| e.into_inner()),
                ),
            ))),
            harness_config: self.harness_config.clone(),
            include_tool_prompt_manifest: include_tool_prompt,
            context_packets: Arc::new(std::sync::Mutex::new(Vec::new())),
            active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
            prompt_cache: self.prompt_cache.clone(),
            cancel_token: CancelToken::new(),
            config: self.config.clone(),
            compaction_provider: None,
            compaction_model_name: None,
            session_id: "subagent".to_string(),
        };

        let policy =
            crate::harness::policy_for_profile(&self.harness_config, self.harness_config.profile);

        let result = crate::turn::run_turn(&sub_ctx, &mut messages, policy).await;
        let elapsed = started.elapsed();

        let text = match result {
            Ok(output) => output,
            Err(err) => format!("Subagent failed: {err:#}"),
        };

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

        let task = Arc::new(SubagentBackgroundTask {
            task_id: task_id.clone(),
            prompt: prompt.clone(),
            description: description.clone(),
            elapsed_ms: std::sync::Mutex::new(0),
            state: std::sync::Mutex::new(SubagentBgState::running()),
            started_at: started,
            result_rx: tokio::sync::Mutex::new(Some(result_rx)),
            cancel_token: CancelToken::new(),
        });
        tasks.insert(task_id.clone(), task.clone());

        // Resolve model provider based on profile.
        let (resolved_provider, resolved_model) =
            self.resolve_model_for_profile(profile.as_deref());
        let model_provider = Arc::new(RwLock::new(resolved_provider));
        let model_name = Arc::new(RwLock::new(resolved_model));
        let prompt_cache = self.prompt_cache.clone();
        let harness_config = self.harness_config.clone();
        let config = self.config.clone();
        let project_dir = self.project_dir.clone();
        let cancel_token = task.cancel_token.clone();

        tokio::spawn(async move {
            let (mut messages, event_tx, _approval_handle, resolver) =
                Self::build_subagent_context_static(&prompt, &description);

            let config_snapshot = config.read().unwrap_or_else(|e| e.into_inner()).clone();

            let sub_ctx = TurnContext {
                model_provider,
                tool_executor: executor,
                project_dir,
                model_name,
                event_tx: Some(event_tx),
                approval_resolver: resolver,
                question_resolver: crate::runtime::QuestionResolver::new_standalone(),
                compact_state: Arc::new(tokio::sync::Mutex::new(CompactState::new(
                    crate::config::effective_context_window(&config_snapshot),
                ))),
                harness_config: harness_config.clone(),
                include_tool_prompt_manifest: crate::config::effective_tool_prompt_manifest(
                    &config_snapshot,
                ),
                context_packets: Arc::new(std::sync::Mutex::new(Vec::new())),
                active_skills: Arc::new(std::sync::Mutex::new(Vec::new())),
                prompt_cache,
                cancel_token,
                config: Arc::new(std::sync::RwLock::new(config_snapshot)),
                compaction_provider: None,
                compaction_model_name: None,
                session_id: "subagent-bg".to_string(),
            };

            let policy =
                crate::harness::policy_for_profile(&harness_config, harness_config.profile);

            let result = crate::turn::run_turn(&sub_ctx, &mut messages, policy).await;
            let output = match result {
                Ok(output) => output,
                Err(err) => format!("Background subagent failed: {err:#}"),
            };
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
        prompt: &str,
        description: &Option<String>,
    ) -> (
        Vec<ModelMessage>,
        tokio::sync::mpsc::UnboundedSender<AgentEvent>,
        tokio::task::JoinHandle<()>,
        ApprovalResolver,
    ) {
        let (messages, tx, handle, resolver) =
            Self::build_subagent_context_static(prompt, description);
        (messages, tx, handle, resolver)
    }

    fn build_subagent_context_static(
        prompt: &str,
        description: &Option<String>,
    ) -> (
        Vec<ModelMessage>,
        tokio::sync::mpsc::UnboundedSender<AgentEvent>,
        tokio::task::JoinHandle<()>,
        ApprovalResolver,
    ) {
        let system = if let Some(desc) = description {
            format!(
                "You are a subagent worker. Execute the assigned task autonomously \
                 using whatever tools are needed. You have access to all the same \
                 tools as the main agent.\n\nContext: {desc}\n\n\
                 When the task is complete, report your findings and any relevant output. \
                 Be concise and focus on delivering the result."
            )
        } else {
            "You are a subagent worker. Execute the assigned task autonomously \
             using whatever tools are needed. You have access to all the same \
             tools as the main agent.\n\n\
             When the task is complete, report your findings and any relevant output. \
             Be concise and focus on delivering the result."
                .to_string()
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

        let approval_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                if let AgentEvent::ApprovalRequested(req) = event {
                    resolver_bg.resolve(ApprovalDecision::Approved { id: req.id.clone() });
                }
            }
        });

        (messages, event_tx, approval_handle, resolver)
    }
}
