//! A `MockEngine` for integration tests.
//!
//! Implements [`EngineDriver`] by recording every call, returning canned
//! responses, and forwarding `RuntimeEvent`s pushed via [`MockEngine::push_event`]
//! to a `tokio::sync::broadcast` channel that the TUI subscribes to.

use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::{Notify, broadcast};

use navi_sdk::{
    ApprovalDecision, EngineDriver, LoadedConfig, McpServerInfo, NaviConfig, NaviConfigSaveTarget,
    NaviError, NaviModelSelectionRequest, NaviModelSelectionResult, NaviProviderCredentialStatus,
    NaviProviderSyncReport, NaviSessionInfo, NaviSessionRequest, NaviSkillInfo, NaviTurnRequest,
    NaviTurnResponse, NaviUsageReport, PlanReviewResponse, QuestionResponse, RuntimeEvent,
    SessionSnapshot, SudoPasswordResponse,
};

/// A recorded call to a method on the engine.
#[derive(Debug, Clone)]
pub enum EngineCall {
    StartSession(NaviSessionRequest),
    SubscribeEvents(String),
    SendTurn(NaviTurnRequest),
    CancelTurn(String),
    ResolveApproval {
        session_id: String,
        decision: ApprovalDecision,
    },
    ResolveQuestion {
        session_id: String,
        response: QuestionResponse,
    },
    ResolvePlanReview {
        session_id: String,
        response: PlanReviewResponse,
    },
    ResolveSudoPassword {
        session_id: String,
        /// Only records whether submitted — never the secret.
        submitted: bool,
    },
    SnapshotSession(String),
    ReloadWasmPlugins,
    SyncModels(NaviConfigSaveTarget),
    SyncProviderModels {
        provider_id: String,
        target: NaviConfigSaveTarget,
    },
    SelectModel(NaviModelSelectionRequest),
    LoadedConfig,
    UsageReport,
    CredentialStatus(String),
    SetProviderApiKey {
        provider_id: String,
        api_key: String,
    },
    ListSkills,
    DeleteSavedSession(String),
}

pub type Result<T> = std::result::Result<T, NaviError>;

const MOCK_EVENT_CHANNEL_CAPACITY: usize = 64;

/// A mock engine for use in TUI integration tests. `Send + Sync`.
pub struct MockEngine {
    state: Mutex<MockState>,
    events_tx: broadcast::Sender<RuntimeEvent>,
    /// Used to keep `send_turn` pending until the test calls
    /// [`Self::complete_turn`], so the TUI's spawned turn task can pick up
    /// `RuntimeEvent`s the test pushed via [`Self::push_event`].
    turn_done: Notify,
}

struct MockState {
    calls: Vec<EngineCall>,
    loaded_config: LoadedConfig,
    skills: Vec<NaviSkillInfo>,
    usage_report: NaviUsageReport,
    /// Default `credential_status(...).configured`. False matches a real
    /// engine with an empty credential store (most unit tests).
    credentials_configured: bool,
}

impl MockEngine {
    /// Create a new mock engine with a default empty config and no skills.
    pub fn new() -> Self {
        Self::with_config(LoadedConfig {
            config: NaviConfig::default(),
            global_config_path: None,
            project_config_path: None,
            data_dir: PathBuf::from("/tmp/navi-mock-data"),
        })
    }

    /// Create a mock engine preloaded with a specific config.
    pub fn with_config(loaded_config: LoadedConfig) -> Self {
        let (events_tx, _) = broadcast::channel(MOCK_EVENT_CHANNEL_CAPACITY);
        Self {
            state: Mutex::new(MockState {
                calls: Vec::new(),
                loaded_config,
                skills: Vec::new(),
                usage_report: NaviUsageReport::default(),
                credentials_configured: false,
            }),
            events_tx,
            turn_done: Notify::new(),
        }
    }

    /// Control whether [`EngineDriver::credential_status`] reports keys present.
    pub fn set_credentials_configured(&self, configured: bool) {
        self.state.lock().unwrap().credentials_configured = configured;
    }

    /// Signal that the current in-flight `send_turn` future should resolve.
    /// The TUI's spawned turn task will then complete and forward any
    /// `RuntimeEvent`s that were pushed before this call.
    pub fn complete_turn(&self) {
        self.turn_done.notify_one();
    }

    /// Push a [`RuntimeEvent`] to all subscribed TUI sessions. Returns
    /// immediately; receivers created via `subscribe_events` will see this
    /// event.
    pub fn push_event(&self, event: RuntimeEvent) {
        // It's fine if there are no subscribers (e.g. before the TUI subscribes).
        let _ = self.events_tx.send(event);
    }

    /// Snapshot of all recorded calls (cloned).
    pub fn calls(&self) -> Vec<EngineCall> {
        self.state.lock().unwrap().calls.clone()
    }

    /// Drain and return all recorded calls.
    pub fn take_calls(&self) -> Vec<EngineCall> {
        std::mem::take(&mut self.state.lock().unwrap().calls)
    }

    /// Number of recorded calls so far.
    pub fn call_count(&self) -> usize {
        self.state.lock().unwrap().calls.len()
    }

    /// Replace the canned loaded config returned by `loaded_config` /
    /// `select_model` / etc.
    pub fn set_loaded_config(&self, config: LoadedConfig) {
        self.state.lock().unwrap().loaded_config = config;
    }

    /// Replace the canned skills returned by `list_skills`.
    pub fn set_skills(&self, skills: Vec<NaviSkillInfo>) {
        self.state.lock().unwrap().skills = skills;
    }

    /// Replace the canned usage report returned by `usage_report`.
    pub fn set_usage_report(&self, report: NaviUsageReport) {
        self.state.lock().unwrap().usage_report = report;
    }
}

impl Default for MockEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EngineDriver for MockEngine {
    async fn start_session(&self, request: NaviSessionRequest) -> Result<NaviSessionInfo> {
        let req = request.clone();
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::StartSession(request));
        let session_id = req.session_id.unwrap_or_else(|| "mock-session".to_string());
        Ok(NaviSessionInfo {
            id: session_id,
            project_dir: req
                .project_dir
                .unwrap_or_else(|| PathBuf::from("/tmp/navi-mock-project")),
            model: "mock-model".to_string(),
            provider: "mock".to_string(),
        })
    }

    fn subscribe_events(&self, session_id: &str) -> Result<broadcast::Receiver<RuntimeEvent>> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::SubscribeEvents(session_id.to_string()));
        Ok(self.events_tx.subscribe())
    }

    async fn send_turn(&self, request: NaviTurnRequest) -> Result<NaviTurnResponse> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::SendTurn(request.clone()));
        // Block until the test signals completion, so the TUI's spawned
        // turn task can race event reception against this future.
        self.turn_done.notified().await;
        // The real streaming happens via the broadcast channel; the turn
        // resolves to an empty response. Tests that need a completed
        // response push a `TurnCompleted` event to the channel.
        Ok(NaviTurnResponse {
            session_id: request.session_id,
            text: String::new(),
        })
    }

    async fn clear_goal(&self, _session_id: &str) -> Result<()> {
        Ok(())
    }

    async fn cancel_turn(&self, session_id: &str) -> Result<()> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::CancelTurn(session_id.to_string()));
        Ok(())
    }

    async fn rewind_session(&self, _session_id: &str, keep_user_turns: usize) -> Result<usize> {
        Ok(keep_user_turns)
    }

    fn agent_mode(&self, _session_id: &str) -> Result<navi_core::plan_mode::AgentMode> {
        Ok(navi_core::plan_mode::AgentMode::Default)
    }

    async fn enter_plan_mode(&self, _session_id: &str) -> Result<()> {
        Ok(())
    }

    async fn exit_plan_mode(&self, _session_id: &str) -> Result<()> {
        Ok(())
    }

    async fn resolve_approval(&self, session_id: &str, decision: ApprovalDecision) -> Result<bool> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::ResolveApproval {
                session_id: session_id.to_string(),
                decision,
            });
        Ok(true)
    }

    async fn resolve_question(&self, session_id: &str, response: QuestionResponse) -> Result<bool> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::ResolveQuestion {
                session_id: session_id.to_string(),
                response,
            });
        Ok(true)
    }

    async fn resolve_plan_review(
        &self,
        session_id: &str,
        response: PlanReviewResponse,
    ) -> Result<bool> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::ResolvePlanReview {
                session_id: session_id.to_string(),
                response,
            });
        Ok(true)
    }

    async fn resolve_sudo_password(
        &self,
        session_id: &str,
        response: SudoPasswordResponse,
    ) -> Result<bool> {
        let submitted = matches!(response, SudoPasswordResponse::Submitted { .. });
        // Drop response (and password) immediately after noting the call.
        drop(response);
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::ResolveSudoPassword {
                session_id: session_id.to_string(),
                submitted,
            });
        Ok(true)
    }

    async fn snapshot_session(&self, session_id: &str) -> Result<SessionSnapshot> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::SnapshotSession(session_id.to_string()));
        Err(NaviError::SessionNotFound(session_id.to_string()))
    }

    async fn close_session(&self, _session_id: &str) -> Result<bool> {
        Ok(false)
    }

    async fn reload_wasm_plugins(&self) -> Result<Vec<String>> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::ReloadWasmPlugins);
        Ok(Vec::new())
    }

    async fn sync_registry(&self, _force: bool) -> Result<bool> {
        Ok(false)
    }

    async fn sync_models(&self, target: NaviConfigSaveTarget) -> Result<NaviProviderSyncReport> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::SyncModels(target));
        let loaded_config = self.state.lock().unwrap().loaded_config.clone();
        Ok(NaviProviderSyncReport {
            loaded_config,
            saved_to: None,
            updated: Vec::new(),
            failed: Vec::new(),
            skipped: Vec::new(),
        })
    }

    async fn sync_provider_models(
        &self,
        provider_id: &str,
        target: NaviConfigSaveTarget,
    ) -> Result<NaviProviderSyncReport> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::SyncProviderModels {
                provider_id: provider_id.to_string(),
                target,
            });
        let loaded_config = self.state.lock().unwrap().loaded_config.clone();
        Ok(NaviProviderSyncReport {
            loaded_config,
            saved_to: None,
            updated: Vec::new(),
            failed: Vec::new(),
            skipped: Vec::new(),
        })
    }

    fn select_model(&self, request: NaviModelSelectionRequest) -> Result<NaviModelSelectionResult> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::SelectModel(request));
        let loaded_config = self.state.lock().unwrap().loaded_config.clone();
        Ok(NaviModelSelectionResult {
            context_window_tokens: None,
            loaded_config,
            saved_to: None,
            provider_id: "mock".to_string(),
            model: "mock-model".to_string(),
            provider_configured: true,
        })
    }

    fn set_background_model(
        &self,
        _task: &str,
        _provider: &str,
        _model: &str,
        _target: NaviConfigSaveTarget,
    ) -> Result<()> {
        Ok(())
    }

    fn clear_background_model(
        &self,
        _task: &str,
        _target: NaviConfigSaveTarget,
    ) -> Result<()> {
        Ok(())
    }

    fn set_attachment_model(
        &self,
        modality: &str,
        provider: &str,
        model: &str,
        _target: NaviConfigSaveTarget,
    ) -> Result<()> {
        use navi_core::config::types::ModelConfig;
        let entry = ModelConfig {
            provider: provider.to_string(),
            name: model.to_string(),
        };
        let mut guard = self.state.lock().unwrap();
        let am = &mut guard.loaded_config.config.attachment_models;
        match modality {
            "image" => am.image = Some(entry),
            "audio" => am.audio = Some(entry),
            "video" => am.video = Some(entry),
            "document" => am.document = Some(entry),
            _ => {}
        }
        Ok(())
    }

    fn clear_attachment_model(
        &self,
        modality: &str,
        _target: NaviConfigSaveTarget,
    ) -> Result<()> {
        let mut guard = self.state.lock().unwrap();
        let am = &mut guard.loaded_config.config.attachment_models;
        match modality {
            "image" => am.image = None,
            "audio" => am.audio = None,
            "video" => am.video = None,
            "document" => am.document = None,
            _ => {}
        }
        Ok(())
    }

    fn loaded_config(&self) -> LoadedConfig {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::LoadedConfig);
        self.state.lock().unwrap().loaded_config.clone()
    }

    fn memory_quick_status(&self) -> Result<String> {
        Ok("on · 0 active · embeddings missing".into())
    }

    async fn usage_report(&self) -> Result<NaviUsageReport> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::UsageReport);
        Ok(self.state.lock().unwrap().usage_report.clone())
    }

    fn credential_status(&self, provider_id: &str) -> Result<NaviProviderCredentialStatus> {
        let mut state = self.state.lock().unwrap();
        state
            .calls
            .push(EngineCall::CredentialStatus(provider_id.to_string()));
        let configured = state.credentials_configured;
        Ok(NaviProviderCredentialStatus {
            provider_id: provider_id.to_string(),
            configured,
            source: if configured {
                Some("mock".to_string())
            } else {
                None
            },
            label: "Mock".to_string(),
            detail: None,
            env_var: String::new(),
            credential_store_path: PathBuf::from("/tmp/navi-mock-creds"),
        })
    }

    fn set_provider_api_key(&self, provider_id: &str, api_key: &str) -> Result<()> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::SetProviderApiKey {
                provider_id: provider_id.to_string(),
                api_key: api_key.to_string(),
            });
        Ok(())
    }

    fn list_skills(&self) -> Result<Vec<NaviSkillInfo>> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::ListSkills);
        Ok(self.state.lock().unwrap().skills.clone())
    }

    fn delete_saved_session(&self, session_id: &str) -> Result<bool> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(EngineCall::DeleteSavedSession(session_id.to_string()));
        Ok(false)
    }

    fn take_tui_panels(
        &self,
        _session_id: &str,
    ) -> Result<Vec<Box<dyn navi_plugin_api::TuiComponent>>> {
        Ok(Vec::new())
    }

    fn list_mcp_servers(&self, _session_id: &str) -> Result<Vec<McpServerInfo>> {
        Ok(vec![])
    }

    async fn list_background_commands(
        &self,
        _session_id: &str,
    ) -> Result<Vec<navi_sdk::BackgroundCommandSnapshot>> {
        Ok(vec![])
    }

    async fn poll_background_command(
        &self,
        _session_id: &str,
        _task_id: &str,
    ) -> Result<navi_sdk::BackgroundCommandSnapshot> {
        Err(NaviError::Config("mock engine: no background tasks".into()))
    }

    async fn cancel_background_command(
        &self,
        _session_id: &str,
        _task_id: &str,
    ) -> Result<navi_sdk::BackgroundCommandSnapshot> {
        Err(NaviError::Config("mock engine: no background tasks".into()))
    }

    async fn set_permission_mode(&self, _mode: navi_core::PermissionMode) -> Result<()> {
        Ok(())
    }
}
