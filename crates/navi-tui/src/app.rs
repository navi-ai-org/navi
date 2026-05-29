use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use navi_sdk::{
    AgentEvent, AgentMode, AgentRunState, ApprovalRequest, CompactState, CredentialStore,
    HarnessPolicy, LoadedConfig, ModelMessage, ModelOption, NaviEngine, SessionId, SessionSnapshot,
    SessionStore, ToolInvocation, available_model_options, build_system_prompt,
    canonical_provider_id, effective_context_window, log_path, select_harness_policy,
};

use crate::dispatch::AsyncEvent;
use crate::runtime::{build_engine, selected_model_runtime_available};
use crate::session::load_saved_sessions;
use crate::state::{
    ChatMessage, ChatRenderCache, ModalKind, Mode, Notification, SelectionState, ThinkingLevel,
};
use crate::ui::modal::ModalStack;

// ─── app state ─────────────────────────────────────────────────────────────────
pub struct TuiApp {
    pub(crate) loaded_config: LoadedConfig,
    pub(crate) input: String,
    pub(crate) input_cursor: usize,
    pub(crate) mode: Mode,
    pub(crate) modal_stack: ModalStack<ModalKind>,
    pub(crate) command_filter: String,
    pub(crate) selected_command: usize,
    pub(crate) models: Vec<ModelOption>,
    pub(crate) selected_model: usize,
    pub(crate) model_scroll: usize,
    pub(crate) model_filter: String,
    pub(crate) selected_agent: Option<AgentMode>,
    pub(crate) thinking_level: ThinkingLevel,
    pub(crate) selected_thinking: usize,
    tick: u64,

    // chat state
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) scroll_offset: usize,
    pub(crate) is_loading: bool,
    pub(crate) loading_start: Option<Instant>,
    pub(crate) conversation_history: Vec<ModelMessage>,

    // async bridge
    async_tx: mpsc::UnboundedSender<AsyncEvent>,
    async_rx: mpsc::UnboundedReceiver<AsyncEvent>,
    stream_task: Option<JoinHandle<()>>,
    tool_task: Option<JoinHandle<()>>,
    engine: NaviEngine,
    pub(crate) provider_configured: bool,
    harness_policy: HarnessPolicy,
    run_state: AgentRunState,
    pub(crate) yolo_mode: bool,
    pub(crate) skip_next_model_done: bool,
    pub(crate) model_retry_attempts: usize,

    // orchestration
    pub(crate) running_tools: HashMap<String, ToolInvocation>,
    pub(crate) pending_approvals: Vec<ApprovalRequest>,
    pub(crate) tool_invocations: HashMap<String, ToolInvocation>,

    // credentials
    credential_store: CredentialStore,
    pub(crate) api_key_input: String,
    pub(crate) api_key_cursor: usize,
    pub(crate) pending_model_selection: Option<usize>,
    pub(crate) pending_provider_setup: Option<String>,

    // stats
    pub(crate) compact_state: CompactState,

    // persistence
    pub(crate) session_store: SessionStore,
    pub(crate) events: Vec<AgentEvent>,
    pub(crate) session_id: SessionId,
    pub(crate) project_dir: PathBuf,
    pub(crate) saved_sessions: Vec<SessionSnapshot>,
    pub(crate) selected_session: usize,
    pub(crate) session_scroll: usize,

    pub(crate) full_tool_view: bool,
    pub(crate) show_thinking: bool,
    pub(crate) selected_setting: usize,
    pub(crate) selected_provider_setting: usize,
    pub(crate) provider_settings_scroll: usize,
    notification: Option<Notification>,
    diagnostics: Vec<String>,
    log_path: PathBuf,
    pub(crate) chat_render_cache: RefCell<ChatRenderCache>,
    pub(crate) selection: Option<SelectionState>,
}

impl TuiApp {
    pub fn new(loaded_config: LoadedConfig, project_dir: PathBuf, task: Option<String>) -> Self {
        let models = available_model_options(&loaded_config.config);
        let selected_provider = canonical_provider_id(&loaded_config.config.model.provider);
        let selected_model = models
            .iter()
            .position(|model| {
                model.name == loaded_config.config.model.name
                    && canonical_provider_id(&model.provider_id) == selected_provider
            })
            .unwrap_or(0);

        let (async_tx, async_rx) = mpsc::unbounded_channel();
        let credential_store = CredentialStore::new(loaded_config.data_dir.clone());
        let engine = build_engine(&loaded_config, project_dir.clone())
            .expect("failed to initialize NAVI runtime engine");
        let provider_configured =
            selected_model_runtime_available(&loaded_config, &credential_store);
        let session_store = SessionStore::with_redaction(
            loaded_config.data_dir.clone(),
            loaded_config.config.security.redact_secrets_in_sessions,
        );
        let session_id = SessionStore::create_id();
        let saved_sessions = load_saved_sessions(&session_store);
        let harness_policy = select_harness_policy(&loaded_config.config);
        let system_prompt = build_system_prompt(&loaded_config.config, &project_dir);
        let log_path = log_path(&loaded_config.data_dir);
        let context_window = effective_context_window(&loaded_config.config);

        let mut app = Self {
            loaded_config,
            input: String::new(),
            input_cursor: 0,
            mode: Mode::Normal,
            modal_stack: ModalStack::default(),
            command_filter: String::new(),
            selected_command: 0,
            models,
            selected_model,
            model_scroll: 0,
            model_filter: String::new(),
            selected_agent: None,
            thinking_level: ThinkingLevel::High,
            selected_thinking: 1,
            tick: 0,
            messages: Vec::new(),
            scroll_offset: 0,
            is_loading: false,
            loading_start: None,
            conversation_history: vec![ModelMessage::system(system_prompt)],
            async_tx,
            async_rx,
            stream_task: None,
            tool_task: None,
            engine,
            provider_configured,
            harness_policy,
            run_state: AgentRunState::default(),
            yolo_mode: false,
            skip_next_model_done: false,
            model_retry_attempts: 0,
            running_tools: HashMap::new(),
            pending_approvals: Vec::new(),
            tool_invocations: HashMap::new(),
            credential_store,
            api_key_input: String::new(),
            api_key_cursor: 0,
            pending_model_selection: None,
            pending_provider_setup: None,
            compact_state: CompactState::new(context_window),
            session_store,
            events: Vec::new(),
            session_id,
            project_dir,
            saved_sessions,
            selected_session: 0,
            session_scroll: 0,
            full_tool_view: false,
            show_thinking: true,
            selected_setting: 0,
            selected_provider_setting: 0,
            provider_settings_scroll: 0,
            notification: None,
            diagnostics: Vec::new(),
            log_path,
            chat_render_cache: RefCell::new(ChatRenderCache::default()),
            selection: None,
        };

        // If a task was passed via CLI, pre-fill input
        if let Some(task_text) = task {
            app.input = task_text;
        }

        app
    }

    pub(crate) fn tick(&self) -> u64 {
        self.tick
    }

    pub(crate) fn advance_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub(crate) fn log_path(&self) -> &std::path::Path {
        &self.log_path
    }

    pub(crate) fn async_sender(&self) -> mpsc::UnboundedSender<AsyncEvent> {
        self.async_tx.clone()
    }

    pub(crate) fn try_recv_async_event(&mut self) -> Option<AsyncEvent> {
        self.async_rx.try_recv().ok()
    }

    pub(crate) fn engine(&self) -> NaviEngine {
        self.engine.clone()
    }

    pub(crate) fn credential_store(&self) -> &CredentialStore {
        &self.credential_store
    }

    pub(crate) fn credential_store_clone(&self) -> CredentialStore {
        self.credential_store.clone()
    }

    pub(crate) fn set_engine(&mut self, engine: NaviEngine) {
        self.engine = engine;
    }

    pub(crate) fn set_stream_task(&mut self, task: JoinHandle<()>) {
        self.stream_task = Some(task);
    }

    pub(crate) fn clear_stream_task(&mut self) {
        self.stream_task = None;
    }

    pub(crate) fn has_stream_task(&self) -> bool {
        self.stream_task.is_some()
    }

    pub(crate) fn has_tool_task(&self) -> bool {
        self.tool_task.is_some()
    }

    pub(crate) fn has_async_task(&self) -> bool {
        self.has_stream_task() || self.has_tool_task()
    }

    pub(crate) fn abort_async_tasks(&mut self) -> (bool, bool) {
        let had_stream = self.stream_task.is_some();
        let had_tool = self.tool_task.is_some();
        if let Some(task) = self.stream_task.take() {
            task.abort();
        }
        if let Some(task) = self.tool_task.take() {
            task.abort();
        }
        (had_stream, had_tool)
    }

    pub(crate) fn harness_policy(&self) -> HarnessPolicy {
        self.harness_policy
    }

    pub(crate) fn refresh_harness_policy(&mut self) {
        self.harness_policy = select_harness_policy(&self.loaded_config.config);
    }

    pub(crate) fn reset_run_state(&mut self) {
        self.run_state = AgentRunState::default();
    }

    pub(crate) fn set_notification(&mut self, notification: Notification) {
        self.notification = Some(notification);
    }

    pub(crate) fn clear_notification(&mut self) {
        self.notification = None;
    }

    pub(crate) fn notification(&self) -> Option<&Notification> {
        self.notification.as_ref()
    }

    pub(crate) fn push_diagnostic(&mut self, message: impl Into<String>) {
        self.diagnostics.push(message.into());
        if self.diagnostics.len() > 20 {
            let overflow = self.diagnostics.len() - 20;
            self.diagnostics.drain(0..overflow);
        }
    }

    pub(crate) fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }
}
