mod event_bus;
mod session_state;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::cancel::CancelToken;
use crate::config::LoadedConfig;
use crate::context::ContextPacket;
use crate::event::{
    AgentEvent, ApprovalDecision, QuestionResponse, RuntimeEvent, RuntimeEventKind,
};
use crate::goal::{
    CreateGoalTool, GetGoalTool, GoalExtension, GoalRuntimeHandle, GoalService,
    UpdateGoalChecklistTool, UpdateGoalTool,
};
use crate::harness::select_harness_policy;
use crate::model::{ModelMessage, ModelProvider, ModelResponse};
use crate::runtime_components::RuntimeComponents;
use crate::security::SecurityPolicy;
use crate::session::{SessionId, SessionStore, current_unix_timestamp};
use crate::skills::{SkillManifest, active_skills, discover_configured_skills};
use crate::tool::builtin::{RepoExploreTool, SubagentTool};
use crate::tool::{Tool, ToolExecutor};
use crate::trace::{TraceStore, turn_traces_from_events};
use crate::{
    ModelOption, SessionSnapshot, available_model_options, canonical_provider_id,
    provider_request_model_name,
};
use anyhow::Result;

pub use event_bus::EventBus;
pub use session_state::SessionState;

type PendingApprovals = Arc<std::sync::Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>>;
type PendingQuestions = Arc<std::sync::Mutex<HashMap<String, oneshot::Sender<QuestionResponse>>>>;

/// Resolves pending tool approvals by matching decision ids to waiting
/// receivers. Cloneable so it can be handed to the UI layer.
#[derive(Clone)]
pub struct ApprovalResolver {
    pending_approvals: PendingApprovals,
    runtime_events_tx: broadcast::Sender<RuntimeEvent>,
}

/// Resolves pending interactive questions by matching response ids to waiting
/// receivers. Cloneable so it can be handed to the UI layer.
#[derive(Clone)]
pub struct QuestionResolver {
    pending_questions: PendingQuestions,
    runtime_events_tx: broadcast::Sender<RuntimeEvent>,
}

impl QuestionResolver {
    #[cfg(test)]
    pub fn new_for_test() -> Self {
        let (tx, _) = broadcast::channel(16);
        Self {
            pending_questions: Arc::new(std::sync::Mutex::new(HashMap::new())),
            runtime_events_tx: tx,
        }
    }

    pub(crate) fn new_standalone() -> Self {
        let (tx, _) = broadcast::channel(16);
        Self {
            pending_questions: Arc::new(std::sync::Mutex::new(HashMap::new())),
            runtime_events_tx: tx,
        }
    }

    /// Register a pending question, returning the receiver for the response.
    pub fn register(&self, id: String) -> oneshot::Receiver<QuestionResponse> {
        let (tx, rx) = oneshot::channel();
        self.pending_questions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);
        rx
    }

    /// Resolves a pending question by id. Returns `true` if a matching request
    /// was found and resolved.
    pub fn resolve(&self, response: QuestionResponse) -> bool {
        let id = response.id().to_string();
        if let Some(tx) = self
            .pending_questions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id)
        {
            let _ = tx.send(response.clone());
            let _ =
                self.runtime_events_tx
                    .send(RuntimeEvent::new(RuntimeEventKind::QuestionResolved(
                        response,
                    )));
            true
        } else {
            false
        }
    }
}

impl ApprovalResolver {
    #[cfg(test)]
    pub fn new_for_test() -> Self {
        let (tx, _) = broadcast::channel(16);
        Self {
            pending_approvals: Arc::new(std::sync::Mutex::new(HashMap::new())),
            runtime_events_tx: tx,
        }
    }

    pub(crate) fn new_standalone() -> Self {
        let (tx, _) = broadcast::channel(16);
        Self {
            pending_approvals: Arc::new(std::sync::Mutex::new(HashMap::new())),
            runtime_events_tx: tx,
        }
    }

    /// Register a pending approval, returning the receiver for the decision.
    pub fn register(&self, id: String) -> oneshot::Receiver<ApprovalDecision> {
        let (tx, rx) = oneshot::channel();
        self.pending_approvals
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);
        rx
    }

    /// Resolves a pending approval by id. Returns `true` if a matching
    /// pending request was found and resolved.
    pub fn resolve(&self, decision: ApprovalDecision) -> bool {
        let id = match &decision {
            ApprovalDecision::Approved { id } => id,
            ApprovalDecision::Denied { id } => id,
        };
        if let Some(tx) = self
            .pending_approvals
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
        {
            let _ = tx.send(decision.clone());
            let _ =
                self.runtime_events_tx
                    .send(RuntimeEvent::new(RuntimeEventKind::ApprovalResolved(
                        decision,
                    )));
            true
        } else {
            false
        }
    }
}

/// A lightweight handle that cancels the current turn when dropped or called.
/// Cloneable so it can be handed to the UI layer.
#[derive(Clone)]
pub struct TurnCanceller {
    inner: CancelToken,
}

impl TurnCanceller {
    /// Cancels the current turn.
    pub fn cancel(&self) {
        self.inner.cancel();
    }
}

/// Options for constructing an [`AgentRuntime`].
pub struct AgentRuntimeOptions {
    /// Loaded and merged configuration.
    pub loaded_config: LoadedConfig,
    /// The model provider implementation.
    pub model_provider: Arc<dyn ModelProvider>,
    /// Project root directory.
    pub project_dir: PathBuf,
    /// Optional custom tool executor (defaults to built-in tools).
    pub tool_executor: Option<Arc<ToolExecutor>>,
    /// Context packets to inject into the session.
    pub context_packets: Vec<ContextPacket>,
    /// Active skill names for this session.
    pub active_skills: Vec<String>,
    /// Seed messages for restoring a session.
    pub initial_messages: Vec<ModelMessage>,
    /// Seed events for restoring a persisted session without losing history.
    pub initial_events: Vec<AgentEvent>,
    /// Original creation timestamp for restored sessions.
    pub initial_created_at: Option<u64>,
    /// Original update timestamp for restored sessions.
    pub initial_updated_at: Option<u64>,
    /// Goal restored from a persisted session snapshot.
    pub initial_goal: Option<crate::goal::types::SessionGoal>,
    /// Session id for restoring an existing session.
    pub session_id: Option<SessionId>,
    /// Optional channel for forwarding agent events outside the runtime.
    pub event_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    /// Replaceable runtime components. Defaults preserve NAVI's code-agent behavior.
    pub runtime_components: Option<RuntimeComponents>,
}

/// The core agent runtime that manages sessions, turns, approvals, and events.
pub struct AgentRuntime {
    loaded_config: LoadedConfig,
    model_provider: Arc<dyn ModelProvider>,
    shared_model_provider: Arc<RwLock<Arc<dyn ModelProvider>>>,
    shared_model_name: Arc<RwLock<String>>,
    shared_config: Arc<RwLock<crate::config::NaviConfig>>,
    project_dir: PathBuf,
    tool_executor: Option<Arc<ToolExecutor>>,
    session_store: SessionStore,
    context_packets: Vec<ContextPacket>,
    shared_context_packets: Arc<std::sync::Mutex<Vec<ContextPacket>>>,
    active_skills: Vec<String>,
    shared_available_skills: Arc<std::sync::Mutex<Vec<crate::skills::SkillManifest>>>,
    shared_active_skills: Arc<std::sync::Mutex<Vec<crate::skills::SkillManifest>>>,
    prompt_cache: Arc<crate::prompt::PromptCache>,
    runtime_components: RuntimeComponents,
    initial_messages: Vec<ModelMessage>,
    event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
    cancel_token: CancelToken,
    pending_approvals: PendingApprovals,
    pending_questions: PendingQuestions,
    event_bus: EventBus,
    session: SessionState,
    /// Goal runtime handle for the current session.
    goal_runtime: Arc<GoalRuntimeHandle>,
    /// Goal extension providing lifecycle hooks.
    goal_extension: GoalExtension,
    /// Whether the model used the `memory` tool with `write` action during the current turn.
    /// Used for mutual exclusion with background extractMemories.
    turn_used_memory_write: bool,
    /// Last user task text — used for extractMemories context.
    last_user_task: String,
}

impl AgentRuntime {
    /// Creates a new runtime from the given options.
    pub fn new(options: AgentRuntimeOptions) -> Self {
        let session_store = SessionStore::with_redaction(
            options.loaded_config.data_dir.clone(),
            options
                .loaded_config
                .config
                .security
                .redact_secrets_in_sessions,
        );

        let shared_context_packets =
            Arc::new(std::sync::Mutex::new(options.context_packets.clone()));
        let shared_available_skills = Arc::new(std::sync::Mutex::new(Vec::new()));
        let shared_active_skills = Arc::new(std::sync::Mutex::new(Vec::new()));
        let shared_model_provider = Arc::new(RwLock::new(options.model_provider.clone()));
        let shared_model_name = Arc::new(RwLock::new(provider_request_model_name(
            &options.loaded_config.config.model.provider,
            &options.loaded_config.config.model.name,
        )));
        let shared_config = Arc::new(RwLock::new(options.loaded_config.config.clone()));
        let prompt_cache = Arc::new(crate::prompt::PromptCache::new());
        let runtime_components = options.runtime_components.unwrap_or_default();
        let goal_service = Arc::new(GoalService::new());
        let goal_runtime = Arc::new(GoalRuntimeHandle::new(options.initial_goal.clone()));
        let goal_extension = GoalExtension::new(goal_service.clone(), goal_runtime.clone());

        Self {
            loaded_config: options.loaded_config,
            model_provider: options.model_provider,
            shared_model_provider,
            shared_model_name,
            shared_config,
            project_dir: options.project_dir,
            tool_executor: options.tool_executor,
            session_store,
            context_packets: options.context_packets,
            shared_context_packets,
            active_skills: options.active_skills,
            shared_available_skills,
            shared_active_skills,
            prompt_cache,
            runtime_components,
            initial_messages: options.initial_messages,
            event_tx: options.event_tx,
            cancel_token: CancelToken::new(),
            pending_approvals: Arc::new(std::sync::Mutex::new(HashMap::new())),
            pending_questions: Arc::new(std::sync::Mutex::new(HashMap::new())),
            event_bus: EventBus::new(),
            session: SessionState::new_with_history(
                options.session_id,
                options.initial_events,
                options.initial_created_at,
                options.initial_updated_at,
            ),
            goal_runtime,
            goal_extension,
            turn_used_memory_write: false,
            last_user_task: String::new(),
        }
    }

    /// Returns all agent events recorded so far.
    /// Returns the current session goal, if any.
    pub fn get_goal(&self) -> Option<crate::goal::types::SessionGoal> {
        self.goal_runtime.get_goal()
    }

    /// Sets or updates the session goal.
    pub fn set_goal(
        &self,
        objective: String,
        token_budget: Option<i64>,
    ) -> crate::goal::types::SessionGoal {
        self.goal_runtime.set_objective(objective, token_budget)
    }

    /// Clears the current session goal.
    pub fn clear_goal(&self) {
        self.goal_runtime.clear_goal();
    }

    /// Updates the stored goal (used after status transitions).
    pub fn update_goal(&self, goal: crate::goal::types::SessionGoal) {
        self.goal_runtime.update_goal(goal);
    }

    /// Updates the goal checklist (replaces all tasks).
    pub fn update_goal_checklist(
        &self,
        tasks: Vec<crate::goal::types::GoalTask>,
    ) -> Option<crate::goal::types::SessionGoal> {
        self.goal_runtime.update_checklist(tasks)
    }

    /// Updates a single task's status in the goal checklist.
    pub fn update_goal_task_status(
        &self,
        task_id: usize,
        status: crate::goal::types::TaskStatus,
    ) -> Option<crate::goal::types::SessionGoal> {
        self.goal_runtime.update_task_status(task_id, status)
    }

    /// Returns a continuation steering prompt if the goal is active and should auto-continue.
    pub fn goal_idle_prompt(&self) -> Option<String> {
        self.goal_extension.on_idle()
    }

    /// Returns all agent events recorded so far.
    pub fn events(&self) -> &[AgentEvent] {
        self.session.events()
    }

    /// Returns the current session id.
    pub fn session_id(&self) -> &SessionId {
        self.session.id()
    }

    /// Returns the session title, if one has been derived.
    pub fn session_title(&self) -> Option<&str> {
        self.session.title()
    }

    /// Adds a context packet to the session and emits a `ContextUpdated` event.
    pub fn add_context_packet(&mut self, packet: ContextPacket) {
        self.context_packets.push(packet.clone());
        self.shared_context_packets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(packet);
        self.event_bus.publish(RuntimeEventKind::ContextUpdated);
    }

    /// Clears all context packets and emits a `ContextUpdated` event.
    pub fn clear_context_packets(&mut self) {
        self.context_packets.clear();
        self.shared_context_packets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.event_bus.publish(RuntimeEventKind::ContextUpdated);
    }

    /// Returns the current context packets.
    pub fn context_packets(&self) -> &[ContextPacket] {
        &self.context_packets
    }

    /// Sets the active skills for this session and emits a `ContextUpdated` event.
    pub fn set_active_skills(&mut self, skills: Vec<String>) {
        self.active_skills = skills;
        let manifests = self.load_active_skills();
        *self
            .shared_active_skills
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = manifests;
        self.event_bus.publish(RuntimeEventKind::ContextUpdated);
    }

    /// Lists available model options from the loaded configuration.
    pub fn list_models(&self) -> Vec<ModelOption> {
        available_model_options(&self.loaded_config.config)
    }

    /// Changes the selected model and emits a `ContextUpdated` event.
    pub fn set_model(&mut self, provider: impl Into<String>, model: impl Into<String>) {
        self.loaded_config.config.model.provider =
            canonical_provider_id(&provider.into()).to_string();
        self.loaded_config.config.model.name = model.into();
        self.update_shared_model_state();
        self.event_bus.publish(RuntimeEventKind::ContextUpdated);
    }

    /// Replaces the runtime configuration and provider used by subsequent turns.
    pub fn set_model_provider(
        &mut self,
        loaded_config: LoadedConfig,
        model_provider: Arc<dyn ModelProvider>,
    ) {
        self.loaded_config = loaded_config;
        self.model_provider = model_provider;
        self.update_shared_model_state();
        self.event_bus.publish(RuntimeEventKind::ContextUpdated);
    }

    /// Registers a host-provided tool with the runtime's tool executor.
    /// Creates a default executor if none exists yet.
    pub fn register_host_tool(&mut self, tool: Arc<dyn Tool>) -> Result<()> {
        if self.tool_executor.is_none() {
            let security_policy = SecurityPolicy::new(
                self.project_dir.clone(),
                self.loaded_config.data_dir.clone(),
                self.loaded_config.config.effective_security_config(),
            )?;
            self.tool_executor = Some(Arc::new(ToolExecutor::with_security_policy(
                security_policy,
                self.runtime_components.security.clone(),
            )));
        }

        let Some(executor) = self.tool_executor.as_mut() else {
            return Err(anyhow::anyhow!("tool executor unavailable"));
        };
        let Some(executor) = Arc::get_mut(executor) else {
            return Err(anyhow::anyhow!(
                "cannot register host tool while tool executor is shared"
            ));
        };
        executor.register_tool(tool);
        self.event_bus.publish(RuntimeEventKind::ContextUpdated);
        Ok(())
    }

    /// Returns a broadcast receiver for [`RuntimeEvent`]s.
    pub fn stream_events(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.event_bus.stream_events()
    }

    /// Cancels the currently running turn.
    pub fn cancel_turn(&self) {
        self.turn_canceller().cancel();
    }

    /// Resolves a pending approval by id. Returns `true` if found.
    pub fn resolve_approval(&self, decision: ApprovalDecision) -> bool {
        self.approval_resolver().resolve(decision)
    }

    /// Resolves a pending interactive question by id. Returns `true` if found.
    pub fn resolve_question(&self, response: QuestionResponse) -> bool {
        self.question_resolver().resolve(response)
    }

    /// Returns an [`ApprovalResolver`] handle for external approval resolution.
    pub fn approval_resolver(&self) -> ApprovalResolver {
        ApprovalResolver {
            pending_approvals: self.pending_approvals.clone(),
            runtime_events_tx: self.event_bus.sender(),
        }
    }

    /// Returns a [`QuestionResolver`] handle for external question resolution.
    pub fn question_resolver(&self) -> QuestionResolver {
        QuestionResolver {
            pending_questions: self.pending_questions.clone(),
            runtime_events_tx: self.event_bus.sender(),
        }
    }

    /// Returns a [`TurnCanceller`] handle for external cancellation.
    pub fn turn_canceller(&self) -> TurnCanceller {
        TurnCanceller {
            inner: self.cancel_token.clone(),
        }
    }

    /// Starts a new session (or restarts if one is already active).
    /// Returns the session id.
    pub fn start_session(&mut self) -> Result<SessionId> {
        if self.session.started() {
            self.goal_extension
                .on_session_end(self.session.id().as_str());
            self.runtime_components
                .hooks
                .on_session_end(self.session.id().as_str());

            // Light auto-memory consolidation on session end (stale + dedup, no model needed)
            let _ = self.consolidate_auto_memory();

            self.event_bus.publish(RuntimeEventKind::SessionFinished {
                session_id: self.session.id().as_str().to_string(),
            });
        }
        self.cancel_token.reset();
        self.pending_approvals = Arc::new(std::sync::Mutex::new(HashMap::new()));
        self.pending_questions = Arc::new(std::sync::Mutex::new(HashMap::new()));
        self.session.start();

        let (session_runtime, event_rx) = self.build_session_runtime()?;
        self.session.set_runtime(session_runtime, event_rx);

        let id = self.session.id().clone();
        // Goal lifecycle: session start + register runtime
        self.goal_extension.on_session_start(id.as_str());
        self.runtime_components.hooks.on_session_start(id.as_str());
        self.event_bus.publish(RuntimeEventKind::SessionStarted {
            session_id: id.as_str().to_string(),
        });

        Ok(id)
    }

    /// Sends a user task to the agent and waits for the full response.
    /// Starts a session automatically if one is not active.
    /// Sends a user turn with optional multimodal content parts.
    ///
    /// When `content_parts` is non-empty, the message is created as a
    /// multimodal user message containing both text and images.
    pub async fn send_turn_with_parts(
        &mut self,
        task: String,
        content_parts: Vec<crate::model::ContentPart>,
        thinking_override: Option<crate::model::ThinkingConfig>,
    ) -> Result<ModelResponse> {
        if !self.session.started() || self.session.runtime().is_none() {
            self.start_session()?;
        }

        // Apply per-turn thinking override before the turn runs so
        // build_model_request picks it up from the shared config.
        if let Some(thinking) = thinking_override {
            let level_str = match thinking {
                crate::model::ThinkingConfig::Adaptive => "adaptive",
                crate::model::ThinkingConfig::Max => "max",
                crate::model::ThinkingConfig::High => "high",
                crate::model::ThinkingConfig::Medium => "medium",
                crate::model::ThinkingConfig::Low => "low",
                crate::model::ThinkingConfig::Off => "off",
            };
            // NOTE: we only update shared_config, NOT loaded_config. Mutating
            // loaded_config would permanently corrupt the original config and
            // leak the last override into future turns that pass thinking: None.
            self.shared_config
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .tui
                .thinking_level = level_str.to_string();
        }

        let submission_tx = self
            .session
            .runtime()
            .ok_or_else(|| anyhow::anyhow!("session not started"))?
            .submission_tx
            .clone();

        let mut event_rx = self
            .session
            .take_event_rx()
            .ok_or_else(|| anyhow::anyhow!("session event stream unavailable"))?;

        self.cancel_token.reset();

        let turn_id = self.session.next_turn_id();
        tracing::info!(
            project = %self.project_dir.display(),
            provider = %self.loaded_config.config.model.provider,
            model = %self.loaded_config.config.model.name,
            "agent task submitted"
        );
        let session_id = self.session.id().as_str().to_string();
        self.runtime_components
            .hooks
            .on_turn_start(&session_id, &task);
        self.goal_extension.on_turn_start(&session_id, &task);
        self.record_event(AgentEvent::UserTaskSubmitted {
            text: task.clone(),
            content_parts: content_parts.clone(),
        });
        self.last_user_task = task.clone();
        self.event_bus.publish(RuntimeEventKind::TurnStarted {
            turn_id: turn_id.clone(),
        });

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        if let Err(e) = submission_tx.send(crate::session::Submission {
            task,
            content_parts,
            response_tx,
        }) {
            return Err(anyhow::anyhow!("failed to send submission: {}", e));
        }

        let mut response_rx = response_rx;
        let result: Result<String> = loop {
            tokio::select! {
                res = &mut response_rx => {
                    break match res {
                        Ok(Ok(text)) => Ok(text),
                        Ok(Err(err)) => Err(anyhow::anyhow!(err)),
                        Err(_) => Err(anyhow::anyhow!("turn cancelled or panicked")),
                    };
                }
                Some(event) = event_rx.recv() => {
                    self.record_event(event);
                }
            }
        };

        while let Ok(event) = event_rx.try_recv() {
            self.record_event(event);
        }
        drop(event_rx);
        self.session.set_updated_at(current_unix_timestamp());
        self.session.update_title_from_events();

        match &result {
            Ok(text) => {
                self.goal_extension.on_turn_end(&session_id);
                self.runtime_components
                    .hooks
                    .on_turn_end(self.session.id().as_str(), text);
                self.event_bus.publish(RuntimeEventKind::TurnCompleted {
                    turn_id,
                    text: text.clone(),
                });

                // extractMemories: background extraction per turn (fire-and-forget)
                // Skip if the model already wrote memories during this turn
                let model_wrote_memory = self.turn_used_memory_write;

                if !model_wrote_memory {
                    // Build conversation snippet from user task + assistant response
                    let user_task = self.last_user_task.clone();
                    let conversation = if user_task.is_empty() {
                        format!("Assistant: {}", text)
                    } else {
                        format!("User: {}\n\nAssistant: {}", user_task, text)
                    };
                    self.try_extract_memories(&session_id, &conversation);
                }

                // Reset per-turn flag
                self.turn_used_memory_write = false;

                // Auto-dream: fire-and-forget check after each turn
                self.try_auto_dream();

                // Auto-distill: fire-and-forget check after each turn
                self.try_auto_distill();
            }
            Err(err) => {
                self.goal_extension.on_turn_error(&err.to_string());
                self.record_event(AgentEvent::Error {
                    message: err.to_string(),
                });
            }
        }

        result.map(|text| {
            tracing::info!(chars = text.len(), "agent task completed");
            ModelResponse { text }
        })
    }

    pub async fn submit_task(&mut self, task: String) -> Result<ModelResponse> {
        self.send_turn_with_parts(task, Vec::new(), None).await
    }

    /// Sends a plain text user turn (no images).
    pub async fn send_turn(&mut self, task: String) -> Result<ModelResponse> {
        self.send_turn_with_parts(task, Vec::new(), None).await
    }

    /// Creates a [`SessionSnapshot`] of the current session state for persistence.
    pub fn snapshot_session(&mut self) -> Result<SessionSnapshot> {
        self.session.set_updated_at(current_unix_timestamp());
        self.session.update_title_from_events();
        let snapshot = self.session.snapshot(
            &self.project_dir,
            &self.session_store,
            &self.event_bus,
            self.goal_runtime.get_goal(),
        )?;
        self.save_trace_snapshot(snapshot.id.as_str());
        Ok(snapshot)
    }

    /// Creates and persists a session snapshot without blocking the async runtime.
    pub async fn snapshot_session_async(&mut self) -> Result<SessionSnapshot> {
        self.session.set_updated_at(current_unix_timestamp());
        self.session.update_title_from_events();
        let snapshot = self
            .session
            .snapshot_async(
                &self.project_dir,
                &self.session_store,
                &self.event_bus,
                self.goal_runtime.get_goal(),
            )
            .await?;
        self.save_trace_snapshot(snapshot.id.as_str());
        Ok(snapshot)
    }

    fn save_trace_snapshot(&self, session_id: &str) {
        let traces = turn_traces_from_events(
            session_id,
            &self.loaded_config.config.model.provider,
            &self.loaded_config.config.model.name,
            self.session.events(),
        );
        if traces.is_empty() {
            return;
        }
        let store = TraceStore::new(&self.loaded_config.data_dir);
        if let Err(err) = store.save_session_traces(session_id, &traces) {
            tracing::warn!(error = %err, session_id, "failed to save turn traces");
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn session_store(&self) -> &SessionStore {
        &self.session_store
    }

    fn ensure_tool_executor(&mut self) -> Result<Arc<ToolExecutor>> {
        if let Some(executor) = self.tool_executor.as_mut() {
            if let Some(executor) = Arc::get_mut(executor) {
                Self::register_goal_tools_on_executor(executor, self.goal_runtime.clone());
                executor.register_skill_loader(
                    self.project_dir.clone(),
                    self.loaded_config.data_dir.clone(),
                    self.shared_config.clone(),
                );
            }
            return Ok(executor.clone());
        }

        let security_policy = SecurityPolicy::new(
            self.project_dir.clone(),
            self.loaded_config.data_dir.clone(),
            self.loaded_config.config.effective_security_config(),
        )?;
        let harness_policy = crate::harness::select_harness_policy(&self.loaded_config.config);
        let profile_name = format!("{:?}", harness_policy.profile).to_lowercase();
        let mut executor = ToolExecutor::with_security_policy(
            security_policy,
            self.runtime_components.security.clone(),
        );
        executor.set_harness_profile(profile_name);
        Self::register_goal_tools_on_executor(&mut executor, self.goal_runtime.clone());
        executor.register_skill_loader(
            self.project_dir.clone(),
            self.loaded_config.data_dir.clone(),
            self.shared_config.clone(),
        );

        let executor = Arc::new_cyclic(|executor_weak| {
            let subagent = SubagentTool::new(
                executor_weak.clone(),
                self.shared_model_provider.clone(),
                self.project_dir.clone(),
                self.loaded_config.data_dir.clone(),
                self.shared_model_name.clone(),
                self.loaded_config.config.harness.clone(),
                self.shared_config.clone(),
                self.prompt_cache.clone(),
                self.runtime_components.clone(),
            );
            executor.register_tool(Arc::new(subagent));
            let repo_explore = RepoExploreTool::new(
                executor_weak.clone(),
                self.shared_model_provider.clone(),
                self.project_dir.clone(),
                self.loaded_config.data_dir.clone(),
                self.shared_model_name.clone(),
                self.loaded_config.config.harness.clone(),
                self.shared_config.clone(),
                self.prompt_cache.clone(),
                self.runtime_components.clone(),
            );
            executor.register_tool(Arc::new(repo_explore));
            executor
        });
        self.tool_executor = Some(executor.clone());
        Ok(executor)
    }

    fn register_goal_tools_on_executor(
        executor: &mut ToolExecutor,
        goal_runtime: Arc<GoalRuntimeHandle>,
    ) {
        executor.register_tool(Arc::new(GetGoalTool::new(goal_runtime.clone())));
        executor.register_tool(Arc::new(CreateGoalTool::new(goal_runtime.clone())));
        executor.register_tool(Arc::new(UpdateGoalTool::new(goal_runtime.clone())));
        executor.register_tool(Arc::new(UpdateGoalChecklistTool::new(goal_runtime)));
    }

    /// Returns the configured tool executor, if any.
    pub fn tool_executor(&self) -> Option<Arc<ToolExecutor>> {
        self.tool_executor.clone()
    }

    /// Replaces the session tool executor (e.g. after installing WASM plugins).
    pub fn set_tool_executor(&mut self, executor: Arc<ToolExecutor>) {
        self.tool_executor = Some(executor);
    }

    /// Runs a lightweight auto-memory consolidation (stale detection + dedup).
    /// Called on session end. Does not require a model provider.
    fn consolidate_auto_memory(&self) -> Result<()> {
        let memory_config = &self.loaded_config.config.memory;
        if !memory_config.enabled {
            return Ok(());
        }

        let manager = crate::memory::MemoryManager::new(
            self.project_dir.clone(),
            self.loaded_config.data_dir.clone(),
            memory_config,
        )?;

        let db_path = manager.auto_memory.db_path.clone();
        if !db_path.exists() {
            return Ok(());
        }

        let store = crate::memory::AutoMemoryStore::open(&db_path)?;
        let report = store.consolidate(30)?;

        if report.marked_stale > 0 || report.duplicates_merged > 0 {
            tracing::info!(
                "auto-memory consolidation on session end: {} stale, {} duplicates, {} active",
                report.marked_stale,
                report.duplicates_merged,
                report.remaining_active
            );
        }

        Ok(())
    }

    /// extractMemories: background per-turn memory extraction.
    /// Spawns a tokio task that calls the model to extract durable memories
    /// from the completed turn. Fire-and-forget — does not block the agent loop.
    fn try_extract_memories(&self, _session_id: &str, conversation: &str) {
        let memory_config = &self.loaded_config.config.memory;
        if !memory_config.enabled {
            return;
        }

        // Get the model provider and name for the extraction call
        let provider = self.model_provider.clone();
        let model_name = self.loaded_config.config.model.name.clone();
        let conversation = conversation.to_string();

        // Open auto-memory store
        let manager = match crate::memory::MemoryManager::new(
            self.project_dir.clone(),
            self.loaded_config.data_dir.clone(),
            memory_config,
        ) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!("extract-memories: failed to init memory manager: {}", e);
                return;
            }
        };

        let db_path = manager.auto_memory.db_path.clone();
        if !db_path.exists() {
            return;
        }

        let store = match crate::memory::AutoMemoryStore::open(&db_path) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("extract-memories: failed to open store: {}", e);
                return;
            }
        };

        // Fire-and-forget
        tokio::spawn(async move {
            match crate::memory::extract::extract_memories(
                &conversation,
                provider.as_ref(),
                &model_name,
                &store,
            )
            .await
            {
                Ok(n) => {
                    if n > 0 {
                        tracing::info!("extract-memories: saved {} memories from turn", n);
                    }
                }
                Err(e) => {
                    tracing::debug!("extract-memories failed: {}", e);
                }
            }
        });
    }

    /// Auto-dream: checks 3 gates after each turn and spawns consolidation if all pass.
    /// Fire-and-forget — does not block the agent loop.
    fn try_auto_dream(&self) {
        let memory_config = &self.loaded_config.config.memory;
        if !memory_config.enabled {
            return;
        }

        let manager = match crate::memory::MemoryManager::new(
            self.project_dir.clone(),
            self.loaded_config.data_dir.clone(),
            memory_config,
        ) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!("auto-dream: failed to init memory manager: {}", e);
                return;
            }
        };

        let interval_hours = memory_config.dream_interval_days * 24;
        let state =
            crate::memory::auto_dream::AutoDreamState::new(manager.store.memory_root.clone())
                .with_interval(interval_hours.max(1));

        let history = match crate::memory::HistoryStore::new(&manager.history.db_path) {
            Ok(h) => h,
            Err(e) => {
                tracing::debug!("auto-dream: failed to open history: {}", e);
                return;
            }
        };

        let last_dream = state.read_last_dream_at();
        if !state.should_dream(&history) {
            return;
        }

        let db_path = manager.auto_memory.db_path.clone();
        let hours_since = if last_dream > 0 {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            (now.saturating_sub(last_dream)) / 3600
        } else {
            0
        };
        let sessions_count = history.list_sessions().map(|s| s.len()).unwrap_or(0);

        tracing::info!(
            "auto-dream triggered: {}h since last, {} sessions",
            hours_since,
            sessions_count
        );

        self.event_bus.publish(RuntimeEventKind::AutoDreamStarted {
            hours_since_last: hours_since,
            sessions_reviewed: sessions_count,
        });

        let memory_root = manager.store.memory_root.clone();
        let event_sender = self.event_bus.sender();
        tokio::spawn(async move {
            let result = run_auto_dream_consolidation(&db_path).await;

            let dream_state = crate::memory::auto_dream::AutoDreamState::new(memory_root);
            match result {
                Ok(report) => {
                    tracing::info!(
                        "auto-dream completed: {} stale, {} duplicates, {} active",
                        report.marked_stale,
                        report.duplicates_merged,
                        report.remaining_active
                    );
                    dream_state.mark_completed();
                    let _ = event_sender.send(RuntimeEvent::new(
                        RuntimeEventKind::AutoDreamCompleted {
                            marked_stale: report.marked_stale,
                            duplicates_merged: report.duplicates_merged,
                            active_count: report.remaining_active,
                        },
                    ));
                }
                Err(e) => {
                    tracing::warn!("auto-dream failed: {}", e);
                    dream_state.release();
                    let _ =
                        event_sender.send(RuntimeEvent::new(RuntimeEventKind::AutoDreamFailed {
                            reason: e.to_string(),
                        }));
                }
            }
        });
    }

    /// Auto-distill: checks time gate after each turn and spawns distill if enough time passed.
    /// Fire-and-forget — does not block the agent loop.
    fn try_auto_distill(&self) {
        let memory_config = &self.loaded_config.config.memory;
        if !memory_config.enabled || memory_config.distill_interval_days == 0 {
            return;
        }

        let manager = match crate::memory::MemoryManager::new(
            self.project_dir.clone(),
            self.loaded_config.data_dir.clone(),
            memory_config,
        ) {
            Ok(m) => m,
            Err(_) => return,
        };

        let interval_hours = memory_config.distill_interval_days * 24;
        let state = crate::memory::auto_dream::AutoDreamState::new(
            manager.store.memory_root.join("distill"),
        )
        .with_interval(interval_hours)
        .with_min_sessions(3);

        let history = match crate::memory::HistoryStore::new(&manager.history.db_path) {
            Ok(h) => h,
            Err(_) => return,
        };

        if !state.should_dream(&history) {
            return;
        }

        tracing::info!("auto-distill triggered");

        let memory_root = manager.store.memory_root.clone();
        tokio::spawn(async move {
            // Distill only does stale + dedup (no model-based SOP extraction in auto mode)
            let db_path = memory_root.join("memories.db");
            match crate::memory::AutoMemoryStore::open(&db_path) {
                Ok(store) => match store.consolidate(60) {
                    Ok(report) => {
                        tracing::info!(
                            "auto-distill completed: {} stale, {} duplicates, {} active",
                            report.marked_stale,
                            report.duplicates_merged,
                            report.remaining_active
                        );
                        state.mark_completed();
                    }
                    Err(e) => {
                        tracing::warn!("auto-distill failed: {}", e);
                        state.release();
                    }
                },
                Err(e) => {
                    tracing::debug!("auto-distill: store not available: {}", e);
                    state.release();
                }
            }
        });
    }

    fn build_session_runtime(
        &mut self,
    ) -> Result<(
        crate::session::SessionRuntime,
        mpsc::UnboundedReceiver<AgentEvent>,
    )> {
        let tool_executor = self.ensure_tool_executor()?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let memory_injection =
            self.session_store
                .load_memory(&self.project_dir)
                .and_then(|memory| {
                    memory.format_injection(self.loaded_config.config.memory.max_memory_entries)
                });

        // Initialize skill snapshots for prompt rendering.
        *self
            .shared_available_skills
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = self.load_available_skills();
        *self
            .shared_active_skills
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = self.load_active_skills();

        let ctx = Arc::new(crate::turn::TurnContext {
            model_provider: self.shared_model_provider.clone(),
            tool_executor,
            project_dir: self.project_dir.clone(),
            data_dir: self.loaded_config.data_dir.clone(),
            model_name: self.shared_model_name.clone(),
            event_tx: Some(event_tx),
            approval_resolver: self.approval_resolver(),
            question_resolver: self.question_resolver(),
            compact_state: Arc::new(tokio::sync::Mutex::new(crate::compact::CompactState::new(
                crate::config::effective_context_window(&self.loaded_config.config),
            ))),
            harness_config: self.loaded_config.config.harness.clone(),
            include_tool_prompt_manifest: crate::config::effective_tool_prompt_manifest(
                &self.loaded_config.config,
            ),
            context_packets: self.shared_context_packets.clone(),
            available_skills: self.shared_available_skills.clone(),
            active_skills: self.shared_active_skills.clone(),
            prompt_cache: self.prompt_cache.clone(),
            instructions: std::sync::Arc::new(std::sync::RwLock::new(None)),
            components: self.runtime_components.clone(),
            cancel_token: self.cancel_token.clone(),
            config: self.shared_config.clone(),
            memory_injection: memory_injection.clone(),
            compaction_provider: None,
            compaction_model_name: None,
            session_id: self.session.id().as_str().to_string(),
            allowed_tool_names: None,
        });

        let policy = select_harness_policy(&self.loaded_config.config);
        let session_runtime = crate::session::SessionRuntime::spawn(
            ctx,
            policy,
            self.initial_messages.clone(),
            memory_injection,
        );

        Ok((session_runtime, event_rx))
    }

    fn record_event(&mut self, event: AgentEvent) {
        // ── Goal accounting driven by agent events ────────────────
        match &event {
            AgentEvent::ToolCompleted(_result) => {
                // Track if the model used the memory tool with write action during this turn
                if _result.ok {
                    if let Some(output) = _result.output.as_object() {
                        if output.get("tool_name").and_then(|v| v.as_str()) == Some("memory") {
                            self.turn_used_memory_write = true;
                        }
                    }
                }
                let budget_prompt = self.goal_extension.on_tool_complete();
                if budget_prompt.is_some() {
                    if let Some(goal) = self.goal_runtime.get_goal() {
                        self.event_bus.publish(RuntimeEventKind::GoalUpdated {
                            session_id: goal.session_id.clone(),
                            goal_id: goal.goal_id.as_str().to_string(),
                            objective: goal.objective.clone(),
                            short_description: goal.short_description.clone(),
                            status: goal.status,
                            tokens_used: goal.tokens_used,
                            token_budget: goal.token_budget,
                        });
                    }
                }
            }
            AgentEvent::UsageReported {
                input_tokens,
                output_tokens,
                ..
            } => {
                let exceeded = self
                    .goal_extension
                    .on_token_usage(*input_tokens, *output_tokens);
                if exceeded {
                    if let Some(goal) = self.goal_runtime.get_goal() {
                        self.event_bus.publish(RuntimeEventKind::GoalUpdated {
                            session_id: goal.session_id.clone(),
                            goal_id: goal.goal_id.as_str().to_string(),
                            objective: goal.objective.clone(),
                            short_description: goal.short_description.clone(),
                            status: goal.status,
                            tokens_used: goal.tokens_used,
                            token_budget: goal.token_budget,
                        });
                    }
                }
            }
            AgentEvent::SetGoalRequested {
                objective,
                short_description,
                token_budget,
            } => {
                let goal = self.goal_runtime.set_objective_with_short_description(
                    objective.clone(),
                    short_description.clone(),
                    *token_budget,
                );
                self.event_bus.publish(RuntimeEventKind::GoalUpdated {
                    session_id: goal.session_id.clone(),
                    goal_id: goal.goal_id.as_str().to_string(),
                    objective: goal.objective.clone(),
                    short_description: goal.short_description.clone(),
                    status: goal.status,
                    tokens_used: goal.tokens_used,
                    token_budget: goal.token_budget,
                });
            }
            _ => {}
        }

        if let Some(tx) = &self.event_tx {
            let _ = tx.send(event.clone());
        }
        let transient = matches!(
            event,
            AgentEvent::SubagentActivity { .. } | AgentEvent::SubagentTranscript { .. }
        );
        if let Some(kind) = runtime_event_kind_from_agent_event(&event) {
            self.event_bus.publish(kind);
        }
        if !transient {
            self.session.push_event(event);
        }
    }

    fn update_shared_model_state(&self) {
        *self
            .shared_model_provider
            .write()
            .unwrap_or_else(|e| e.into_inner()) = self.model_provider.clone();
        *self
            .shared_model_name
            .write()
            .unwrap_or_else(|e| e.into_inner()) = provider_request_model_name(
            &self.loaded_config.config.model.provider,
            &self.loaded_config.config.model.name,
        );
        *self
            .shared_config
            .write()
            .unwrap_or_else(|e| e.into_inner()) = self.loaded_config.config.clone();
    }

    fn load_active_skills(&self) -> Vec<SkillManifest> {
        match self.discover_available_skills() {
            Ok(skills) => active_skills(
                &skills,
                &self.loaded_config.config.skills.active,
                &self.active_skills,
            ),
            Err(err) => {
                tracing::warn!(error = %err, "failed to load configured skills");
                Vec::new()
            }
        }
    }

    fn load_available_skills(&self) -> Vec<SkillManifest> {
        match self.discover_available_skills() {
            Ok(skills) => skills,
            Err(err) => {
                tracing::warn!(error = %err, "failed to discover configured skills");
                Vec::new()
            }
        }
    }

    fn discover_available_skills(&self) -> Result<Vec<SkillManifest>> {
        discover_configured_skills(
            &self.loaded_config.config.skills,
            &self.project_dir,
            &self.loaded_config.data_dir,
        )
    }
}

fn runtime_event_kind_from_agent_event(event: &AgentEvent) -> Option<RuntimeEventKind> {
    match event {
        AgentEvent::ModelDelta { text } => {
            Some(RuntimeEventKind::AssistantDelta { text: text.clone() })
        }
        AgentEvent::ModelThinkingDelta { text } => {
            Some(RuntimeEventKind::AssistantThinkingDelta { text: text.clone() })
        }
        AgentEvent::ToolRequested(invocation) => {
            Some(RuntimeEventKind::ToolRequested(invocation.clone()))
        }
        AgentEvent::ToolCompleted(result) => Some(RuntimeEventKind::ToolCompleted(result.clone())),
        AgentEvent::SubagentActivity {
            invocation_id,
            message,
        } => Some(RuntimeEventKind::SubagentActivity {
            invocation_id: invocation_id.clone(),
            message: message.clone(),
        }),
        AgentEvent::SubagentTranscript {
            invocation_id,
            item,
        } => Some(RuntimeEventKind::SubagentTranscript {
            invocation_id: invocation_id.clone(),
            item: item.clone(),
        }),
        AgentEvent::ApprovalRequested(request) => {
            Some(RuntimeEventKind::ApprovalRequired(request.clone()))
        }
        AgentEvent::UsageReported {
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        } => Some(RuntimeEventKind::TokensUpdated {
            input_tokens: *input_tokens,
            output_tokens: *output_tokens,
            cache_creation_tokens: *cache_creation_tokens,
            cache_read_tokens: *cache_read_tokens,
        }),
        AgentEvent::Error { message } => Some(RuntimeEventKind::Error {
            message: message.clone(),
        }),
        AgentEvent::ApprovalResolved(decision) => {
            Some(RuntimeEventKind::ApprovalResolved(decision.clone()))
        }
        AgentEvent::CapabilityRecorded(entry) => {
            Some(RuntimeEventKind::CapabilityRecorded(entry.clone()))
        }
        AgentEvent::QuestionRequested(request) => {
            Some(RuntimeEventKind::QuestionRequired(request.clone()))
        }
        AgentEvent::QuestionResolved(response) => {
            Some(RuntimeEventKind::QuestionResolved(response.clone()))
        }
        AgentEvent::HarnessTrace(value) => Some(RuntimeEventKind::HarnessTrace(value.clone())),
        AgentEvent::HarnessStopped {
            reason,
            message,
            tool_name,
        } => Some(RuntimeEventKind::HarnessStopped {
            reason: reason.clone(),
            message: message.clone(),
            tool_name: tool_name.clone(),
        }),
        AgentEvent::PatchProposed(patch) => Some(RuntimeEventKind::PatchProposed(patch.clone())),
        AgentEvent::MicroCompactApplied { messages_cleared } => {
            Some(RuntimeEventKind::MicroCompactApplied {
                messages_cleared: *messages_cleared,
            })
        }
        AgentEvent::AutoCompactStarted => Some(RuntimeEventKind::AutoCompactStarted),
        AgentEvent::AutoCompactCompleted { tokens_saved } => {
            Some(RuntimeEventKind::AutoCompactCompleted {
                tokens_saved: *tokens_saved,
            })
        }
        AgentEvent::AutoCompactFailed { reason } => Some(RuntimeEventKind::AutoCompactFailed {
            reason: reason.clone(),
        }),
        AgentEvent::UserTaskSubmitted { .. } | AgentEvent::ModelOutput { .. } => None,
        AgentEvent::RepeatedToolCallWarning { .. } => None,
        AgentEvent::RepetitionDetected { .. } => None,
        AgentEvent::GoalUpdated { .. } => None,
        AgentEvent::SetGoalRequested {
            objective,
            short_description,
            token_budget,
        } => Some(RuntimeEventKind::SetGoalRequested {
            objective: objective.clone(),
            short_description: short_description.clone(),
            token_budget: *token_budget,
        }),
        AgentEvent::AutoDreamStarted {
            hours_since_last,
            sessions_reviewed,
        } => Some(RuntimeEventKind::AutoDreamStarted {
            hours_since_last: *hours_since_last,
            sessions_reviewed: *sessions_reviewed,
        }),
        AgentEvent::AutoDreamCompleted {
            marked_stale,
            duplicates_merged,
            active_count,
        } => Some(RuntimeEventKind::AutoDreamCompleted {
            marked_stale: *marked_stale,
            duplicates_merged: *duplicates_merged,
            active_count: *active_count,
        }),
        AgentEvent::AutoDreamFailed { reason } => Some(RuntimeEventKind::AutoDreamFailed {
            reason: reason.clone(),
        }),
    }
}

/// Runs the auto-dream consolidation pass (stale + dedup) on the auto-memory SQLite store.
/// Called from a tokio::spawn background task — must not access AgentRuntime state.
async fn run_auto_dream_consolidation(
    db_path: &std::path::Path,
) -> anyhow::Result<crate::memory::ConsolidationReport> {
    let store = crate::memory::AutoMemoryStore::open(db_path)?;
    store.consolidate(30)
}
