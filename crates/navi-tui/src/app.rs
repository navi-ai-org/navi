use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use std::sync::Arc;

use navi_sdk::{
    AgentEvent, AgentRunState, ApprovalRequest, CompactState, CredentialStore, EngineDriver,
    HarnessPolicy, LoadedConfig, ModelMessage, ModelOption, NaviSkillInfo, ProviderConfig,
    SessionId, SessionSnapshot, SessionStore, ToolInvocation, available_model_options,
    build_system_prompt, canonical_provider_id, clean_session_title, effective_context_window,
    log_path, provider_catalog, select_harness_policy,
};

use crate::dispatch::AsyncEvent;
use crate::runtime::{build_engine, selected_model_runtime_available};
use crate::session::load_saved_sessions;
use crate::state::{
    ChatMessage, ChatRenderCache, ModalKind, Mode, Notification, PluginApprovalRequest,
    QuestionUiState, SelectionState, ThinkingLevel,
};
use crate::theme::{ThemeId, ThemePalette};
use crate::ui::interaction::{HitAction, HitRegion, InteractionRegistry};
use crate::ui::modal::ModalStack;

// ─── app state ─────────────────────────────────────────────────────────────────
pub struct TuiApp {
    pub(crate) loaded_config: LoadedConfig,
    pub(crate) input: String,
    pub(crate) input_cursor: usize,
    pub(crate) input_wrap_width: usize,
    pub(crate) mode: Mode,
    pub(crate) modal_stack: ModalStack<ModalKind>,
    pub(crate) command_filter: String,
    pub(crate) selected_command: usize,
    pub(crate) command_scroll: usize,
    pub(crate) models: Vec<ModelOption>,
    pub(crate) selected_model: usize,
    pub(crate) model_scroll: usize,
    pub(crate) model_filter: String,
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
    engine: Arc<dyn EngineDriver>,
    pub(crate) provider_configured: bool,
    harness_policy: HarnessPolicy,
    run_state: AgentRunState,
    pub(crate) yolo_mode: bool,
    pub(crate) skip_next_model_done: bool,
    pub(crate) model_retry_attempts: usize,

    // orchestration
    pub(crate) running_tools: HashMap<String, ToolInvocation>,
    pub(crate) pending_approvals: Vec<ApprovalRequest>,
    pub(crate) pending_questions: Vec<QuestionUiState>,
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
    pub(crate) git_branch: Option<String>,
    pub(crate) saved_sessions: Vec<SessionSnapshot>,
    pub(crate) selected_session: usize,
    pub(crate) session_scroll: usize,
    pub(crate) session_filter: String,

    pub(crate) full_tool_view: bool,
    pub(crate) compact_tool_visible_limit: usize,
    pub(crate) show_thinking: bool,
    pub(crate) selected_setting: usize,
    pub(crate) selected_theme: usize,
    pub(crate) theme_filter: String,
    pub(crate) selected_provider_setting: usize,
    pub(crate) provider_settings_scroll: usize,
    pub(crate) provider_filter: String,
    notification: Option<Notification>,
    diagnostics: Vec<String>,
    log_path: PathBuf,
    pub(crate) chat_render_cache: RefCell<ChatRenderCache>,
    pub(crate) interaction_registry: RefCell<InteractionRegistry>,
    pub(crate) selection: Option<SelectionState>,
    pub(crate) hover_index: Option<usize>,
    pub(crate) theme_id: ThemeId,
    pub(crate) message_action_target: Option<usize>,
    pub(crate) selected_message_action: usize,
    pub(crate) expanded_tool_results: HashSet<String>,
    pub(crate) hovered_chat_source: Option<crate::state::ChatLineSource>,

    // skills
    pub(crate) available_skills: Vec<NaviSkillInfo>,
    pub(crate) active_skills: Vec<String>,
    pub(crate) selected_skill: usize,
    pub(crate) skill_filter: String,
    pub(crate) skill_scroll: usize,

    // plugins modal (marketplace catalog + installed)
    pub(crate) plugin_catalog: Vec<navi_plugin_manifest::PluginCatalogEntry>,
    pub(crate) plugin_catalog_loading: bool,
    pub(crate) plugin_catalog_error: String,
    pub(crate) selected_plugin_row: usize,
    pub(crate) plugin_row_scroll: usize,

    // plugin install / update approvals
    pub(crate) pending_plugin_approvals: Vec<PluginApprovalRequest>,
    pub(crate) plugin_approval_scroll: usize,
}

impl TuiApp {
    pub fn new(
        loaded_config: LoadedConfig,
        project_dir: PathBuf,
        task: Option<String>,
    ) -> Result<Self> {
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
        let engine: Arc<dyn EngineDriver> =
            Arc::new(build_engine(&loaded_config, project_dir.clone())?);
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
        let initial_active_skills = loaded_config.config.skills.active.clone();
        let show_thinking = loaded_config.config.tui.show_thinking;
        let full_tool_view = loaded_config.config.tui.full_tool_view;
        let compact_tool_visible_limit = loaded_config
            .config
            .tui
            .compact_tool_visible_limit
            .clamp(1, 20);
        let yolo_mode = loaded_config.config.tui.yolo_mode;
        let theme_id = ThemeId::from_config(&loaded_config.config.tui.theme);
        let thinking_level = ThinkingLevel::from_config(&loaded_config.config.tui.thinking_level);
        let git_branch = detect_git_branch(&project_dir);

        let mut app = Self {
            loaded_config,
            input: String::new(),
            input_cursor: 0,
            input_wrap_width: 80,
            mode: Mode::Normal,
            modal_stack: ModalStack::default(),
            command_filter: String::new(),
            selected_command: 0,
            command_scroll: 0,
            models,
            selected_model,
            model_scroll: 0,
            model_filter: String::new(),
            thinking_level,
            selected_thinking: thinking_level.index(),
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
            yolo_mode,
            skip_next_model_done: false,
            model_retry_attempts: 0,
            running_tools: HashMap::new(),
            pending_approvals: Vec::new(),
            pending_questions: Vec::new(),
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
            git_branch,
            saved_sessions,
            selected_session: 0,
            session_scroll: 0,
            session_filter: String::new(),
            full_tool_view,
            compact_tool_visible_limit,
            show_thinking,
            selected_setting: 0,
            selected_theme: ThemeId::ALL
                .iter()
                .position(|id| *id == theme_id)
                .unwrap_or(0),
            theme_filter: String::new(),
            selected_provider_setting: 0,
            provider_settings_scroll: 0,
            provider_filter: String::new(),
            notification: None,
            diagnostics: Vec::new(),
            log_path,
            chat_render_cache: RefCell::new(ChatRenderCache::default()),
            interaction_registry: RefCell::new(InteractionRegistry::default()),
            selection: None,
            hover_index: None,
            theme_id,
            message_action_target: None,
            selected_message_action: 0,
            expanded_tool_results: HashSet::new(),
            hovered_chat_source: None,
            available_skills: Vec::new(),
            active_skills: initial_active_skills,
            selected_skill: 0,
            skill_filter: String::new(),
            skill_scroll: 0,
            plugin_catalog: Vec::new(),
            plugin_catalog_loading: false,
            plugin_catalog_error: String::new(),
            selected_plugin_row: 0,
            plugin_row_scroll: 0,
            pending_plugin_approvals: Vec::new(),
            plugin_approval_scroll: 0,
        };

        // If a task was passed via CLI, pre-fill input
        if let Some(task_text) = task {
            app.input = task_text;
        }

        // Load available skills
        app.refresh_skills();

        Ok(app)
    }

    pub(crate) fn advance_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub(crate) fn tick(&self) -> u64 {
        self.tick
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

    pub(crate) fn engine(&self) -> Arc<dyn EngineDriver> {
        self.engine.clone()
    }

    pub(crate) fn theme_palette(&self) -> ThemePalette {
        self.theme_id.palette()
    }

    pub(crate) fn clear_interactions(&self) {
        self.interaction_registry.borrow_mut().clear();
    }

    pub(crate) fn register_hit(
        &self,
        rect: ratatui::layout::Rect,
        z: i16,
        label: impl Into<String>,
        action: HitAction,
    ) -> String {
        self.interaction_registry
            .borrow_mut()
            .register(rect, z, label, action)
    }

    pub(crate) fn hit_test(&self, col: u16, row: u16) -> Option<HitRegion> {
        self.interaction_registry.borrow().hit(col, row)
    }

    pub(crate) fn set_theme(&mut self, theme_id: ThemeId) {
        self.theme_id = theme_id;
        self.chat_render_cache.borrow_mut().signature_hash = 0;
        crate::persistence::save_preferences(self);
    }

    pub(crate) fn credential_store(&self) -> &CredentialStore {
        &self.credential_store
    }

    pub(crate) fn credential_store_clone(&self) -> CredentialStore {
        self.credential_store.clone()
    }

    pub(crate) fn set_engine(&mut self, engine: Arc<dyn EngineDriver>) {
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

    pub(crate) fn refresh_skills(&mut self) {
        self.available_skills = self.engine.list_skills().unwrap_or_default();
    }

    pub(crate) fn toggle_skill(&mut self, skill_id: &str) {
        if let Some(pos) = self.active_skills.iter().position(|s| s == skill_id) {
            self.active_skills.remove(pos);
        } else {
            self.active_skills.push(skill_id.to_string());
        }
        crate::persistence::save_preferences(self);
    }

    pub(crate) fn is_skill_active(&self, skill_id: &str) -> bool {
        self.active_skills.iter().any(|s| s == skill_id)
    }

    pub(crate) fn filtered_skills(&self) -> Vec<&NaviSkillInfo> {
        let filter = self.skill_filter.trim().to_lowercase();
        self.available_skills
            .iter()
            .filter(|skill| {
                filter.is_empty()
                    || skill.name.to_lowercase().contains(&filter)
                    || skill.id.to_lowercase().contains(&filter)
                    || skill
                        .description
                        .as_ref()
                        .map(|d| d.to_lowercase().contains(&filter))
                        .unwrap_or(false)
            })
            .collect()
    }

    pub(crate) fn filtered_sessions(&self) -> Vec<&SessionSnapshot> {
        let filter = self.session_filter.trim().to_lowercase();
        self.saved_sessions
            .iter()
            .filter(|snapshot| {
                if filter.is_empty() {
                    return true;
                }
                let project = snapshot
                    .project
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| snapshot.project.to_string_lossy().to_string())
                    .to_lowercase();
                let title = snapshot
                    .title
                    .as_deref()
                    .and_then(clean_session_title)
                    .unwrap_or_else(|| project.clone())
                    .to_lowercase();
                title.contains(&filter) || project.contains(&filter)
            })
            .collect()
    }

    pub(crate) fn filtered_providers(&self) -> Vec<ProviderConfig> {
        let filter = self.provider_filter.trim().to_lowercase();
        let providers = provider_catalog(&self.loaded_config.config);
        if filter.is_empty() {
            return providers;
        }
        providers
            .into_iter()
            .filter(|p| {
                p.id.to_lowercase().contains(&filter)
                    || p.label.to_lowercase().contains(&filter)
                    || p.description.to_lowercase().contains(&filter)
            })
            .collect()
    }
}

fn detect_git_branch(project_dir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(project_dir.join(".git").join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(branch) = head.strip_prefix("ref: refs/heads/") {
        return Some(branch.to_string());
    }
    if head.len() >= 7 {
        return Some(head.chars().take(7).collect());
    }
    None
}
