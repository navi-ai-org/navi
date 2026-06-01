mod event_bus;
mod session_state;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::agent::AgentMode;
use crate::cancel::CancelToken;
use crate::config::LoadedConfig;
use crate::context::ContextPacket;
use crate::event::{AgentEvent, ApprovalDecision, RuntimeEvent, RuntimeEventKind};
use crate::harness::select_harness_policy;
use crate::model::{ModelMessage, ModelProvider, ModelResponse};
use crate::security::SecurityPolicy;
use crate::session::{SessionId, SessionStore, current_unix_timestamp};
use crate::skills::{SkillManifest, active_skills, discover_configured_skills};
use crate::tool::{Tool, ToolExecutor};
use crate::{
    ModelOption, SessionSnapshot, available_model_options, canonical_provider_id,
    provider_request_model_name,
};
use anyhow::Result;

pub use event_bus::EventBus;
pub use session_state::SessionState;

type PendingApprovals = Arc<std::sync::Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>>;

/// Resolves pending tool approvals by matching decision ids to waiting
/// receivers. Cloneable so it can be handed to the UI layer.
#[derive(Clone)]
pub struct ApprovalResolver {
    pending_approvals: PendingApprovals,
    runtime_events_tx: broadcast::Sender<RuntimeEvent>,
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
    /// Initial agent mode (e.g. `Plan`, `Edit`).
    pub agent_mode: Option<AgentMode>,
    /// Context packets to inject into the session.
    pub context_packets: Vec<ContextPacket>,
    /// Active skill names for this session.
    pub active_skills: Vec<String>,
    /// Seed messages for restoring a session.
    pub initial_messages: Vec<ModelMessage>,
    /// Session id for restoring an existing session.
    pub session_id: Option<SessionId>,
    /// Optional channel for forwarding agent events outside the runtime.
    pub event_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
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
    agent_mode: Option<AgentMode>,
    context_packets: Vec<ContextPacket>,
    shared_context_packets: Arc<std::sync::Mutex<Vec<ContextPacket>>>,
    active_skills: Vec<String>,
    shared_active_skills: Arc<std::sync::Mutex<Vec<crate::skills::SkillManifest>>>,
    initial_messages: Vec<ModelMessage>,
    event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
    cancel_token: CancelToken,
    pending_approvals: PendingApprovals,
    event_bus: EventBus,
    session: SessionState,
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
        let shared_active_skills = Arc::new(std::sync::Mutex::new(Vec::new()));
        let shared_model_provider = Arc::new(RwLock::new(options.model_provider.clone()));
        let shared_model_name = Arc::new(RwLock::new(provider_request_model_name(
            &options.loaded_config.config.model.provider,
            &options.loaded_config.config.model.name,
        )));
        let shared_config = Arc::new(RwLock::new(options.loaded_config.config.clone()));

        Self {
            loaded_config: options.loaded_config,
            model_provider: options.model_provider,
            shared_model_provider,
            shared_model_name,
            shared_config,
            project_dir: options.project_dir,
            tool_executor: options.tool_executor,
            session_store,
            agent_mode: options.agent_mode,
            context_packets: options.context_packets,
            shared_context_packets,
            active_skills: options.active_skills,
            shared_active_skills,
            initial_messages: options.initial_messages,
            event_tx: options.event_tx,
            cancel_token: CancelToken::new(),
            pending_approvals: Arc::new(std::sync::Mutex::new(HashMap::new())),
            event_bus: EventBus::new(),
            session: SessionState::new(options.session_id),
        }
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

    /// Returns the active agent mode, if set.
    pub fn agent_mode(&self) -> Option<AgentMode> {
        self.agent_mode
    }

    /// Sets the agent mode and emits a `ContextUpdated` event.
    pub fn set_agent_mode(&mut self, mode: Option<AgentMode>) {
        self.agent_mode = mode;
        self.event_bus.publish(RuntimeEventKind::ContextUpdated);
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
                self.loaded_config.config.security.clone(),
            )?;
            self.tool_executor = Some(Arc::new(ToolExecutor::new(security_policy)));
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

    /// Returns an [`ApprovalResolver`] handle for external approval resolution.
    pub fn approval_resolver(&self) -> ApprovalResolver {
        ApprovalResolver {
            pending_approvals: self.pending_approvals.clone(),
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
            self.event_bus.publish(RuntimeEventKind::SessionFinished {
                session_id: self.session.id().as_str().to_string(),
            });
        }
        self.cancel_token.reset();
        self.pending_approvals = Arc::new(std::sync::Mutex::new(HashMap::new()));
        self.session.start();

        let (session_runtime, event_rx) = self.build_session_runtime()?;
        self.session.set_runtime(session_runtime, event_rx);

        let id = self.session.id().clone();
        self.event_bus.publish(RuntimeEventKind::SessionStarted {
            session_id: id.as_str().to_string(),
        });

        Ok(id)
    }

    /// Sends a user task to the agent and waits for the full response.
    /// Starts a session automatically if one is not active.
    pub async fn send_turn(&mut self, task: String) -> Result<ModelResponse> {
        if !self.session.started() || self.session.runtime().is_none() {
            self.start_session()?;
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
        self.record_event(AgentEvent::UserTaskSubmitted { text: task.clone() });
        self.event_bus.publish(RuntimeEventKind::TurnStarted {
            turn_id: turn_id.clone(),
        });

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        if let Err(e) = submission_tx.send(crate::session::Submission { task, response_tx }) {
            self.session.set_event_rx(event_rx);
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
        self.session.set_event_rx(event_rx);
        self.session.set_updated_at(current_unix_timestamp());
        self.session.update_title_from_events();

        match &result {
            Ok(text) => {
                self.event_bus.publish(RuntimeEventKind::TurnCompleted {
                    turn_id,
                    text: text.clone(),
                });
            }
            Err(err) => {
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
        self.send_turn(task).await
    }

    /// Creates a [`SessionSnapshot`] of the current session state for persistence.
    pub fn snapshot_session(&mut self) -> Result<SessionSnapshot> {
        self.session.set_updated_at(current_unix_timestamp());
        self.session.update_title_from_events();
        self.session
            .snapshot(&self.project_dir, &self.session_store, &self.event_bus)
    }

    /// Creates and persists a session snapshot without blocking the async runtime.
    pub async fn snapshot_session_async(&mut self) -> Result<SessionSnapshot> {
        self.session.set_updated_at(current_unix_timestamp());
        self.session.update_title_from_events();
        self.session
            .snapshot_async(&self.project_dir, &self.session_store, &self.event_bus)
            .await
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn session_store(&self) -> &SessionStore {
        &self.session_store
    }

    fn ensure_tool_executor(&mut self) -> Result<Arc<ToolExecutor>> {
        if let Some(executor) = self.tool_executor.clone() {
            return Ok(executor);
        }

        let security_policy = SecurityPolicy::new(
            self.project_dir.clone(),
            self.loaded_config.data_dir.clone(),
            self.loaded_config.config.security.clone(),
        )?;
        let executor = Arc::new(ToolExecutor::new(security_policy));
        self.tool_executor = Some(executor.clone());
        Ok(executor)
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

        // Initialize shared_active_skills with current loaded skills
        *self
            .shared_active_skills
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = self.load_active_skills();

        let ctx = Arc::new(crate::turn::TurnContext {
            model_provider: self.shared_model_provider.clone(),
            tool_executor,
            agent_control: crate::agent::AgentControl::new(),
            project_dir: self.project_dir.clone(),
            model_name: self.shared_model_name.clone(),
            event_tx: Some(event_tx),
            approval_resolver: self.approval_resolver(),
            compact_state: Arc::new(tokio::sync::Mutex::new(crate::compact::CompactState::new(
                crate::config::effective_context_window(&self.loaded_config.config),
            ))),
            harness_config: self.loaded_config.config.harness.clone(),
            include_tool_prompt_manifest: crate::config::effective_tool_prompt_manifest(
                &self.loaded_config.config,
            ),
            agent_mode: self.agent_mode,
            context_packets: self.shared_context_packets.clone(),
            active_skills: self.shared_active_skills.clone(),
            cancel_token: self.cancel_token.clone(),
            config: self.shared_config.clone(),
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
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(event.clone());
        }
        if let Some(kind) = runtime_event_kind_from_agent_event(&event) {
            self.event_bus.publish(kind);
        }
        self.session.push_event(event);
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
        match discover_configured_skills(
            &self.loaded_config.config.skills,
            &self.project_dir,
            &self.loaded_config.data_dir,
        ) {
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
        AgentEvent::ApprovalRequested(request) => {
            Some(RuntimeEventKind::ApprovalRequired(request.clone()))
        }
        AgentEvent::UsageReported {
            input_tokens,
            output_tokens,
        } => Some(RuntimeEventKind::TokensUpdated {
            input_tokens: *input_tokens,
            output_tokens: *output_tokens,
        }),
        AgentEvent::Error { message } => Some(RuntimeEventKind::Error {
            message: message.clone(),
        }),
        AgentEvent::ApprovalResolved(decision) => {
            Some(RuntimeEventKind::ApprovalResolved(decision.clone()))
        }
        AgentEvent::HarnessTrace(value) => Some(RuntimeEventKind::HarnessTrace(value.clone())),
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
    }
}
