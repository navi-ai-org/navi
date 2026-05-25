use crate::agent::AgentMode;
use crate::config::LoadedConfig;
use crate::context::ContextPacket;
use crate::event::{AgentEvent, ApprovalDecision, RuntimeEvent, RuntimeEventKind};
use crate::harness::select_harness_policy;
use crate::model::{ModelMessage, ModelProvider, ModelResponse};
use crate::security::SecurityPolicy;
use crate::session::{current_unix_timestamp, session_title_from_events};
use crate::skills::{SkillManifest, active_skills, discover_configured_skills};
use crate::tool::{Tool, ToolExecutor};
use crate::{
    ModelOption, SessionId, SessionSnapshot, SessionStore, available_model_options,
    canonical_provider_id, provider_request_model_name,
};
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Notify, broadcast, mpsc, oneshot};

type PendingApprovals = Arc<std::sync::Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>>;

#[derive(Clone)]
pub struct ApprovalResolver {
    pending_approvals: PendingApprovals,
    runtime_events_tx: broadcast::Sender<RuntimeEvent>,
}

impl ApprovalResolver {
    pub fn resolve(&self, decision: ApprovalDecision) -> bool {
        let id = match &decision {
            ApprovalDecision::Approved { id } => id,
            ApprovalDecision::Denied { id } => id,
        };
        if let Some(tx) = self.pending_approvals.lock().unwrap().remove(id) {
            let _ = tx.send(decision.clone());
            let _ =
                self.runtime_events_tx
                    .send(RuntimeEvent::new(RuntimeEventKind::LegacyAgentEvent(
                        AgentEvent::ApprovalResolved(decision),
                    )));
            true
        } else {
            false
        }
    }
}

#[derive(Clone)]
pub struct TurnCanceller {
    cancel_requested: Arc<AtomicBool>,
    cancel_notify: Arc<Notify>,
}

impl TurnCanceller {
    pub fn cancel(&self) {
        self.cancel_requested.store(true, Ordering::SeqCst);
        self.cancel_notify.notify_waiters();
    }
}

pub struct AgentRuntimeOptions {
    pub loaded_config: LoadedConfig,
    pub model_provider: Arc<dyn ModelProvider>,
    pub project_dir: PathBuf,
    pub tool_executor: Option<Arc<ToolExecutor>>,
    pub agent_mode: Option<AgentMode>,
    pub context_packets: Vec<ContextPacket>,
    pub active_skills: Vec<String>,
    pub initial_messages: Vec<ModelMessage>,
    pub session_id: Option<SessionId>,
    pub event_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
}

pub struct AgentRuntime {
    loaded_config: LoadedConfig,
    model_provider: Arc<dyn ModelProvider>,
    project_dir: PathBuf,
    tool_executor: Option<Arc<ToolExecutor>>,
    session_store: SessionStore,
    agent_mode: Option<AgentMode>,
    context_packets: Vec<ContextPacket>,
    active_skills: Vec<String>,
    initial_messages: Vec<ModelMessage>,
    event_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    runtime_events_tx: broadcast::Sender<RuntimeEvent>,
    session_runtime: Option<crate::session::SessionRuntime>,
    session_event_rx: Option<mpsc::UnboundedReceiver<AgentEvent>>,
    session_id: SessionId,
    session_created_at: u64,
    session_updated_at: u64,
    session_title: Option<String>,
    turn_sequence: u64,
    session_started: bool,
    cancel_requested: Arc<AtomicBool>,
    cancel_notify: Arc<Notify>,
    pending_approvals: PendingApprovals,
    requested_session_id: Option<SessionId>,
    events: Vec<AgentEvent>,
}

pub type NaviRuntime = AgentRuntime;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ApprovalConfig, HarnessConfig, ModelRequest, ModelStream, NaviConfig, SecurityConfig,
        ToolInvocation,
    };
    use anyhow::Result;
    use async_trait::async_trait;
    use futures_util::stream;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::time::timeout;

    struct MockToolProvider {
        calls: Arc<Mutex<usize>>,
        file_path: String,
    }

    #[async_trait]
    impl ModelProvider for MockToolProvider {
        fn stream(&self, request: ModelRequest) -> ModelStream {
            let mut calls = self.calls.lock().expect("calls");
            *calls += 1;
            let call_number = *calls;
            drop(calls);

            if call_number == 1 {
                assert!(!request.tools.is_empty());
                assert!(request.messages[0].content.contains("Workflow contract"));
                assert!(request.messages[0].content.contains("Agent mode: Plan"));
                assert!(request.messages[0].content.contains("runtime context"));
                Box::pin(stream::iter(vec![Ok(
                    crate::model::ModelStreamEvent::ToolCall(ToolInvocation {
                        id: "call-1".to_string(),
                        tool_name: "read_file".to_string(),
                        input: json!({ "path": self.file_path }),
                    }),
                )]))
            } else {
                assert!(
                    request
                        .messages
                        .iter()
                        .any(|message| message.role == crate::model::ModelRole::Tool)
                );
                Box::pin(stream::iter(vec![
                    Ok(crate::model::ModelStreamEvent::TextDelta {
                        text: "read complete".to_string(),
                    }),
                    Ok(crate::model::ModelStreamEvent::Done),
                ]))
            }
        }

        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse> {
            ModelProvider::complete(self, request).await
        }
    }

    struct SimpleProvider;

    #[async_trait]
    impl ModelProvider for SimpleProvider {
        fn stream(&self, _request: ModelRequest) -> ModelStream {
            Box::pin(stream::iter(vec![
                Ok(crate::model::ModelStreamEvent::TextDelta {
                    text: "simple".to_string(),
                }),
                Ok(crate::model::ModelStreamEvent::Done),
            ]))
        }
    }

    #[tokio::test]
    async fn headless_runtime_executes_read_tools_and_continues() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let file = tempdir.path().join("Cargo.toml");
        std::fs::write(&file, "[package]\nname = \"demo\"\n").expect("write file");
        let loaded_config = crate::LoadedConfig {
            config: NaviConfig {
                harness: HarnessConfig::default(),
                approvals: ApprovalConfig::default(),
                security: SecurityConfig::default(),
                ..NaviConfig::default()
            },
            global_config_path: None,
            project_config_path: None,
            data_dir: tempdir.path().join("data"),
        };
        let provider = Arc::new(MockToolProvider {
            calls: Arc::new(Mutex::new(0)),
            file_path: file.display().to_string(),
        });
        let mut runtime = AgentRuntime::new(AgentRuntimeOptions {
            loaded_config,
            model_provider: provider,
            project_dir: tempdir.path().to_path_buf(),
            tool_executor: None,
            agent_mode: Some(crate::AgentMode::Plan),
            context_packets: vec![crate::ContextPacket {
                id: Some("ctx-1".to_string()),
                source: crate::ContextSource::FocusThread,
                title: Some("focus".to_string()),
                content: "runtime context".to_string(),
                priority: 10,
                metadata: json!({}),
            }],
            active_skills: Vec::new(),
            initial_messages: Vec::new(),
            session_id: None,
            event_tx: None,
        });

        let response = runtime
            .submit_task("inspect".to_string())
            .await
            .expect("run");

        assert_eq!(response.text, "read complete");
        assert!(
            runtime
                .events()
                .iter()
                .any(|event| matches!(event, AgentEvent::ToolCompleted(_)))
        );
        assert!(
            runtime
                .events()
                .iter()
                .any(|event| matches!(event, AgentEvent::HarnessTrace(_)))
        );
    }

    #[tokio::test]
    async fn runtime_session_lifecycle_streams_events_and_snapshots() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let loaded_config = crate::LoadedConfig {
            config: NaviConfig {
                harness: HarnessConfig::default(),
                approvals: ApprovalConfig::default(),
                security: SecurityConfig::default(),
                ..NaviConfig::default()
            },
            global_config_path: None,
            project_config_path: None,
            data_dir: tempdir.path().join("data"),
        };
        let provider = Arc::new(SimpleProvider);
        let mut runtime = AgentRuntime::new(AgentRuntimeOptions {
            loaded_config,
            model_provider: provider,
            project_dir: tempdir.path().to_path_buf(),
            tool_executor: None,
            agent_mode: Some(crate::AgentMode::Plan),
            context_packets: vec![crate::ContextPacket {
                id: Some("ctx-1".to_string()),
                source: crate::ContextSource::FocusThread,
                title: Some("focus".to_string()),
                content: "runtime context".to_string(),
                priority: 10,
                metadata: json!({}),
            }],
            active_skills: Vec::new(),
            initial_messages: Vec::new(),
            session_id: None,
            event_tx: None,
        });

        let mut events = runtime.stream_events();
        let session_id = runtime.start_session().expect("start session");

        let first_event = timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("session event timeout")
            .expect("session event");
        assert!(matches!(
            first_event.kind,
            RuntimeEventKind::SessionStarted { session_id: ref id } if id == &session_id.0
        ));

        let response = runtime
            .send_turn("inspect".to_string())
            .await
            .expect("run turn");
        assert_eq!(response.text, "simple");

        let mut saw_turn_started = false;
        let mut saw_turn_completed = false;
        for _ in 0..8 {
            let event = timeout(Duration::from_secs(1), events.recv())
                .await
                .expect("turn event timeout")
                .expect("turn event");
            match event.kind {
                RuntimeEventKind::TurnStarted { .. } => saw_turn_started = true,
                RuntimeEventKind::TurnCompleted { ref text, .. } => {
                    saw_turn_completed = true;
                    assert_eq!(text, "simple");
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_turn_started);
        assert!(saw_turn_completed);

        let snapshot = runtime.snapshot_session().expect("snapshot");
        assert_eq!(snapshot.id.0, session_id.0);
        assert!(snapshot.title.is_some());
        let snapshot_path = runtime
            .session_store
            .root()
            .join(format!("{}.json", snapshot.id.0));
        assert!(snapshot_path.exists());
    }

    #[tokio::test]
    async fn runtime_uses_requested_session_id_once() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let loaded_config = crate::LoadedConfig {
            config: NaviConfig {
                harness: HarnessConfig::default(),
                approvals: ApprovalConfig::default(),
                security: SecurityConfig::default(),
                ..NaviConfig::default()
            },
            global_config_path: None,
            project_config_path: None,
            data_dir: tempdir.path().join("data"),
        };
        let provider = Arc::new(SimpleProvider);
        let mut runtime = AgentRuntime::new(AgentRuntimeOptions {
            loaded_config,
            model_provider: provider,
            project_dir: tempdir.path().to_path_buf(),
            tool_executor: None,
            agent_mode: Some(crate::AgentMode::Tutor),
            context_packets: Vec::new(),
            active_skills: Vec::new(),
            initial_messages: Vec::new(),
            session_id: Some(SessionId(
                "navi_tutor_algoritmos_2026-05-25_14-32-10".to_string(),
            )),
            event_tx: None,
        });

        let first_id = runtime.start_session().expect("start first session");
        let second_id = runtime.start_session().expect("start second session");

        assert_eq!(first_id.0, "navi_tutor_algoritmos_2026-05-25_14-32-10");
        assert_ne!(second_id.0, first_id.0);
    }
}

impl AgentRuntime {
    pub fn new(options: AgentRuntimeOptions) -> Self {
        let session_store = SessionStore::with_redaction(
            options.loaded_config.data_dir.clone(),
            options
                .loaded_config
                .config
                .security
                .redact_secrets_in_sessions,
        );
        let (runtime_events_tx, _) = broadcast::channel(256);
        let now = current_unix_timestamp();

        Self {
            loaded_config: options.loaded_config,
            model_provider: options.model_provider,
            project_dir: options.project_dir,
            tool_executor: options.tool_executor,
            session_store,
            agent_mode: options.agent_mode,
            context_packets: options.context_packets,
            active_skills: options.active_skills,
            initial_messages: options.initial_messages,
            event_tx: options.event_tx,
            runtime_events_tx,
            session_runtime: None,
            session_event_rx: None,
            session_id: options
                .session_id
                .clone()
                .unwrap_or_else(SessionStore::create_id),
            session_created_at: now,
            session_updated_at: now,
            session_title: None,
            turn_sequence: 0,
            session_started: false,
            cancel_requested: Arc::new(AtomicBool::new(false)),
            cancel_notify: Arc::new(Notify::new()),
            pending_approvals: Arc::new(std::sync::Mutex::new(HashMap::new())),
            requested_session_id: options.session_id,
            events: Vec::new(),
        }
    }

    pub fn events(&self) -> &[AgentEvent] {
        &self.events
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn session_title(&self) -> Option<&str> {
        self.session_title.as_deref()
    }

    pub fn agent_mode(&self) -> Option<AgentMode> {
        self.agent_mode
    }

    pub fn set_agent_mode(&mut self, mode: Option<AgentMode>) {
        self.agent_mode = mode;
        self.publish_runtime_event(RuntimeEventKind::ContextUpdated);
    }

    pub fn add_context_packet(&mut self, packet: ContextPacket) {
        self.context_packets.push(packet);
        self.publish_runtime_event(RuntimeEventKind::ContextUpdated);
    }

    pub fn clear_context_packets(&mut self) {
        self.context_packets.clear();
        self.publish_runtime_event(RuntimeEventKind::ContextUpdated);
    }

    pub fn context_packets(&self) -> &[ContextPacket] {
        &self.context_packets
    }

    pub fn set_active_skills(&mut self, skills: Vec<String>) {
        self.active_skills = skills;
        self.publish_runtime_event(RuntimeEventKind::ContextUpdated);
    }

    pub fn list_models(&self) -> Vec<ModelOption> {
        available_model_options(&self.loaded_config.config)
    }

    pub fn set_model(&mut self, provider: impl Into<String>, model: impl Into<String>) {
        self.loaded_config.config.model.provider =
            canonical_provider_id(&provider.into()).to_string();
        self.loaded_config.config.model.name = model.into();
        self.publish_runtime_event(RuntimeEventKind::ContextUpdated);
    }

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
        self.publish_runtime_event(RuntimeEventKind::ContextUpdated);
        Ok(())
    }

    pub fn stream_events(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.runtime_events_tx.subscribe()
    }

    pub fn cancel_turn(&self) {
        self.turn_canceller().cancel();
    }

    pub fn resolve_approval(&self, decision: ApprovalDecision) -> bool {
        self.approval_resolver().resolve(decision)
    }

    pub fn approval_resolver(&self) -> ApprovalResolver {
        ApprovalResolver {
            pending_approvals: self.pending_approvals.clone(),
            runtime_events_tx: self.runtime_events_tx.clone(),
        }
    }

    pub fn turn_canceller(&self) -> TurnCanceller {
        TurnCanceller {
            cancel_requested: self.cancel_requested.clone(),
            cancel_notify: self.cancel_notify.clone(),
        }
    }

    pub fn start_session(&mut self) -> Result<SessionId> {
        if self.session_started {
            self.publish_runtime_event(RuntimeEventKind::SessionFinished {
                session_id: self.session_id.0.clone(),
            });
        }
        self.cancel_requested.store(false, Ordering::SeqCst);
        self.cancel_notify = Arc::new(Notify::new());
        self.pending_approvals = Arc::new(std::sync::Mutex::new(HashMap::new()));
        self.session_runtime = None;
        self.session_event_rx = None;
        self.session_id = self
            .requested_session_id
            .take()
            .unwrap_or_else(SessionStore::create_id);
        self.session_created_at = current_unix_timestamp();
        self.session_updated_at = self.session_created_at;
        self.session_title = None;
        self.turn_sequence = 0;
        self.session_started = true;
        self.events.clear();

        let (session_runtime, event_rx) = self.build_session_runtime()?;
        self.session_runtime = Some(session_runtime);
        self.session_event_rx = Some(event_rx);

        self.publish_runtime_event(RuntimeEventKind::SessionStarted {
            session_id: self.session_id.0.clone(),
        });

        Ok(self.session_id.clone())
    }

    pub async fn send_turn(&mut self, task: String) -> Result<ModelResponse> {
        if !self.session_started || self.session_runtime.is_none() {
            self.start_session()?;
        }

        let submission_tx = self
            .session_runtime
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("session not started"))?
            .submission_tx
            .clone();

        let mut event_rx = self
            .session_event_rx
            .take()
            .ok_or_else(|| anyhow::anyhow!("session event stream unavailable"))?;

        self.cancel_requested.store(false, Ordering::SeqCst);

        let turn_id = self.next_turn_id();
        tracing::info!(
            project = %self.project_dir.display(),
            provider = %self.loaded_config.config.model.provider,
            model = %self.loaded_config.config.model.name,
            "agent task submitted"
        );
        self.record_event(AgentEvent::UserTaskSubmitted { text: task.clone() });
        self.publish_runtime_event(RuntimeEventKind::TurnStarted {
            turn_id: turn_id.clone(),
        });

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        if let Err(e) = submission_tx.send(crate::session::Submission { task, response_tx }) {
            self.session_event_rx = Some(event_rx);
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
        self.session_event_rx = Some(event_rx);

        self.session_updated_at = current_unix_timestamp();
        self.session_title = session_title_from_events(&self.events);

        match &result {
            Ok(text) => {
                self.publish_runtime_event(RuntimeEventKind::TurnCompleted {
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

    pub fn snapshot_session(&mut self) -> Result<SessionSnapshot> {
        self.session_updated_at = current_unix_timestamp();
        self.session_title = session_title_from_events(&self.events);
        let snapshot = SessionSnapshot {
            id: self.session_id.clone(),
            title: self.session_title.clone(),
            project: self.project_dir.clone(),
            created_at: self.session_created_at,
            updated_at: self.session_updated_at,
            events: self.events.clone(),
            memory: self.session_store.load_memory(&self.project_dir),
        };
        self.session_store.save(&snapshot)?;
        self.publish_runtime_event(RuntimeEventKind::SessionSaved {
            session_id: snapshot.id.0.clone(),
        });
        Ok(snapshot)
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
        let ctx = Arc::new(crate::turn::TurnContext {
            model_provider: self.model_provider.clone(),
            tool_executor,
            agent_control: crate::agent::AgentControl::new(),
            project_dir: self.project_dir.clone(),
            model_name: provider_request_model_name(
                &self.loaded_config.config.model.provider,
                &self.loaded_config.config.model.name,
            ),
            event_tx: Some(event_tx),
            pending_approvals: self.pending_approvals.clone(),
            compact_state: Arc::new(tokio::sync::Mutex::new(crate::compact::CompactState::new(
                crate::config::effective_context_window(&self.loaded_config.config),
            ))),
            harness_config: self.loaded_config.config.harness.clone(),
            include_tool_prompt_manifest: crate::config::effective_tool_prompt_manifest(
                &self.loaded_config.config,
            ),
            agent_mode: self.agent_mode,
            context_packets: self.context_packets.clone(),
            active_skills: self.load_active_skills(),
            cancel_requested: self.cancel_requested.clone(),
            cancel_notify: self.cancel_notify.clone(),
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
            self.publish_runtime_event(kind);
        }
        self.events.push(event);
        self.session_updated_at = current_unix_timestamp();
        self.session_title = session_title_from_events(&self.events);
    }

    fn publish_runtime_event(&self, kind: RuntimeEventKind) {
        let _ = self.runtime_events_tx.send(RuntimeEvent::new(kind));
    }

    fn next_turn_id(&mut self) -> String {
        self.turn_sequence += 1;
        format!("{}-turn-{}", self.session_id.0, self.turn_sequence)
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
        _ => Some(RuntimeEventKind::LegacyAgentEvent(event.clone())),
    }
}
