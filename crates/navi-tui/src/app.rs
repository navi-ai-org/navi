use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use std::sync::Arc;

use navi_sdk::{
    AgentEvent, AgentRunState, ApprovalRequest, BackgroundCommandSnapshot, CompactState,
    CredentialStore, EngineDriver, HarnessPolicy, LoadedConfig, ModelMessage, ModelOption,
    NaviSkillInfo, SessionId, SessionSnapshot, SessionStore, ToolInvocation,
    available_model_options, build_system_prompt, canonical_provider_id, clean_session_title,
    effective_context_window, log_path, provider_catalog, select_harness_policy,
};

use ratatui_image::picker::Picker;

use crate::dispatch::AsyncEvent;
use crate::runtime::{build_engine, selected_model_runtime_available};
use crate::session::load_saved_sessions;
use crate::state::{
    ChatMessage, ChatRenderCache, ChatRole, McpUiState, ModalKind, Mode, Notification,
    OAuthUiState, PluginApprovalRequest, QuestionUiState, SelectionState, ThinkingLevel,
};
use crate::theme::{ThemeId, ThemePalette};
use crate::ui::interaction::{HitAction, HitRegion, InteractionRegistry};
use crate::ui::modal::ModalStack;

// ─── app state ─────────────────────────────────────────────────────────────────
pub struct TuiApp {
    pub(crate) loaded_config: LoadedConfig,
    pub(crate) input: String,
    pub(crate) input_cursor: usize,
    pub(crate) input_selection: Option<(usize, usize)>,
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

    // clipboard images
    /// Images captured from the clipboard, waiting to be attached to the next message.
    pub(crate) pending_images: Vec<crate::state::PendingImage>,
    /// Image protocol picker for terminal rendering.
    pub(crate) image_picker: Option<Picker>,

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
    pub(crate) oauth_state: Option<OAuthUiState>,
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
    pub(crate) cancel_esc_pressed: bool,

    /// Cached set of canonical provider IDs with resolved credentials.
    /// Populated by refresh_authenticated_providers().
    pub(crate) authenticated_providers: HashSet<String>,
    /// Setup wizard phase (None when not in setup mode).
    pub(crate) setup_phase: Option<crate::state::SetupPhase>,

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

    // mcp
    pub(crate) mcp_ui_state: McpUiState,

    // background commands
    pub(crate) background_commands: Vec<BackgroundCommandSnapshot>,
    pub(crate) bg_command_selected: usize,
    pub(crate) bg_command_scroll: usize,
    pub(crate) bg_poll_task: Option<JoinHandle<()>>,

    // session naming
    pub(crate) session_title: Option<String>,
    /// How many background model tasks are actively running (naming, compaction, etc.)
    pub(crate) bg_models_running: usize,

    // background models config
    pub(crate) bg_models_selected: usize,
    pub(crate) bg_models_scroll: usize,
    pub(crate) bg_model_picker_active: bool,
    pub(crate) bg_model_picker_task: Option<String>,
    pub(crate) bg_model_picker_selected: usize,
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
            selected_model_runtime_available(&loaded_config, &credential_store, &project_dir);
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

        #[cfg(not(test))]
        let terminal_picker = terminal_image_picker();

        let mut app = Self {
            loaded_config,
            input: String::new(),
            input_cursor: 0,
            input_selection: None,
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
            pending_images: Vec::new(),
            #[cfg(not(test))]
            image_picker: terminal_picker,
            #[cfg(test)]
            image_picker: None,
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
            oauth_state: None,
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
            cancel_esc_pressed: false,
            authenticated_providers: HashSet::new(),
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
            mcp_ui_state: Default::default(),
            background_commands: Vec::new(),
            bg_command_selected: 0,
            bg_command_scroll: 0,
            bg_poll_task: None,
            session_title: None,
            bg_models_running: 0,
            bg_models_selected: 0,
            bg_models_scroll: 0,
            bg_model_picker_active: false,
            bg_model_picker_task: None,
            bg_model_picker_selected: 0,
            setup_phase: None,
        };

        // If a task was passed via CLI, pre-fill input
        if let Some(task_text) = task {
            app.input = task_text;
        }

        // Load available skills
        app.refresh_skills();

        // Cache authenticated provider IDs for fast model picker filtering
        app.refresh_authenticated_providers();

        // Seed recents with the configured provider/model so they appear under
        // the "Recent" sections on first open, before the user has done any
        // explicit switching.
        {
            let provider = app.loaded_config.config.model.provider.clone();
            let model = app.loaded_config.config.model.name.clone();
            if !provider.is_empty() && !model.is_empty() {
                crate::providers::push_recent_provider(&mut app, &provider);
                crate::providers::push_recent_model(&mut app, &provider, &model);
            }
        }

        Ok(app)
    }
    /// Create a TuiApp in setup wizard mode.
    /// Opens the model picker and transitions to the setup interview when
    /// a provider is configured.
    pub fn setup_mode(loaded_config: LoadedConfig, project_dir: PathBuf) -> Result<Self> {
        let mut app = Self::new(loaded_config, project_dir, None)?;
        app.setup_phase = Some(crate::state::SetupPhase::ProviderLogin);
        app.mode = Mode::Setup;
        // Open model picker immediately so the user can select a provider.
        app.modal_stack.open(ModalKind::Models);
        app.model_filter.clear();
        app.model_scroll = 0;
        app.refresh_authenticated_providers();
        // Pre-populate a welcome message for the setup flow.
        app.messages.push(ChatMessage::new(
            ChatRole::Assistant,
            "Welcome to NAVI! Let's get you set up.\n\n\
            First, choose a provider and enter your API key.\n\
            Press ctrl+m or click the model picker above."
                .to_string(),
        ));
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

    /// Rebuild the cached set of authenticated provider IDs.
    /// Call this when opening the model picker or after credential changes.
    pub(crate) fn refresh_authenticated_providers(&mut self) {
        let engine = self.engine.clone();
        let unique_providers: HashSet<String> = self
            .models
            .iter()
            .map(|m| canonical_provider_id(&m.provider_id).to_string())
            .collect();
        self.authenticated_providers = unique_providers
            .into_iter()
            .filter(|pid| {
                engine
                    .credential_status(pid)
                    .map(|s| s.configured)
                    .unwrap_or(false)
            })
            .collect();
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

    pub(crate) fn filtered_providers(&self) -> Vec<crate::providers::ProviderListRow> {
        use crate::providers::ProviderListRow;

        let filter = self.provider_filter.trim().to_lowercase();
        let providers = provider_catalog(&self.loaded_config.config);
        let total = providers.len();

        let index_of = |id: &str| -> Option<usize> {
            let canonical = navi_sdk::canonical_provider_id(id);
            providers
                .iter()
                .position(|p| navi_sdk::canonical_provider_id(&p.id) == canonical)
        };

        if !filter.is_empty() {
            return providers
                .into_iter()
                .enumerate()
                .filter(|(_, p)| {
                    p.id.to_lowercase().contains(&filter)
                        || p.label.to_lowercase().contains(&filter)
                        || p.description.to_lowercase().contains(&filter)
                })
                .map(|(index, _)| ProviderListRow::Provider { index })
                .collect();
        }

        let mut rows: Vec<ProviderListRow> = Vec::new();
        let mut emitted: Vec<bool> = vec![false; total];
        let push_provider_with_accounts =
            |rows: &mut Vec<ProviderListRow>, provider_index: usize, app: &TuiApp| {
                rows.push(ProviderListRow::Provider {
                    index: provider_index,
                });
                let Some(provider) = providers.get(provider_index) else {
                    return;
                };
                let accounts = app
                    .credential_store
                    .list_credential_accounts(&provider.id, Some(&app.project_dir))
                    .unwrap_or_default();
                for account in accounts {
                    rows.push(ProviderListRow::Account {
                        provider_index,
                        account_id: account.account_id,
                        label: account.label,
                        selected: account.is_project_selected,
                    });
                }
            };

        let recents: Vec<usize> = self
            .loaded_config
            .config
            .tui
            .recent_provider_ids
            .iter()
            .filter_map(|id| index_of(id))
            .filter(|idx| {
                if emitted[*idx] {
                    false
                } else {
                    emitted[*idx] = true;
                    true
                }
            })
            .collect();
        if !recents.is_empty() {
            rows.push(ProviderListRow::Header {
                label: "— Recent —".to_string(),
            });
            for idx in &recents {
                push_provider_with_accounts(&mut rows, *idx, self);
            }
        }

        let connected: Vec<usize> = providers
            .iter()
            .enumerate()
            .filter_map(|(index, p)| {
                if !self.authenticated_providers.contains(p.id.as_str()) {
                    return None;
                }
                if emitted[index] {
                    return None;
                }
                emitted[index] = true;
                Some(index)
            })
            .collect();
        if !connected.is_empty() {
            rows.push(ProviderListRow::Header {
                label: "— Connected —".to_string(),
            });
            for idx in connected {
                push_provider_with_accounts(&mut rows, idx, self);
            }
        }

        let others: Vec<usize> = (0..total).filter(|i| !emitted[*i]).collect();
        if !others.is_empty() {
            rows.push(ProviderListRow::Header {
                label: "— Other providers —".to_string(),
            });
            for idx in others {
                push_provider_with_accounts(&mut rows, idx, self);
            }
        }

        rows
    }
}

#[cfg(not(test))]
fn terminal_image_picker() -> Option<Picker> {
    if let Some(reason) = terminal_image_detection_skip_reason() {
        tracing::debug!(reason, "skipping terminal image support detection");
        return None;
    }

    match Picker::from_query_stdio() {
        Ok(picker) => {
            tracing::info!("terminal image protocol detected");
            Some(picker)
        }
        Err(e) => {
            tracing::debug!(error = %e, "no terminal image protocol detected");
            None
        }
    }
}

#[cfg(not(test))]
fn terminal_image_detection_skip_reason() -> Option<&'static str> {
    terminal_image_detection_skip_reason_from_env(|key| {
        std::env::var_os(key).map(|value| value.to_string_lossy().into_owned())
    })
}

fn terminal_image_detection_skip_reason_from_env(
    mut env: impl FnMut(&str) -> Option<String>,
) -> Option<&'static str> {
    if env("NAVI_SMOKE_TEST").is_some() {
        return Some("NAVI_SMOKE_TEST");
    }
    if env("NAVI_DISABLE_TERMINAL_IMAGES").is_some() {
        return Some("NAVI_DISABLE_TERMINAL_IMAGES");
    }
    if is_termux_environment(&mut env) {
        return Some("termux");
    }
    None
}

fn is_termux_environment(env: &mut impl FnMut(&str) -> Option<String>) -> bool {
    if env("TERMUX_VERSION").is_some() {
        return true;
    }

    ["PREFIX", "HOME", "TMPDIR"]
        .into_iter()
        .filter_map(env)
        .any(|value| value.contains("/com.termux/") || value.contains("/data/data/com.termux"))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn skip_reason(vars: &[(&str, &str)]) -> Option<&'static str> {
        terminal_image_detection_skip_reason_from_env(|key| {
            vars.iter()
                .find(|(candidate, _)| *candidate == key)
                .map(|(_, value)| (*value).to_string())
        })
    }

    #[test]
    fn terminal_image_detection_skips_termux_version() {
        assert_eq!(
            skip_reason(&[("TERMUX_VERSION", "0.118.1")]),
            Some("termux")
        );
    }

    #[test]
    fn terminal_image_detection_skips_termux_paths() {
        assert_eq!(
            skip_reason(&[("PREFIX", "/data/data/com.termux/files/usr")]),
            Some("termux")
        );
        assert_eq!(
            skip_reason(&[("HOME", "/data/data/com.termux/files/home")]),
            Some("termux")
        );
    }

    #[test]
    fn terminal_image_detection_respects_explicit_disable() {
        assert_eq!(
            skip_reason(&[("NAVI_DISABLE_TERMINAL_IMAGES", "1")]),
            Some("NAVI_DISABLE_TERMINAL_IMAGES")
        );
    }

    #[test]
    fn terminal_image_detection_runs_for_normal_terminals() {
        assert_eq!(
            skip_reason(&[
                ("TERM", "xterm-256color"),
                ("HOME", "/home/enrell"),
                ("PREFIX", "/usr")
            ]),
            None
        );
    }
}
