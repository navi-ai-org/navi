use anyhow::Result;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use navi_core::{
    AgentEvent, AgentRunState, ApprovalDecision, CredentialStore, HarnessPolicy, LoadedConfig,
    ModelMessage, ModelOption, ModelProvider, ModelRole, ProviderConfig, SecurityPolicy, SessionId,
    SessionSnapshot, SessionStore, ThinkingConfig, ToolExecutor, ToolInvocation, ToolResult,
    available_model_options, build_system_prompt, canonical_provider_id, compact_tool_observation,
    log_path, resolve_provider_config, save_global_config, select_harness_policy,
};
use navi_openai::OpenAiProvider;
use navi_plugin_host::{LoadedPlugin, load_configured_plugins};
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{CrosstermBackend, Frame, Line, Span, Terminal};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};
use std::cell::RefCell;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SyntectStyle, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

// ─── palette ───────────────────────────────────────────────────────────────────
const ACCENT: Color = Color::Rgb(176, 34, 255);
const RED: Color = Color::Rgb(218, 64, 255);
const PINK: Color = Color::Rgb(194, 31, 255);
const SIGNAL: Color = Color::Rgb(236, 218, 255);
const TEXT: Color = Color::Rgb(245, 239, 255);
const MUTED: Color = Color::Rgb(150, 128, 166);
const PANEL: Color = Color::Rgb(19, 13, 26);
const BG: Color = Color::Rgb(9, 5, 13);
const GHOST: Color = Color::Rgb(58, 38, 74);
const USER_ACCENT: Color = Color::Rgb(176, 34, 255);
const CODE_KEYWORD: Color = Color::Rgb(220, 96, 255);
const CODE_STRING: Color = Color::Rgb(205, 166, 255);
const CODE_COMMENT: Color = Color::Rgb(124, 100, 146);
const CODE_NUMBER: Color = Color::Rgb(160, 220, 255);
const CODE_PUNCT: Color = Color::Rgb(185, 145, 220);
const CODE_TYPE: Color = Color::Rgb(111, 214, 255);
const CODE_FUNC: Color = Color::Rgb(190, 146, 255);
const CODE_CONST: Color = Color::Rgb(255, 199, 112);
const CODE_OPERATOR: Color = Color::Rgb(255, 118, 214);
const NOTIFICATION_TTL: Duration = Duration::from_secs(2);

const NAVI_COMPACT_LOGO: &[&str] = &[
    r"███╗   ██╗ █████╗ ██╗   ██╗██╗",
    r"████╗  ██║██╔══██╗██║   ██║██║",
    r"██╔██╗ ██║███████║██║   ██║██║",
    r"██║╚██╗██║██╔══██║╚██╗ ██╔╝██║",
    r"██║ ╚████║██║  ██║ ╚████╔╝ ██║",
    r"╚═╝  ╚═══╝╚═╝  ╚═╝  ╚═══╝  ╚═╝",
];

// ─── chat message type ─────────────────────────────────────────────────────────
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    pub model_label: Option<String>,
    pub provider_label: Option<String>,
    pub elapsed_ms: Option<u64>,
    pub status: Option<String>,
    pub usage_label: Option<String>,
    pub thinking_content: String,
    pub tool_invocation: Option<ToolInvocation>,
    pub tool_result: Option<ToolResult>,
    pub is_compact_summary: bool,
}

impl ChatMessage {
    pub fn new(role: ChatRole, content: String) -> Self {
        Self {
            role,
            content,
            model_label: None,
            provider_label: None,
            elapsed_ms: None,
            status: None,
            usage_label: None,
            thinking_content: String::new(),
            tool_invocation: None,
            tool_result: None,
            is_compact_summary: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

// ─── async bridge ──────────────────────────────────────────────────────────────
enum AsyncEvent {
    ModelError {
        message: String,
    },
    SyncCompleted {
        loaded_config: LoadedConfig,
        message: String,
    },
    OAuthDeviceStarted {
        provider_id: String,
        verification_uri: String,
        user_code: String,
    },
    OAuthCompleted {
        provider_id: String,
        result: Result<(), String>,
    },
    Agent(navi_core::AgentEvent),
    TurnCompleted(Result<String, String>),
    RetryModel,
}

// ─── app state ─────────────────────────────────────────────────────────────────
pub struct TuiApp {
    loaded_config: LoadedConfig,
    input: String,
    input_cursor: usize,
    mode: Mode,
    command_filter: String,
    selected_command: usize,
    models: Vec<ModelOption>,
    selected_model: usize,
    model_scroll: usize,
    model_filter: String,
    thinking_level: ThinkingLevel,
    selected_thinking: usize,
    tick: u64,

    // chat state
    messages: Vec<ChatMessage>,
    scroll_offset: usize,
    is_loading: bool,
    loading_start: Option<Instant>,
    conversation_history: Vec<ModelMessage>,

    // async bridge
    async_tx: mpsc::UnboundedSender<AsyncEvent>,
    async_rx: mpsc::UnboundedReceiver<AsyncEvent>,
    stream_task: Option<JoinHandle<()>>,
    tool_task: Option<JoinHandle<()>>,
    tool_executor: Arc<ToolExecutor>,
    _loaded_plugins: Vec<LoadedPlugin>,
    harness_policy: HarnessPolicy,
    run_state: AgentRunState,
    yolo_mode: bool,
    skip_next_model_done: bool,
    model_retry_attempts: usize,

    // orchestration
    session_runtime: Option<navi_core::session::SessionRuntime>,
    turn_context: Option<Arc<navi_core::turn::TurnContext>>,
    running_tools: std::collections::HashMap<String, ToolInvocation>,
    pending_approvals: Vec<navi_core::ApprovalRequest>,
    tool_invocations: std::collections::HashMap<String, ToolInvocation>,

    // provider
    model_provider: Option<Arc<dyn ModelProvider>>,

    // credentials
    credential_store: CredentialStore,
    api_key_input: String,
    api_key_cursor: usize,
    pending_model_selection: Option<usize>,
    pending_provider_setup: Option<String>,

    // stats
    total_tokens_estimate: usize,
    compact_state: navi_core::compact::CompactState,

    // persistence
    session_store: SessionStore,
    events: Vec<AgentEvent>,
    session_id: SessionId,
    project_dir: PathBuf,
    saved_sessions: Vec<SessionSnapshot>,
    selected_session: usize,
    session_scroll: usize,

    full_tool_view: bool,
    show_thinking: bool,
    selected_setting: usize,
    selected_provider_setting: usize,
    provider_settings_scroll: usize,
    notification: Option<Notification>,
    diagnostics: Vec<String>,
    log_path: PathBuf,
    chat_render_cache: RefCell<ChatRenderCache>,
}

#[derive(Debug, Clone)]
struct Notification {
    title: String,
    message: String,
    created_at: Instant,
    ttl: Duration,
}

#[derive(Default)]
struct ChatRenderCache {
    width: usize,
    full_tool_view: bool,
    show_thinking: bool,
    signature: String,
    lines: Vec<Line<'static>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Commands,
    Models,
    ApiKeyEntry,
    Thinking,
    Sessions,
    Settings,
    Providers,
    Debug,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThinkingLevel {
    Max,
    High,
    Medium,
    Low,
    Off,
}

impl From<ThinkingLevel> for ThinkingConfig {
    fn from(value: ThinkingLevel) -> Self {
        match value {
            ThinkingLevel::Max => Self::Max,
            ThinkingLevel::High => Self::High,
            ThinkingLevel::Medium => Self::Medium,
            ThinkingLevel::Low => Self::Low,
            ThinkingLevel::Off => Self::Off,
        }
    }
}

impl ThinkingLevel {
    fn label(self) -> &'static str {
        match self {
            Self::Max => "max",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Off => "off",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct CommandItem {
    label: &'static str,
    shortcut: Option<&'static str>,
    action: CommandAction,
}

#[derive(Debug, Clone, Copy)]
enum CommandAction {
    NewSession,
    Sessions,
    SwitchModel,
    RetryLast,
    OpenThinking,
    Compact,
    InitializeProject,
    SyncModels,
    Quit,
    Settings,
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
        let model_provider = build_provider(&loaded_config, &credential_store);
        let session_store = SessionStore::with_redaction(
            loaded_config.data_dir.clone(),
            loaded_config.config.security.redact_secrets_in_sessions,
        );
        let session_id = SessionStore::create_id();
        let saved_sessions = load_saved_sessions(&session_store);
        let tool_policy = SecurityPolicy::new(
            project_dir.clone(),
            loaded_config.data_dir.clone(),
            loaded_config.config.security.clone(),
        )
        .expect("failed to initialize security policy");
        let mut tool_executor = ToolExecutor::new(tool_policy.clone());
        let plugin_report = load_configured_plugins(
            &loaded_config.config.plugins,
            &tool_policy,
            &mut tool_executor,
        );
        let plugin_warning = plugin_report.warnings.first().cloned();
        let loaded_plugins = plugin_report.loaded_plugins;
        let tool_executor = Arc::new(tool_executor);
        let harness_policy = select_harness_policy(&loaded_config.config);
        let system_prompt = build_system_prompt(&loaded_config.config, &project_dir);
        let log_path = log_path(&loaded_config.data_dir);
        let context_window = navi_core::config::effective_context_window(&loaded_config.config);

        let mut app = Self {
            loaded_config,
            input: String::new(),
            input_cursor: 0,
            mode: Mode::Normal,
            command_filter: String::new(),
            selected_command: 0,
            models,
            selected_model,
            model_scroll: 0,
            model_filter: String::new(),
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
            tool_executor,
            _loaded_plugins: loaded_plugins,
            harness_policy,
            run_state: AgentRunState::default(),
            yolo_mode: false,
            skip_next_model_done: false,
            model_retry_attempts: 0,
            session_runtime: None,
            turn_context: None,
            running_tools: std::collections::HashMap::new(),
            pending_approvals: Vec::new(),
            tool_invocations: std::collections::HashMap::new(),
            model_provider,
            credential_store,
            api_key_input: String::new(),
            api_key_cursor: 0,
            pending_model_selection: None,
            pending_provider_setup: None,
            total_tokens_estimate: 0,
            compact_state: navi_core::compact::CompactState::new(context_window),
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
        };

        if let Some(warning) = plugin_warning {
            push_diagnostic(&mut app, format!("Plugin warning: {warning}"));
            show_notification(&mut app, "Plugins", warning);
        }

        // If a task was passed via CLI, pre-fill input
        if let Some(task_text) = task {
            app.input = task_text;
        }

        app
    }
}

// ─── commands ──────────────────────────────────────────────────────────────────
const COMMANDS: &[CommandItem] = &[
    CommandItem {
        label: "New Layer",
        shortcut: Some("ctrl+n"),
        action: CommandAction::NewSession,
    },
    CommandItem {
        label: "Memory",
        shortcut: Some("ctrl+s"),
        action: CommandAction::Sessions,
    },
    CommandItem {
        label: "Switch Protocol",
        shortcut: Some("ctrl+m"),
        action: CommandAction::SwitchModel,
    },
    CommandItem {
        label: "Retry Last Response",
        shortcut: None,
        action: CommandAction::RetryLast,
    },
    CommandItem {
        label: "Thinking Mode",
        shortcut: None,
        action: CommandAction::OpenThinking,
    },
    CommandItem {
        label: "Compact Context",
        shortcut: None,
        action: CommandAction::Compact,
    },
    CommandItem {
        label: "Initialize Layer",
        shortcut: None,
        action: CommandAction::InitializeProject,
    },
    CommandItem {
        label: "Sync Models",
        shortcut: None,
        action: CommandAction::SyncModels,
    },
    CommandItem {
        label: "Settings",
        shortcut: None,
        action: CommandAction::Settings,
    },
    CommandItem {
        label: "Quit",
        shortcut: Some("ctrl+c"),
        action: CommandAction::Quit,
    },
];

// ─── entry point (sync — no nested runtime) ────────────────────────────────────
// The caller (navi-cli `#[tokio::main]`) already owns a multi-thread tokio
// runtime, so `tokio::spawn` works from inside this synchronous event loop.
// We must NOT create a second runtime here.
pub fn run(app: TuiApp) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    // Enable the kitty keyboard protocol so the terminal can distinguish
    // Ctrl+Enter from plain Enter (and report other modifier combos).
    let enhanced_keyboard = supports_keyboard_enhancement().unwrap_or(false);
    if enhanced_keyboard {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, app);

    // Restore keyboard mode before leaving.
    if enhanced_keyboard {
        execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags)?;
    }
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    disable_raw_mode()?;
    terminal.show_cursor()?;

    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, mut app: TuiApp) -> Result<()> {
    // If input was pre-filled from CLI task, submit on first frame
    if !app.input.trim().is_empty() && app.mode == Mode::Normal {
        submit_message(&mut app);
    }

    let mut needs_draw = true;
    loop {
        if needs_draw {
            terminal.draw(|frame| render(frame, &app))?;
            app.tick = app.tick.wrapping_add(1);
            needs_draw = false;
        }

        if expire_notification(&mut app) {
            needs_draw = true;
        }

        // Check for async model stream events (non-blocking)
        while let Ok(event) = app.async_rx.try_recv() {
            needs_draw = true;
            match event {
                AsyncEvent::Agent(agent_event) => {
                    match agent_event {
                        navi_core::AgentEvent::ModelDelta { text } => {
                            if let Some(message) = active_assistant_message(&mut app) {
                                message.content.push_str(&text);
                                message.status = Some("receiving".to_string());
                            }
                            app.scroll_offset = 0;
                        }
                        navi_core::AgentEvent::ModelThinkingDelta { text } => {
                            if let Some(message) = active_assistant_message(&mut app) {
                                message.thinking_content.push_str(&text);
                                message.status = Some("thinking".to_string());
                            }
                            app.scroll_offset = 0;
                        }
                        navi_core::AgentEvent::ToolRequested(invocation) => {
                            app.tool_invocations
                                .insert(invocation.id.clone(), invocation.clone());
                            app.running_tools
                                .insert(invocation.id.clone(), invocation.clone());

                            // finalize assistant response if any
                            let active_content = active_assistant_message(&mut app)
                                .map(|active| active.content.clone())
                                .unwrap_or_default();
                            if !active_content.trim().is_empty() {
                                app.conversation_history
                                    .push(ModelMessage::assistant(active_content));
                            }
                            app.conversation_history
                                .push(ModelMessage::assistant_tool_call(invocation.clone()));
                            app.events
                                .push(navi_core::AgentEvent::ToolRequested(invocation));
                            update_active_assistant_status(&mut app);
                        }
                        navi_core::AgentEvent::ToolCompleted(result) => {
                            app.running_tools.remove(&result.invocation_id);
                            if let Some(invocation) =
                                app.tool_invocations.get(&result.invocation_id).cloned()
                            {
                                remove_active_tool_placeholder(&mut app);
                                app.messages.push(ChatMessage {
                                    status: Some("tool result".to_string()),
                                    tool_invocation: Some(invocation.clone()),
                                    tool_result: Some(result.clone()),
                                    ..ChatMessage::new(ChatRole::Assistant, String::new())
                                });
                                let observation = compact_tool_observation(
                                    &invocation,
                                    &result,
                                    app.harness_policy,
                                );
                                app.conversation_history.push(ModelMessage::tool_result(
                                    invocation.id.clone(),
                                    invocation.tool_name.clone(),
                                    observation,
                                ));
                            }
                            app.events
                                .push(navi_core::AgentEvent::ToolCompleted(result));
                            update_active_assistant_status(&mut app);
                        }
                        navi_core::AgentEvent::ApprovalRequested(request) => {
                            if app.yolo_mode {
                                if let Some(ctx) = &app.turn_context {
                                    let decision = navi_core::ApprovalDecision::Approved {
                                        id: request.id.clone(),
                                    };
                                    ctx.resolve_approval(decision);
                                }
                            } else {
                                app.pending_approvals.push(request.clone());
                                app.events
                                    .push(navi_core::AgentEvent::ApprovalRequested(request));
                                update_active_assistant_status(&mut app);
                            }
                        }
                        navi_core::AgentEvent::ApprovalResolved(decision) => {
                            let id = match &decision {
                                navi_core::ApprovalDecision::Approved { id } => id,
                                navi_core::ApprovalDecision::Denied { id } => id,
                            };
                            app.pending_approvals.retain(|r| &r.id != id);
                            app.events
                                .push(navi_core::AgentEvent::ApprovalResolved(decision));
                            update_active_assistant_status(&mut app);
                        }
                        navi_core::AgentEvent::Error { message } => {
                            handle_model_error(&mut app, message);
                        }
                        navi_core::AgentEvent::HarnessTrace(value) => {
                            app.events.push(navi_core::AgentEvent::HarnessTrace(value));
                        }
                        navi_core::AgentEvent::PatchProposed(patch) => {
                            app.events.push(navi_core::AgentEvent::PatchProposed(patch));
                        }
                        navi_core::AgentEvent::UsageReported {
                            input_tokens,
                            output_tokens,
                        } => {
                            app.compact_state.update_usage(input_tokens);
                            if let Some(msg) = app.messages.last_mut() {
                                if msg.role == ChatRole::Assistant && msg.usage_label.is_none() {
                                    msg.usage_label = Some(format!(
                                        "{}k in · {}k out",
                                        input_tokens / 1000,
                                        output_tokens / 1000,
                                    ));
                                }
                            }
                            app.events.push(navi_core::AgentEvent::UsageReported {
                                input_tokens,
                                output_tokens,
                            });
                        }
                        navi_core::AgentEvent::MicroCompactApplied { messages_cleared } => {
                            show_notification(
                                &mut app,
                                "Micro-Compact",
                                format!(
                                    "{} old tool results cleared (60+ min gap)",
                                    messages_cleared
                                ),
                            );
                            app.events.push(navi_core::AgentEvent::MicroCompactApplied {
                                messages_cleared,
                            });
                        }
                        navi_core::AgentEvent::AutoCompactStarted => {
                            push_diagnostic(
                                &mut app,
                                "Auto-compact: context threshold reached, summarizing..."
                                    .to_string(),
                            );
                            app.events.push(navi_core::AgentEvent::AutoCompactStarted);
                        }
                        navi_core::AgentEvent::AutoCompactCompleted { tokens_saved } => {
                            show_notification(
                                &mut app,
                                "Auto-Compact",
                                format!(
                                    "Context compacted ({}k tokens saved)",
                                    tokens_saved / 1000
                                ),
                            );
                            app.compact_state.consecutive_failures = 0;
                            if let Some(summary) = &app.compact_state.summary {
                                app.messages.push(ChatMessage {
                                    status: Some("compacted".to_string()),
                                    is_compact_summary: true,
                                    content: format!(
                                        "[Context compacted — {}k tokens saved]\n\n{}",
                                        tokens_saved / 1000,
                                        summary,
                                    ),
                                    ..ChatMessage::new(ChatRole::Assistant, String::new())
                                });
                            }
                            app.events
                                .push(navi_core::AgentEvent::AutoCompactCompleted { tokens_saved });
                        }
                        navi_core::AgentEvent::AutoCompactFailed { reason } => {
                            push_diagnostic(&mut app, format!("Auto-compact failed: {reason}"));
                            app.compact_state.consecutive_failures =
                                app.compact_state.consecutive_failures.saturating_add(1);
                            app.events
                                .push(navi_core::AgentEvent::AutoCompactFailed { reason });
                        }
                        navi_core::AgentEvent::UserTaskSubmitted { text: _ } => {}
                        navi_core::AgentEvent::ModelOutput {
                            text: _,
                            thinking: _,
                        } => {}
                    }
                }
                AsyncEvent::TurnCompleted(res) => {
                    let elapsed_ms = app
                        .loading_start
                        .map(|start| start.elapsed().as_millis() as u64)
                        .unwrap_or(0);
                    match res {
                        Ok(_) => {
                            finalize_active_assistant(&mut app, elapsed_ms);
                            app.is_loading = false;
                            app.loading_start = None;
                            app.stream_task = None;
                            app.scroll_offset = 0;
                            app.running_tools.clear();
                            app.pending_approvals.clear();
                        }
                        Err(err) => {
                            app.is_loading = false;
                            app.loading_start = None;
                            app.stream_task = None;
                            app.scroll_offset = 0;
                            app.running_tools.clear();
                            app.pending_approvals.clear();
                            handle_model_error(&mut app, err);
                        }
                    }
                }

                AsyncEvent::ModelError { message } => {
                    handle_model_error(&mut app, message);
                }
                AsyncEvent::RetryModel => {
                    app.stream_task = None;
                    if app.is_loading {
                        start_streaming_request(&mut app);
                    }
                }
                AsyncEvent::SyncCompleted {
                    loaded_config,
                    message,
                } => {
                    app.loaded_config = loaded_config;
                    app.models = available_model_options(&app.loaded_config.config);
                    let selected_name = app.loaded_config.config.model.name.clone();
                    let selected_provider =
                        canonical_provider_id(&app.loaded_config.config.model.provider);
                    app.selected_model = app
                        .models
                        .iter()
                        .position(|model| {
                            model.name == selected_name
                                && canonical_provider_id(&model.provider_id) == selected_provider
                        })
                        .unwrap_or(0);
                    rebuild_provider(&mut app);
                    app.messages.push(ChatMessage {
                        status: Some("synced".to_string()),
                        ..ChatMessage::new(ChatRole::Assistant, message)
                    });
                    app.is_loading = false;
                    app.loading_start = None;
                    app.stream_task = None;
                    app.scroll_offset = 0;
                }
                AsyncEvent::OAuthDeviceStarted {
                    provider_id,
                    verification_uri,
                    user_code,
                } => {
                    show_notification(
                        &mut app,
                        "OAuth",
                        format!("{provider_id}: open {verification_uri} and enter {user_code}"),
                    );
                    app.messages.push(ChatMessage {
                        status: Some("oauth".to_string()),
                        ..ChatMessage::new(
                            ChatRole::Assistant,
                            format!(
                                "OAuth started for {provider_id}.\nOpen {verification_uri}\nEnter code: {user_code}"
                            ),
                        )
                    });
                }
                AsyncEvent::OAuthCompleted {
                    provider_id,
                    result,
                } => {
                    app.is_loading = false;
                    app.loading_start = None;
                    app.stream_task = None;
                    match result {
                        Ok(()) => {
                            rebuild_provider(&mut app);
                            show_notification(
                                &mut app,
                                "OAuth",
                                format!("{provider_id} connected."),
                            );
                        }
                        Err(err) => {
                            show_notification(
                                &mut app,
                                "OAuth",
                                format!("{provider_id} failed: {err}"),
                            );
                        }
                    }
                }
            }
        }

        let timeout = if app.is_loading {
            Duration::from_millis(16)
        } else if app.messages.is_empty() || visible_notification(&app).is_some() {
            Duration::from_millis(80)
        } else {
            Duration::from_millis(250)
        };

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                needs_draw = true;
                if handle_key(&mut app, key.code, key.modifiers) {
                    break;
                }
            }
        } else if app.is_loading || app.messages.is_empty() || visible_notification(&app).is_some()
        {
            needs_draw = true;
        }
    }

    save_current_session(&mut app);
    save_preferences(&mut app);

    Ok(())
}

fn submit_message(app: &mut TuiApp) {
    let text = app.input.trim().to_string();
    if text.is_empty() {
        return;
    }
    tracing::info!(
        model = %app.loaded_config.config.model.name,
        provider = %app.loaded_config.config.model.provider,
        chars = text.len(),
        "TUI prompt submitted"
    );

    let word_count = text.split_whitespace().count();
    app.total_tokens_estimate += word_count * 4 / 3;

    app.messages
        .push(ChatMessage::new(ChatRole::User, text.clone()));

    app.conversation_history
        .push(ModelMessage::user(text.clone()));

    app.events
        .push(AgentEvent::UserTaskSubmitted { text: text.clone() });

    app.input.clear();
    app.input_cursor = 0;
    app.scroll_offset = 0;
    app.run_state = AgentRunState::default();
    app.model_retry_attempts = 0;

    start_streaming_request(app);
}

fn start_streaming_request(app: &mut TuiApp) {
    let Some(provider) = app.model_provider.clone() else {
        tracing::warn!(provider = %app.loaded_config.config.model.provider, "cannot start stream without API key");
        push_diagnostic(app, "No API key configured for selected provider.");
        app.messages.push(ChatMessage {
            status: Some("missing key".to_string()),
            ..ChatMessage::new(
                ChatRole::Assistant,
                "⚠ No API key configured. Press ctrl+m, choose a protocol, then enter its key."
                    .to_string(),
            )
        });
        return;
    };

    app.is_loading = true;
    app.loading_start = Some(Instant::now());
    tracing::info!(
        provider = %app.loaded_config.config.model.provider,
        model = %app.loaded_config.config.model.name,
        history = app.conversation_history.len(),
        "TUI model stream started"
    );

    let model_label = app.loaded_config.config.model.name.clone();
    let request_model_name = provider_request_model_name(
        &app.loaded_config.config.model.provider,
        &app.loaded_config.config.model.name,
    );
    let provider_label = selected_provider_label(app).to_string();
    app.messages.push(ChatMessage {
        model_label: Some(model_label.clone()),
        provider_label: Some(provider_label),
        status: Some("thinking".to_string()),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });

    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let pending_approvals = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    let ctx = Arc::new(navi_core::turn::TurnContext {
        model_provider: provider,
        tool_executor: app.tool_executor.clone(),
        agent_control: navi_core::agent::AgentControl::new(),
        project_dir: app.project_dir.clone(),
        model_name: request_model_name,
        event_tx: Some(event_tx),
        pending_approvals: pending_approvals.clone(),
        compact_state: Arc::new(tokio::sync::Mutex::new(app.compact_state.clone())),
        harness_config: app.loaded_config.config.harness.clone(),
    });

    app.turn_context = Some(ctx.clone());

    let mut initial_messages = app.conversation_history.clone();
    let user_prompt = if !initial_messages.is_empty() {
        let last = initial_messages.pop().unwrap();
        last.content
    } else {
        String::new()
    };

    let memory_injection = if app.loaded_config.config.memory.session_memory_enabled
        && app.conversation_history.is_empty()
    {
        app.session_store
            .load_memory(&app.project_dir)
            .and_then(|m| m.format_injection(app.loaded_config.config.memory.max_memory_entries))
    } else {
        None
    };

    let session_runtime = navi_core::session::SessionRuntime::spawn(
        ctx,
        app.harness_policy,
        initial_messages,
        memory_injection,
    );
    app.session_runtime = Some(session_runtime.clone());

    let tx = app.async_tx.clone();

    // Send the submission
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    if let Err(e) = session_runtime
        .submission_tx
        .send(navi_core::session::Submission {
            task: user_prompt,
            response_tx,
        })
    {
        tracing::error!("Failed to send submission: {}", e);
        let _ = tx.send(AsyncEvent::ModelError {
            message: format!("Failed to send submission: {e}"),
        });
        return;
    }

    app.stream_task = Some(tokio::spawn(async move {
        let mut response_rx = response_rx;
        loop {
            tokio::select! {
                event_opt = event_rx.recv() => {
                    if let Some(event) = event_opt {
                        let _ = tx.send(AsyncEvent::Agent(event));
                    } else {
                        break;
                    }
                }
                res = &mut response_rx => {
                    match res {
                        Ok(Ok(text)) => {
                            let _ = tx.send(AsyncEvent::TurnCompleted(Ok(text)));
                        }
                        Ok(Err(err)) => {
                            let _ = tx.send(AsyncEvent::TurnCompleted(Err(format!("{err:#}"))));
                        }
                        Err(_) => {
                            let _ = tx.send(AsyncEvent::TurnCompleted(Err("Turn cancelled or panicked".to_string())));
                        }
                    }
                    return;
                }
            }
        }

        match response_rx.await {
            Ok(Ok(text)) => {
                let _ = tx.send(AsyncEvent::TurnCompleted(Ok(text)));
            }
            Ok(Err(err)) => {
                let _ = tx.send(AsyncEvent::TurnCompleted(Err(format!("{err:#}"))));
            }
            Err(_) => {
                let _ = tx.send(AsyncEvent::TurnCompleted(Err(
                    "Turn cancelled or panicked".to_string()
                )));
            }
        }
    }));
}

fn active_assistant_message(app: &mut TuiApp) -> Option<&mut ChatMessage> {
    app.messages
        .iter_mut()
        .rev()
        .find(|message| message.role == ChatRole::Assistant)
}

fn update_active_assistant_status(app: &mut TuiApp) {
    let status = if !app.pending_approvals.is_empty() {
        if app.pending_approvals.len() == 1 {
            let req = &app.pending_approvals[0];
            let name = app
                .tool_invocations
                .get(&req.id)
                .map(|inv| inv.tool_name.as_str())
                .unwrap_or("tool");
            Some(format!("approval: {}", name))
        } else {
            Some(format!("approval: {} tools", app.pending_approvals.len()))
        }
    } else if !app.running_tools.is_empty() {
        if app.running_tools.len() == 1 {
            let name = app
                .running_tools
                .values()
                .next()
                .map(|inv| inv.tool_name.as_str())
                .unwrap_or("tool");
            Some(format!("tool: {}", name))
        } else {
            let names: Vec<&str> = app
                .running_tools
                .values()
                .map(|inv| inv.tool_name.as_str())
                .collect();
            Some(format!("tool: {}", names.join(", ")))
        }
    } else if app.is_loading {
        Some("thinking".to_string())
    } else {
        None
    };

    if let Some(msg) = active_assistant_message(app) {
        msg.status = status;
    }
}

fn finalize_active_assistant(app: &mut TuiApp, elapsed_ms: u64) {
    app.model_retry_attempts = 0;
    let (text, thinking) = {
        let Some(active) = active_assistant_message(app) else {
            return;
        };
        active.elapsed_ms = Some(elapsed_ms);
        active.status = None;
        (
            active.content.clone(),
            if active.thinking_content.is_empty() {
                None
            } else {
                Some(active.thinking_content.clone())
            },
        )
    };
    if text.trim().is_empty() {
        if let Some(active) = active_assistant_message(app) {
            active.content = "No response.".to_string();
        }
        return;
    }

    let word_count = text.split_whitespace().count();
    app.total_tokens_estimate += word_count * 4 / 3;
    app.conversation_history
        .push(ModelMessage::assistant(text.clone()));
    app.events.push(AgentEvent::ModelOutput { text, thinking });
    tracing::info!(elapsed_ms, "TUI model stream finalized");
}

fn handle_model_error(app: &mut TuiApp, message: String) {
    if should_retry_model_error(&message)
        && !is_usage_limit_error(&message)
        && app.model_retry_attempts < max_model_retries(app)
    {
        let next_attempt = app.model_retry_attempts + 1;
        let retry_delay = model_retry_delay(&message, next_attempt);
        tracing::warn!(
            error = %message,
            attempt = next_attempt,
            max = max_model_retries(app),
            retry_delay_ms = retry_delay.as_millis() as u64,
            "transient model error retrying"
        );
        push_diagnostic(app, format!("Retrying transient provider error: {message}"));
        app.model_retry_attempts = next_attempt;
        app.skip_next_model_done = false;
        app.is_loading = true;
        app.loading_start = None;
        remove_active_tool_placeholder(app);
        remove_active_empty_generation_placeholder(app);
        app.messages.push(ChatMessage {
            status: Some("retrying".to_string()),
            ..ChatMessage::new(
                ChatRole::Assistant,
                format!(
                    "Transient provider error: {message}\nRetrying agent step {}/{} in {}.",
                    app.model_retry_attempts,
                    max_model_retries(app),
                    human_duration(retry_delay),
                ),
            )
        });
        schedule_model_retry(app, retry_delay);
        return;
    }

    tracing::error!(error = %message, "model stream failed");
    push_diagnostic(app, format!("Model error: {message}"));
    app.skip_next_model_done = false;
    app.messages.push(ChatMessage {
        status: Some("error".to_string()),
        ..ChatMessage::new(
            ChatRole::Assistant,
            format_model_error_message(app, &message),
        )
    });
    app.events.push(AgentEvent::Error { message });
    app.is_loading = false;
    app.loading_start = None;
    app.stream_task = None;
}

fn schedule_model_retry(app: &mut TuiApp, delay: Duration) {
    let tx = app.async_tx.clone();
    app.stream_task = Some(tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        let _ = tx.send(AsyncEvent::RetryModel);
    }));
}

fn remove_active_empty_generation_placeholder(app: &mut TuiApp) {
    let Some(index) = app.messages.iter().rposition(|message| {
        message.role == ChatRole::Assistant
            && message.content.trim().is_empty()
            && message.thinking_content.trim().is_empty()
            && message.status.as_deref().is_some_and(|status| {
                status == "thinking" || status == "receiving" || status.starts_with("tool:")
            })
    }) else {
        return;
    };
    app.messages.remove(index);
}

fn should_retry_model_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("429")
        || message.contains("too many requests")
        || message.contains("unexpected eof")
        || message.contains("connection")
        || message.contains("timeout")
        || message.contains("timed out")
}

fn is_usage_limit_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("freeusagelimiterror")
        || message.contains("free usage limit")
        || message.contains("usage limit exceeded")
}

fn format_model_error_message(app: &TuiApp, message: &str) -> String {
    if is_usage_limit_error(message) {
        let model = app.loaded_config.config.model.name.as_str();
        let provider = selected_provider_label(app);
        let free_hint = if is_free_model_name(model) {
            "This selected model is a free-tier model. Free-tier quota can be exhausted even when the provider account still has paid/regular capacity."
        } else {
            "The selected provider reported a usage-limit error for this request."
        };
        format!(
            "⚠ Usage limit reached for {model} via {provider}.\n\n{free_hint}\n\n{message}\n\nUse ctrl+m and select a non-free model, or wait for the provider limit window to reset."
        )
    } else {
        format!("⚠ Error: {message}")
    }
}

fn is_free_model_name(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.ends_with("-free") || model.contains(" free")
}

fn provider_request_model_name(provider_id: &str, model: &str) -> String {
    if canonical_provider_id(provider_id) == "opencode" {
        opencode_zen_model_id(model).unwrap_or_else(|| model.to_string())
    } else {
        model.to_string()
    }
}

fn opencode_zen_model_id(model: &str) -> Option<String> {
    let normalized = model
        .trim()
        .trim_start_matches("opencode/")
        .to_ascii_lowercase()
        .replace([' ', '_'], "-");
    let collapsed = normalized
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    match collapsed.as_str() {
        "deepseek-v4-flash-free" => Some("deepseek-v4-flash-free".to_string()),
        "nemotron-3-super-free" => Some("nemotron-3-super-free".to_string()),
        "big-pickle" => Some("big-pickle".to_string()),
        "qwen3.6-plus" | "qwen-3.6-plus" => Some("qwen3.6-plus".to_string()),
        "qwen3.5-plus" | "qwen-3.5-plus" => Some("qwen3.5-plus".to_string()),
        "minimax-m2.7" | "mini-max-m2.7" => Some("minimax-m2.7".to_string()),
        "minimax-m2.5" | "mini-max-m2.5" => Some("minimax-m2.5".to_string()),
        "glm-5.1" => Some("glm-5.1".to_string()),
        "glm-5" => Some("glm-5".to_string()),
        "kimi-k2.6" => Some("kimi-k2.6".to_string()),
        "kimi-k2.5" => Some("kimi-k2.5".to_string()),
        "grok-build-0.1" => Some("grok-build-0.1".to_string()),
        _ => None,
    }
}

fn max_model_retries(app: &TuiApp) -> usize {
    match app.harness_policy.profile {
        navi_core::HarnessProfile::Small => 2,
        _ => 3,
    }
}

fn model_retry_delay(message: &str, attempt: usize) -> Duration {
    if let Some(delay) = parse_requested_retry_delay(message) {
        return delay.min(Duration::from_secs(60));
    }

    if message.to_ascii_lowercase().contains("429")
        || message.to_ascii_lowercase().contains("too many requests")
    {
        return Duration::from_secs((attempt as u64).saturating_mul(10).min(60));
    }

    Duration::from_secs(
        2_u64
            .saturating_pow(attempt.saturating_sub(1) as u32)
            .min(15),
    )
}

fn parse_requested_retry_delay(message: &str) -> Option<Duration> {
    let marker = "requested delay: Some(";
    let start = message.find(marker)? + marker.len();
    let end = message[start..].find(')')? + start;
    parse_duration_fragment(&message[start..end])
}

fn parse_duration_fragment(fragment: &str) -> Option<Duration> {
    let value = fragment.trim();
    if let Some(ms) = value.strip_suffix("ms") {
        return ms.trim().parse::<u64>().ok().map(Duration::from_millis);
    }
    if let Some(secs) = value.strip_suffix('s') {
        return secs.trim().parse::<f64>().ok().map(Duration::from_secs_f64);
    }
    None
}

fn human_duration(duration: Duration) -> String {
    if duration.as_secs() > 0 {
        format!("{}s", duration.as_secs())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

fn remove_active_tool_placeholder(app: &mut TuiApp) {
    let Some(index) = app.messages.iter().rposition(|message| {
        message.role == ChatRole::Assistant
            && message.content.trim().is_empty()
            && message.thinking_content.trim().is_empty()
            && message.status.as_deref().is_some_and(|status| {
                status.starts_with("tool:") || status.starts_with("approval:")
            })
    }) else {
        return;
    };
    app.messages.remove(index);
}

fn tool_compact_text(invocation: &ToolInvocation, result: &ToolResult) -> String {
    format!(
        "{} called · {}",
        invocation.tool_name,
        if result.ok { "success" } else { "error" }
    )
}

fn tool_full_content(invocation: &ToolInvocation, result: &ToolResult) -> String {
    let mut content = format!(
        "{} {}\n\n",
        if result.ok { "✓" } else { "✗" },
        tool_compact_text(invocation, result),
    );

    if let Some(formatted) = formatted_tool_output(invocation, result) {
        content.push_str(&formatted);
    } else {
        content.push_str(&generic_tool_summary(invocation, result));
    }

    content
}

fn formatted_tool_output(invocation: &ToolInvocation, result: &ToolResult) -> Option<String> {
    let obj = result.output.as_object()?;
    let mut content = String::new();

    if let Some(error) = obj.get("error").and_then(|v| v.as_str()) {
        content.push_str(&format!("Error: {error}\n"));
        if invocation.tool_name == "bash" {
            let stdout = obj.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
            let stderr = obj.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
            if !stdout.is_empty() {
                content.push_str("\nStdout:\n```\n");
                content.push_str(stdout);
                if !stdout.ends_with('\n') {
                    content.push('\n');
                }
                content.push_str("```\n");
            }
            if !stderr.is_empty() {
                content.push_str("\nStderr:\n```\n");
                content.push_str(stderr);
                if !stderr.ends_with('\n') {
                    content.push('\n');
                }
                content.push_str("```\n");
            }
        }
        return Some(content);
    }

    if !result.ok && invocation.tool_name != "bash" {
        return None;
    }

    if invocation.tool_name == "read_file" || invocation.tool_name == "view_file" {
        let path = obj.get("path").and_then(|v| v.as_str())?;
        content.push_str(&format!("View {path}\n\n"));
        if let Some(file_content) = obj.get("content").and_then(|v| v.as_str()) {
            let language = language_for_path(path);
            content.push_str(&format!("```{language}\n"));
            content.push_str(file_content);
            if !file_content.ends_with('\n') {
                content.push('\n');
            }
            content.push_str("```\n");
        }
    } else if invocation.tool_name == "write_file" {
        let path = obj.get("path").and_then(|v| v.as_str())?;
        let added = invocation
            .input
            .get("content")
            .and_then(|v| v.as_str())
            .map(count_changed_lines)
            .unwrap_or(0);
        content.push_str(&format!("Edited {path} (+{added} -0)\n"));
    } else if invocation.tool_name == "apply_patch" {
        if let Some(patch) = invocation.input.get("patch").and_then(|v| v.as_str()) {
            let summaries = patch_edit_summaries(patch);
            if summaries.is_empty() {
                content.push_str("Applied patch\n");
            } else {
                for summary in summaries {
                    content.push_str(&summary);
                    content.push('\n');
                }
            }
        } else {
            content.push_str("Applied patch successfully\n");
        }
        let stdout = obj.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        let stderr = obj.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
        if !stdout.is_empty() {
            content.push_str("\nStdout:\n```\n");
            content.push_str(stdout);
            if !stdout.ends_with('\n') {
                content.push('\n');
            }
            content.push_str("```\n");
        }
        if !stderr.is_empty() {
            content.push_str("\nStderr:\n```\n");
            content.push_str(stderr);
            if !stderr.ends_with('\n') {
                content.push('\n');
            }
            content.push_str("```\n");
        }
    } else if invocation.tool_name == "bash" {
        let status = obj.get("status").and_then(|v| v.as_i64());
        if let Some(status_code) = status {
            content.push_str(&format!("Command exited with status {status_code}\n"));
        } else {
            content.push_str("Command completed\n");
        }
        let stdout = obj.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        let stderr = obj.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
        if !stdout.is_empty() {
            content.push_str("\nStdout:\n```\n");
            content.push_str(stdout);
            if !stdout.ends_with('\n') {
                content.push('\n');
            }
            content.push_str("```\n");
        }
        if !stderr.is_empty() {
            content.push_str("\nStderr:\n```\n");
            content.push_str(stderr);
            if !stderr.ends_with('\n') {
                content.push('\n');
            }
            content.push_str("```\n");
        }
    } else if invocation.tool_name == "grep" {
        content.push_str("Found matches:\n\n");
        if let Some(matches) = obj.get("matches").and_then(|v| v.as_array()) {
            for m in matches {
                if let Some(m_obj) = m.as_object() {
                    let path = m_obj.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    let line = m_obj.get("line").and_then(|v| v.as_u64()).unwrap_or(0);
                    let text = m_obj.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    content.push_str(&format!("{path}:{line}: {text}\n"));
                }
            }
        }
    } else if invocation.tool_name == "list_files" {
        content.push_str("List files\n\n");
        if let Some(files) = obj.get("files").and_then(|v| v.as_array()) {
            for (i, file) in files.iter().enumerate() {
                if let Some(file) = file.as_str() {
                    content.push_str(&format!("{:>4}  {}\n", i + 1, file));
                }
            }
        }
    } else {
        return None;
    }

    if obj.get("truncated").and_then(|v| v.as_bool()) == Some(true) {
        content.push_str("... (truncated)\n");
    }
    Some(content)
}

fn generic_tool_summary(invocation: &ToolInvocation, result: &ToolResult) -> String {
    if result.ok {
        format!("{} completed successfully\n", invocation.tool_name)
    } else if let Some(error) = result.output.get("error").and_then(|v| v.as_str()) {
        format!("Error: {error}\n")
    } else {
        format!("{} failed\n", invocation.tool_name)
    }
}

fn count_changed_lines(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.lines().count().max(1)
    }
}

fn patch_edit_summaries(patch: &str) -> Vec<String> {
    let mut summaries = Vec::new();
    let mut current_path: Option<String> = None;
    let mut added = 0usize;
    let mut removed = 0usize;

    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            flush_patch_summary(&mut summaries, &mut current_path, &mut added, &mut removed);
            current_path = Some(path.to_string());
            continue;
        }
        if current_path.is_none() {
            if let Some(path) = line.strip_prefix("*** Update File: ") {
                current_path = Some(path.to_string());
                continue;
            }
            if let Some(path) = line.strip_prefix("*** Add File: ") {
                current_path = Some(path.to_string());
                continue;
            }
        }
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    flush_patch_summary(&mut summaries, &mut current_path, &mut added, &mut removed);

    summaries
}

fn flush_patch_summary(
    summaries: &mut Vec<String>,
    current_path: &mut Option<String>,
    added: &mut usize,
    removed: &mut usize,
) {
    if let Some(path) = current_path.take() {
        summaries.push(format!("Edited {path} (+{} -{})", *added, *removed));
        *added = 0;
        *removed = 0;
    }
}

fn language_for_path(path: &str) -> &'static str {
    match path
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or_default()
    {
        "rs" => "rust",
        "toml" => "toml",
        "json" => "json",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" => "typescript",
        "jsx" => "javascript",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cc" | "cpp" | "hpp" => "cpp",
        "sh" | "bash" => "bash",
        "zsh" => "zsh",
        "fish" => "fish",
        "md" | "markdown" => "markdown",
        "yaml" | "yml" => "yaml",
        "html" => "html",
        "css" => "css",
        "xml" => "xml",
        "sql" => "sql",
        _ => "",
    }
}

fn approve_pending_tool(app: &mut TuiApp) {
    if !app.pending_approvals.is_empty() {
        let request = app.pending_approvals.remove(0);
        tracing::info!(invocation_id = %request.id, "tool approval accepted via pending_approvals");
        if let Some(ctx) = &app.turn_context {
            let decision = ApprovalDecision::Approved {
                id: request.id.clone(),
            };
            ctx.resolve_approval(decision);
        }
        update_active_assistant_status(app);
    }
}

fn deny_pending_tool(app: &mut TuiApp) {
    if !app.pending_approvals.is_empty() {
        let request = app.pending_approvals.remove(0);
        tracing::warn!(invocation_id = %request.id, "tool approval denied via pending_approvals");
        push_diagnostic(app, format!("Denied tool ID: {}", request.id));
        if let Some(ctx) = &app.turn_context {
            let decision = ApprovalDecision::Denied {
                id: request.id.clone(),
            };
            ctx.resolve_approval(decision);
        }
        update_active_assistant_status(app);
    }
}

fn cancel_stream(app: &mut TuiApp) {
    tracing::warn!(
        had_stream = app.stream_task.is_some(),
        had_tool = app.tool_task.is_some(),
        "active operation cancelled"
    );
    push_diagnostic(app, "Cancelled active operation.");
    if let Some(task) = app.stream_task.take() {
        task.abort();
    }
    if let Some(task) = app.tool_task.take() {
        task.abort();
    }
    app.is_loading = false;
    app.loading_start = None;
    app.session_runtime = None;
    app.turn_context = None;
    app.pending_approvals.clear();
    app.running_tools.clear();
    app.skip_next_model_done = false;
    if let Some(active) = active_assistant_message(app) {
        active.status = Some("cancelled".to_string());
        if active.content.is_empty() {
            active.content = "Cancelled.".to_string();
        }
    }
}

fn retry_last_response(app: &mut TuiApp) {
    if app.is_loading {
        cancel_stream(app);
    }

    if app
        .messages
        .last()
        .is_some_and(|message| message.role == ChatRole::Assistant)
    {
        app.messages.pop();
    }
    if app
        .conversation_history
        .last()
        .is_some_and(|message| matches!(message.role, ModelRole::Assistant))
    {
        app.conversation_history.pop();
    }
    if app
        .events
        .last()
        .is_some_and(|event| matches!(event, AgentEvent::ModelOutput { .. }))
    {
        app.events.pop();
    }

    if app
        .conversation_history
        .last()
        .is_some_and(|message| matches!(message.role, ModelRole::User))
    {
        start_streaming_request(app);
    }
}

fn build_provider(
    loaded_config: &LoadedConfig,
    credential_store: &CredentialStore,
) -> Option<Arc<dyn ModelProvider>> {
    let provider_config =
        resolve_provider_config(&loaded_config.config, &loaded_config.config.model.provider)?;

    let api_key = resolve_provider_api_key(
        credential_store,
        &provider_config,
        &loaded_config.config.model.provider,
    )
    .or_else(|| {
        if model_can_run_publicly(&provider_config.id, &loaded_config.config.model.name) {
            Some("public".to_string())
        } else {
            None
        }
    })?;

    match OpenAiProvider::from_provider_config_with_key(&provider_config, api_key) {
        Ok(provider) => Some(Arc::new(provider)),
        Err(_) => None,
    }
}

fn resolve_provider_api_key(
    credential_store: &CredentialStore,
    provider_config: &navi_core::ProviderConfig,
    requested_provider_id: &str,
) -> Option<String> {
    provider_env_api_key_for_config(provider_config)
        .or_else(|| opencode_auth_json_api_key(credential_store, &provider_config.id))
        .or_else(|| credential_store.get_api_key(&provider_config.id))
        .or_else(|| {
            if requested_provider_id != provider_config.id {
                credential_store.get_api_key(requested_provider_id)
            } else {
                None
            }
        })
}

fn provider_env_api_key_for_config(provider_config: &navi_core::ProviderConfig) -> Option<String> {
    if canonical_provider_id(&provider_config.id) == "opencode" {
        provider_env_api_key("OPENCODE_API_KEY")
            .or_else(|| provider_env_api_key("OPENCODE_ZEN_API_KEY"))
    } else {
        provider_env_api_key(&provider_config.api_key_env)
    }
}

fn opencode_auth_json_api_key(
    credential_store: &CredentialStore,
    provider_id: &str,
) -> Option<String> {
    if canonical_provider_id(provider_id) == "opencode" {
        credential_store.get_opencode_api_key()
    } else {
        None
    }
}

fn provider_env_api_key(env_var: &str) -> Option<String> {
    let key = std::env::var(env_var).ok()?;
    if key.is_empty() { None } else { Some(key) }
}

fn rebuild_provider(app: &mut TuiApp) {
    app.model_provider = build_provider(&app.loaded_config, &app.credential_store);
    app.harness_policy = select_harness_policy(&app.loaded_config.config);
    refresh_system_context(app);
    tracing::info!(
        provider = %app.loaded_config.config.model.provider,
        model = %app.loaded_config.config.model.name,
        "provider rebuilt"
    );
}

fn reset_system_context(app: &mut TuiApp) {
    app.conversation_history = vec![ModelMessage::system(build_system_prompt(
        &app.loaded_config.config,
        &app.project_dir,
    ))];
    app.run_state = AgentRunState::default();
}

fn refresh_system_context(app: &mut TuiApp) {
    let system = ModelMessage::system(build_system_prompt(
        &app.loaded_config.config,
        &app.project_dir,
    ));
    if let Some(first) = app.conversation_history.first_mut() {
        *first = system;
    } else {
        app.conversation_history.push(system);
    }
}

fn provider_has_api_key(app: &TuiApp, provider_id: &str) -> bool {
    resolve_provider_config(&app.loaded_config.config, provider_id)
        .and_then(|provider_config| {
            resolve_provider_api_key(&app.credential_store, &provider_config, provider_id)
        })
        .is_some()
}

fn model_can_run_publicly(provider_id: &str, model: &str) -> bool {
    canonical_provider_id(provider_id) == "opencode" && is_free_model_name(model)
}

fn model_is_available_for_selection(app: &TuiApp, model: &ModelOption) -> bool {
    provider_has_api_key(app, &model.provider_id)
        || model_can_run_publicly(&model.provider_id, &model.name)
}

fn apply_model_selection(app: &mut TuiApp, model_index: usize) {
    let Some(model) = app.models.get(model_index) else {
        return;
    };

    app.loaded_config.config.model.provider = model.provider_id.clone();
    app.loaded_config.config.model.name = model.name.clone();
    app.selected_model = model_index;
    app.model_scroll = 0;
    if canonical_provider_id(&model.provider_id) == "opencode" && is_free_model_name(&model.name) {
        show_notification(
            app,
            "OpenCode Zen",
            "Free model selected. NAVI will use your Zen key when configured.",
        );
    }
    rebuild_provider(app);
}

fn selected_or_pending_provider_id(app: &TuiApp) -> String {
    app.pending_provider_setup.clone().unwrap_or_else(|| {
        app.pending_model_selection
            .and_then(|index| app.models.get(index))
            .map(|model| model.provider_id.clone())
            .unwrap_or_else(|| app.loaded_config.config.model.provider.clone())
    })
}

fn selected_or_pending_provider_label(app: &TuiApp) -> String {
    if let Some(provider_id) = &app.pending_provider_setup {
        return resolve_provider_config(&app.loaded_config.config, provider_id)
            .map(|provider| provider.label)
            .unwrap_or_else(|| provider_id.clone());
    }

    app.pending_model_selection
        .and_then(|index| app.models.get(index))
        .map(|model| model.provider_label.clone())
        .unwrap_or_else(|| selected_provider_label(app).to_string())
}

fn save_api_key_and_rebuild(app: &mut TuiApp) {
    let key = app.api_key_input.trim().to_string();
    if key.is_empty() {
        return;
    }

    let provider_id = selected_or_pending_provider_id(app);
    if let Err(err) = app.credential_store.set_api_key(&provider_id, &key) {
        show_notification(app, "Credentials", format!("Failed to save key: {err:#}"));
    } else {
        show_notification(
            app,
            "Credentials",
            format!("API key saved for provider \"{provider_id}\"."),
        );
    }

    let return_to_providers = app.pending_provider_setup.take().is_some();
    if let Some(model_index) = app.pending_model_selection.take() {
        apply_model_selection(app, model_index);
    } else {
        rebuild_provider(app);
    }
    app.api_key_input.clear();
    app.api_key_cursor = 0;
    app.mode = if return_to_providers {
        Mode::Providers
    } else {
        Mode::Normal
    };
}

fn current_provider_env_var(app: &TuiApp) -> String {
    let provider_id = selected_or_pending_provider_id(app);
    resolve_provider_config(&app.loaded_config.config, &provider_id)
        .map(|p| p.api_key_env.clone())
        .unwrap_or_else(|| "API_KEY".to_string())
}

fn current_provider_credential_status(app: &TuiApp) -> String {
    let provider_id = selected_or_pending_provider_id(app);
    let Some(provider_config) = resolve_provider_config(&app.loaded_config.config, &provider_id)
    else {
        return "unknown provider".to_string();
    };
    let model_name = app
        .pending_model_selection
        .and_then(|index| app.models.get(index))
        .map(|model| model.name.as_str())
        .unwrap_or(app.loaded_config.config.model.name.as_str());

    if canonical_provider_id(&provider_config.id) == "opencode" {
        if provider_env_api_key("OPENCODE_API_KEY").is_some() {
            "env OPENCODE_API_KEY".to_string()
        } else if provider_env_api_key("OPENCODE_ZEN_API_KEY").is_some() {
            "env OPENCODE_ZEN_API_KEY".to_string()
        } else if app.credential_store.get_opencode_api_key().is_some() {
            "OpenCode auth.json".to_string()
        } else if resolve_provider_api_key(&app.credential_store, &provider_config, &provider_id)
            .is_some()
        {
            "stored credential".to_string()
        } else if model_can_run_publicly(&provider_id, model_name) {
            "free model access without key".to_string()
        } else {
            "missing".to_string()
        }
    } else if provider_env_api_key(&provider_config.api_key_env).is_some() {
        format!("env {}", provider_config.api_key_env)
    } else if resolve_provider_api_key(&app.credential_store, &provider_config, &provider_id)
        .is_some()
    {
        "stored credential".to_string()
    } else if model_can_run_publicly(&provider_id, model_name) {
        "free model access without key".to_string()
    } else {
        "missing".to_string()
    }
}

struct ProviderAuthStatus {
    configured: bool,
    label: String,
}

fn provider_auth_status(app: &TuiApp, provider_config: &ProviderConfig) -> ProviderAuthStatus {
    if canonical_provider_id(&provider_config.id) == "opencode" {
        if provider_env_api_key("OPENCODE_API_KEY").is_some() {
            return ProviderAuthStatus {
                configured: true,
                label: "env".to_string(),
            };
        }
        if provider_env_api_key("OPENCODE_ZEN_API_KEY").is_some() {
            return ProviderAuthStatus {
                configured: true,
                label: "env".to_string(),
            };
        }
        if app.credential_store.get_opencode_api_key().is_some() {
            return ProviderAuthStatus {
                configured: true,
                label: "opencode".to_string(),
            };
        }
    } else if provider_env_api_key(&provider_config.api_key_env).is_some() {
        return ProviderAuthStatus {
            configured: true,
            label: "env".to_string(),
        };
    }

    if resolve_provider_api_key(&app.credential_store, provider_config, &provider_config.id)
        .is_some()
    {
        ProviderAuthStatus {
            configured: true,
            label: "stored".to_string(),
        }
    } else {
        ProviderAuthStatus {
            configured: false,
            label: "missing".to_string(),
        }
    }
}

fn provider_supports_oauth(provider_id: &str) -> bool {
    canonical_provider_id(provider_id) == "github-copilot"
}

fn sync_provider_settings_scroll(app: &mut TuiApp, visible_rows: usize) {
    if app.selected_provider_setting < app.provider_settings_scroll {
        app.provider_settings_scroll = app.selected_provider_setting;
    } else if app.selected_provider_setting >= app.provider_settings_scroll + visible_rows {
        app.provider_settings_scroll = app
            .selected_provider_setting
            .saturating_sub(visible_rows.saturating_sub(1));
    }
}

fn start_provider_oauth(app: &mut TuiApp, provider: &ProviderConfig) {
    if !provider_supports_oauth(&provider.id) {
        show_notification(
            app,
            "OAuth",
            format!("{} uses API key setup.", provider.label),
        );
        return;
    }
    if app.is_loading {
        return;
    }

    app.is_loading = true;
    app.loading_start = Some(Instant::now());
    let tx = app.async_tx.clone();
    let credential_store = app.credential_store.clone();
    let provider_id = provider.id.clone();
    app.stream_task = Some(tokio::spawn(async move {
        let result = github_copilot_device_oauth(&tx, &provider_id, credential_store).await;
        let _ = tx.send(AsyncEvent::OAuthCompleted {
            provider_id,
            result,
        });
    }));
}

async fn github_copilot_device_oauth(
    tx: &mpsc::UnboundedSender<AsyncEvent>,
    provider_id: &str,
    credential_store: CredentialStore,
) -> Result<(), String> {
    const CLIENT_ID: &str = "Ov23li8tweQw6odWQebz";
    let client = reqwest::Client::new();
    let device_response = client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .header("User-Agent", "navi/0.1.0")
        .json(&serde_json::json!({
            "client_id": CLIENT_ID,
            "scope": "read:user",
        }))
        .send()
        .await
        .map_err(|err| err.to_string())?;

    if !device_response.status().is_success() {
        return Err(format!(
            "device authorization failed: {}",
            device_response.status()
        ));
    }

    let device_data: serde_json::Value = device_response
        .json()
        .await
        .map_err(|err| err.to_string())?;
    let verification_uri = device_data
        .get("verification_uri")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing verification URL".to_string())?
        .to_string();
    let user_code = device_data
        .get("user_code")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing user code".to_string())?
        .to_string();
    let device_code = device_data
        .get("device_code")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing device code".to_string())?
        .to_string();
    let mut interval = device_data
        .get("interval")
        .and_then(|value| value.as_u64())
        .unwrap_or(5)
        .max(1);

    let _ = tx.send(AsyncEvent::OAuthDeviceStarted {
        provider_id: provider_id.to_string(),
        verification_uri,
        user_code,
    });

    for _ in 0..120 {
        tokio::time::sleep(Duration::from_secs(interval + 3)).await;
        let token_response = client
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .header("User-Agent", "navi/0.1.0")
            .json(&serde_json::json!({
                "client_id": CLIENT_ID,
                "device_code": device_code,
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            }))
            .send()
            .await
            .map_err(|err| err.to_string())?;

        if !token_response.status().is_success() {
            return Err(format!(
                "token exchange failed: {}",
                token_response.status()
            ));
        }

        let token_data: serde_json::Value =
            token_response.json().await.map_err(|err| err.to_string())?;
        if let Some(access_token) = token_data
            .get("access_token")
            .and_then(|value| value.as_str())
        {
            credential_store
                .set_api_key(provider_id, access_token)
                .map_err(|err| err.to_string())?;
            return Ok(());
        }

        match token_data.get("error").and_then(|value| value.as_str()) {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += 5,
            Some(error) => return Err(error.to_string()),
            None => {}
        }
    }

    Err("device authorization timed out".to_string())
}

fn show_notification(app: &mut TuiApp, title: impl Into<String>, message: impl Into<String>) {
    app.notification = Some(Notification {
        title: title.into(),
        message: message.into(),
        created_at: Instant::now(),
        ttl: NOTIFICATION_TTL,
    });
}

fn push_diagnostic(app: &mut TuiApp, message: impl Into<String>) {
    app.diagnostics.push(message.into());
    if app.diagnostics.len() > 20 {
        let overflow = app.diagnostics.len() - 20;
        app.diagnostics.drain(0..overflow);
    }
}

fn expire_notification(app: &mut TuiApp) -> bool {
    let expired = app
        .notification
        .as_ref()
        .is_some_and(|notification| notification.created_at.elapsed() >= notification.ttl);
    if expired {
        app.notification = None;
    }
    expired
}

fn visible_notification(app: &TuiApp) -> Option<&Notification> {
    app.notification
        .as_ref()
        .filter(|notification| notification.created_at.elapsed() < notification.ttl)
}

// ─── key handling ──────────────────────────────────────────────────────────────
fn handle_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    if !app.pending_approvals.is_empty() {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                approve_pending_tool(app);
                return false;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                deny_pending_tool(app);
                return false;
            }
            _ => {}
        }
    }

    if app.mode == Mode::Normal
        && code == KeyCode::Esc
        && (app.is_loading || app.stream_task.is_some() || app.tool_task.is_some())
    {
        cancel_stream(app);
        return false;
    }

    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('c') => return true,
            KeyCode::Char('d') => {
                app.mode = Mode::Debug;
                tracing::info!("debug modal opened");
                return false;
            }
            KeyCode::Char('g') => {
                app.yolo_mode = !app.yolo_mode;
                tracing::info!(enabled = app.yolo_mode, "yolo mode toggled");
                show_notification(
                    app,
                    "Tools",
                    format!(
                        "YOLO mode {}.",
                        if app.yolo_mode { "enabled" } else { "disabled" }
                    ),
                );
                return false;
            }
            KeyCode::Char('p') => {
                app.mode = Mode::Commands;
                app.command_filter.clear();
                app.selected_command = 0;
                return false;
            }
            KeyCode::Char('m') => {
                open_model_picker(app);
                return false;
            }
            KeyCode::Char('o') | KeyCode::Char('O') => {
                app.full_tool_view = !app.full_tool_view;
                show_notification(
                    app,
                    "Tools",
                    if app.full_tool_view {
                        "Full tool view enabled."
                    } else {
                        "Compact tool view enabled."
                    },
                );
                return false;
            }
            KeyCode::Char('j') | KeyCode::Char('\n') | KeyCode::Char('\r') | KeyCode::Enter => {
                if !app.input.trim().is_empty() && !app.is_loading {
                    submit_message(app);
                }
                return false;
            }
            KeyCode::Char('n') => {
                app.messages.clear();
                reset_system_context(app);
                app.input.clear();
                app.input_cursor = 0;
                app.scroll_offset = 0;
                app.total_tokens_estimate = 0;
                show_notification(app, "Layer", "New layer started.");
                return false;
            }
            _ => {}
        }
    }

    match app.mode {
        Mode::Normal => handle_normal_key(app, code, modifiers),
        Mode::Commands => handle_command_key(app, code),
        Mode::Models => handle_model_key(app, code, modifiers),
        Mode::ApiKeyEntry => handle_api_key_key(app, code, modifiers),
        Mode::Thinking => handle_thinking_key(app, code),
        Mode::Sessions => handle_sessions_key(app, code),
        Mode::Settings => handle_settings_key(app, code),
        Mode::Providers => handle_providers_key(app, code),
        Mode::Debug => handle_debug_key(app, code),
        Mode::Help => handle_help_key(app, code),
    }
}

fn handle_debug_key(app: &mut TuiApp, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc | KeyCode::Enter => {
            app.mode = Mode::Normal;
            tracing::info!("debug modal closed");
        }
        _ => {}
    }
    false
}

fn handle_help_key(app: &mut TuiApp, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') => {
            app.mode = Mode::Normal;
        }
        _ => {}
    }
    false
}

fn open_model_picker(app: &mut TuiApp) {
    app.mode = Mode::Models;
    app.pending_model_selection = None;
    app.model_filter.clear();
    app.model_scroll = 0;

    let rows = build_model_rows(app);
    app.selected_model = first_model_index(&rows).unwrap_or(app.selected_model);
}

fn open_thinking_picker(app: &mut TuiApp) {
    app.mode = Mode::Thinking;
    app.selected_thinking = app.thinking_level as usize;
}

const THINKING_OPTIONS: &[ThinkingLevel] = &[
    ThinkingLevel::Max,
    ThinkingLevel::High,
    ThinkingLevel::Medium,
    ThinkingLevel::Low,
    ThinkingLevel::Off,
];

fn handle_thinking_key(app: &mut TuiApp, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Down => {
            app.selected_thinking = (app.selected_thinking + 1).min(THINKING_OPTIONS.len() - 1);
        }
        KeyCode::Up => {
            app.selected_thinking = app.selected_thinking.saturating_sub(1);
        }
        KeyCode::Enter => {
            let level = THINKING_OPTIONS[app.selected_thinking];
            app.thinking_level = level;
            app.mode = Mode::Normal;
            show_notification(
                app,
                "Thinking",
                format!("Thinking set to {}.", level.label()),
            );
        }
        _ => {}
    }

    false
}

fn handle_settings_key(app: &mut TuiApp, code: KeyCode) -> bool {
    const SETTINGS_COUNT: usize = 3;
    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Down => {
            app.selected_setting = (app.selected_setting + 1).min(SETTINGS_COUNT - 1);
        }
        KeyCode::Up => {
            app.selected_setting = app.selected_setting.saturating_sub(1);
        }
        KeyCode::Char(' ') | KeyCode::Enter => match app.selected_setting {
            0 => {
                app.show_thinking = !app.show_thinking;
                show_notification(
                    app,
                    "Settings",
                    if app.show_thinking {
                        "Thinking text visible."
                    } else {
                        "Thinking text hidden."
                    },
                );
            }
            1 => {
                app.full_tool_view = !app.full_tool_view;
                show_notification(
                    app,
                    "Settings",
                    if app.full_tool_view {
                        "Full tool output visible."
                    } else {
                        "Tool output compacted."
                    },
                );
            }
            2 => {
                app.mode = Mode::Providers;
                app.selected_provider_setting = 0;
                app.provider_settings_scroll = 0;
            }
            _ => {}
        },
        _ => {}
    }
    false
}

fn handle_providers_key(app: &mut TuiApp, code: KeyCode) -> bool {
    let providers = navi_core::provider_catalog(&app.loaded_config.config);
    let max_index = providers.len().saturating_sub(1);
    match code {
        KeyCode::Esc => app.mode = Mode::Settings,
        KeyCode::Down => {
            app.selected_provider_setting = (app.selected_provider_setting + 1).min(max_index);
            sync_provider_settings_scroll(app, 12);
        }
        KeyCode::Up => {
            app.selected_provider_setting = app.selected_provider_setting.saturating_sub(1);
            sync_provider_settings_scroll(app, 12);
        }
        KeyCode::Enter | KeyCode::Char('k') => {
            if let Some(provider) = providers.get(app.selected_provider_setting) {
                app.pending_provider_setup = Some(provider.id.clone());
                app.pending_model_selection = None;
                app.api_key_input.clear();
                app.api_key_cursor = 0;
                app.mode = Mode::ApiKeyEntry;
            }
        }
        KeyCode::Char('o') | KeyCode::Char('O') => {
            if let Some(provider) = providers.get(app.selected_provider_setting) {
                start_provider_oauth(app, provider);
            }
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            if let Some(provider) = providers.get(app.selected_provider_setting) {
                let provider_id = provider.id.clone();
                sync_provider_tui(app, &provider_id);
            }
        }
        _ => {}
    }
    false
}

fn open_sessions_picker(app: &mut TuiApp) {
    app.saved_sessions = load_saved_sessions(&app.session_store);
    app.mode = Mode::Sessions;
    app.selected_session = 0;
    app.session_scroll = 0;
}

fn handle_sessions_key(app: &mut TuiApp, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Down => {
            app.selected_session =
                (app.selected_session + 1).min(app.saved_sessions.len().saturating_sub(1));
        }
        KeyCode::Up => {
            app.selected_session = app.selected_session.saturating_sub(1);
        }
        KeyCode::Enter => {
            if let Some(snapshot) = app.saved_sessions.get(app.selected_session).cloned() {
                save_current_session(app);
                load_session(app, &snapshot);
            }
            app.mode = Mode::Normal;
        }
        KeyCode::Delete => {
            if let Some(snapshot) = app.saved_sessions.get(app.selected_session) {
                let path = app
                    .session_store
                    .root()
                    .join(format!("{}.json", snapshot.id.0));
                let _ = std::fs::remove_file(&path);
            }
            app.saved_sessions = load_saved_sessions(&app.session_store);
            app.selected_session = app
                .selected_session
                .min(app.saved_sessions.len().saturating_sub(1));
        }
        _ => {}
    }

    false
}

fn handle_normal_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Left | KeyCode::Char('b') => move_input_previous_control_stop(app),
            KeyCode::Right | KeyCode::Char('f') => move_input_next_control_stop(app),
            KeyCode::Backspace
            | KeyCode::Char('h')
            | KeyCode::Char('w')
            | KeyCode::Char('\u{7f}') => delete_input_previous_hump(app),
            KeyCode::Delete => delete_input_next_hump(app),
            KeyCode::Char('a') => app.input_cursor = 0,
            KeyCode::Char('e') => app.input_cursor = app.input.len(),
            KeyCode::Char('u') => {
                app.input.drain(..app.input_cursor);
                app.input_cursor = 0;
            }
            KeyCode::Char('k') => {
                app.input.truncate(app.input_cursor);
            }
            _ => return false,
        }
        return false;
    }

    if modifiers.contains(KeyModifiers::ALT) {
        match code {
            KeyCode::Left | KeyCode::Char('b') | KeyCode::Char(',') => {
                move_input_previous_hump(app)
            }
            KeyCode::Right | KeyCode::Char('f') | KeyCode::Char('.') => move_input_next_hump(app),
            KeyCode::Backspace | KeyCode::Char('h') | KeyCode::Char('\u{7f}') => {
                delete_input_previous_space_word(app)
            }
            KeyCode::Delete | KeyCode::Char('d') => delete_input_next_hump(app),
            _ => return false,
        }
        return false;
    }

    match code {
        KeyCode::Char('/') if app.input.is_empty() => {
            app.mode = Mode::Commands;
            app.command_filter.clear();
            app.selected_command = 0;
        }
        KeyCode::Char('?') if app.input.is_empty() => {
            app.mode = Mode::Help;
        }
        KeyCode::Char('q') if app.input.is_empty() && app.messages.is_empty() => return true,
        KeyCode::Char(ch) => insert_input_char(app, ch),
        KeyCode::Backspace => {
            delete_input_previous_char(app);
        }
        KeyCode::Delete => {
            delete_input_next_char(app);
        }
        KeyCode::Left => {
            move_input_previous_char(app);
        }
        KeyCode::Right => {
            move_input_next_char(app);
        }
        KeyCode::Home => {
            app.input_cursor = 0;
        }
        KeyCode::End => {
            app.input_cursor = app.input.len();
        }
        KeyCode::Up => {
            app.scroll_offset = app.scroll_offset.saturating_add(3);
        }
        KeyCode::Down => {
            app.scroll_offset = app.scroll_offset.saturating_sub(3);
        }
        KeyCode::PageUp => {
            app.scroll_offset = app.scroll_offset.saturating_add(15);
        }
        KeyCode::PageDown => {
            app.scroll_offset = app.scroll_offset.saturating_sub(15);
        }
        KeyCode::Enter => {
            insert_input_char(app, '\n');
        }
        KeyCode::Esc => {
            if app.is_loading {
                cancel_stream(app);
            } else {
                app.scroll_offset = 0;
            }
        }
        _ => {}
    }

    false
}

fn handle_command_key(app: &mut TuiApp, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Char(ch) => {
            app.command_filter.push(ch);
            app.selected_command = 0;
        }
        KeyCode::Backspace => {
            app.command_filter.pop();
            app.selected_command = app
                .selected_command
                .min(filtered_commands(app).len().saturating_sub(1));
        }
        KeyCode::Down | KeyCode::Tab => {
            let len = filtered_commands(app).len();
            if len > 0 {
                app.selected_command = (app.selected_command + 1).min(len - 1);
            }
        }
        KeyCode::Up => {
            app.selected_command = app.selected_command.saturating_sub(1);
        }
        KeyCode::Enter => return run_selected_command(app),
        _ => {}
    }

    false
}

fn handle_model_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    let rows = build_model_rows(app);
    // List visible height is approximately modal height (22) minus decoration (~7 rows)
    let visible_rows = 14u16;
    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
            sync_models_tui(app);
            app.mode = Mode::Normal;
        }
        KeyCode::Char('e') if modifiers.contains(KeyModifiers::CONTROL) => {
            if selected_model_in_rows(&rows, app.selected_model).is_some() {
                app.pending_model_selection = Some(app.selected_model);
                app.mode = Mode::ApiKeyEntry;
                app.api_key_input.clear();
                app.api_key_cursor = 0;
            }
        }
        KeyCode::Tab => {
            // Sync just the provider that owns the currently selected model
            let provider_id = app
                .models
                .get(app.selected_model)
                .map(|m| m.provider_id.clone());
            if let Some(pid) = provider_id {
                sync_provider_tui(app, &pid);
            }
            app.mode = Mode::Normal;
        }
        KeyCode::Char(ch) => {
            app.model_filter.push(ch);
            app.model_scroll = 0;
            app.selected_model =
                first_model_index(&build_model_rows(app)).unwrap_or(app.selected_model);
        }
        KeyCode::Backspace => {
            app.model_filter.pop();
            app.model_scroll = 0;
            app.selected_model =
                first_model_index(&build_model_rows(app)).unwrap_or(app.selected_model);
        }
        KeyCode::Down => {
            app.selected_model = next_model_index(app, &rows);
            sync_scroll_to_selection(app, &rows, visible_rows);
        }
        KeyCode::Up => {
            app.selected_model = previous_model_index(app, &rows);
            sync_scroll_to_selection(app, &rows, visible_rows);
        }
        KeyCode::Enter => {
            if selected_model_in_rows(&rows, app.selected_model).is_none() {
                return false;
            }
            let model = &app.models[app.selected_model];
            if model_is_available_for_selection(app, model) {
                apply_model_selection(app, app.selected_model);
                app.pending_model_selection = None;
                app.mode = Mode::Normal;
            } else {
                app.pending_model_selection = Some(app.selected_model);
                app.mode = Mode::ApiKeyEntry;
                app.api_key_input.clear();
                app.api_key_cursor = 0;
            }
        }
        _ => {}
    }

    false
}

fn handle_api_key_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('a') => {
                app.api_key_cursor = 0;
                return false;
            }
            KeyCode::Char('e') => {
                app.api_key_cursor = app.api_key_input.len();
                return false;
            }
            KeyCode::Char('u') => {
                app.api_key_input.drain(..app.api_key_cursor);
                app.api_key_cursor = 0;
                return false;
            }
            // ctrl+v is handled as a paste by the terminal — characters arrive as Char events
            _ => return false,
        }
    }

    match code {
        KeyCode::Esc => {
            app.api_key_input.clear();
            app.api_key_cursor = 0;
            app.pending_model_selection = None;
            let return_to_providers = app.pending_provider_setup.take().is_some();
            app.mode = if return_to_providers {
                Mode::Providers
            } else {
                Mode::Normal
            };
        }
        KeyCode::Enter => {
            save_api_key_and_rebuild(app);
        }
        KeyCode::Char(ch) => {
            app.api_key_input.insert(app.api_key_cursor, ch);
            app.api_key_cursor += ch.len_utf8();
        }
        KeyCode::Backspace => {
            if app.api_key_cursor > 0 {
                let prev =
                    previous_char_boundary(&app.api_key_input, app.api_key_cursor).unwrap_or(0);
                app.api_key_input.drain(prev..app.api_key_cursor);
                app.api_key_cursor = prev;
            }
        }
        KeyCode::Left => {
            if let Some(prev) = previous_char_boundary(&app.api_key_input, app.api_key_cursor) {
                app.api_key_cursor = prev;
            }
        }
        KeyCode::Right => {
            if let Some(next) = next_char_boundary(&app.api_key_input, app.api_key_cursor) {
                app.api_key_cursor = next;
            }
        }
        KeyCode::Home => {
            app.api_key_cursor = 0;
        }
        KeyCode::End => {
            app.api_key_cursor = app.api_key_input.len();
        }
        _ => {}
    }

    false
}

fn run_selected_command(app: &mut TuiApp) -> bool {
    let commands = filtered_commands(app);
    let Some(command) = commands.get(app.selected_command).copied() else {
        app.mode = Mode::Normal;
        return false;
    };

    match command.action {
        CommandAction::NewSession => {
            app.messages.clear();
            reset_system_context(app);
            app.input.clear();
            app.input_cursor = 0;
            app.scroll_offset = 0;
            app.total_tokens_estimate = 0;
            app.mode = Mode::Normal;
        }
        CommandAction::SwitchModel => {
            open_model_picker(app);
        }
        CommandAction::RetryLast => {
            retry_last_response(app);
        }
        CommandAction::OpenThinking => {
            open_thinking_picker(app);
        }
        CommandAction::Compact => {
            if app.is_loading {
                show_notification(app, "Compact", "Cannot compact while a request is active.");
            } else {
                show_notification(
                    app,
                    "Compact",
                    "Compaction will trigger on next request if context is full.",
                );
                app.compact_state.last_input_tokens = Some(app.compact_state.context_window);
            }
            app.mode = Mode::Normal;
        }
        CommandAction::Sessions => {
            open_sessions_picker(app);
        }
        CommandAction::SyncModels => {
            sync_models_tui(app);
            app.mode = Mode::Normal;
        }
        CommandAction::Quit => return true,
        CommandAction::Settings => {
            app.mode = Mode::Settings;
            app.selected_setting = 0;
        }
        _ => app.mode = Mode::Normal,
    }

    false
}

fn sync_models_tui(app: &mut TuiApp) {
    if app.is_loading {
        return;
    }
    app.is_loading = true;
    app.loading_start = Some(Instant::now());

    app.messages.push(ChatMessage {
        status: Some("syncing".to_string()),
        ..ChatMessage::new(
            ChatRole::Assistant,
            "Syncing models from providers...".to_string(),
        )
    });

    let tx = app.async_tx.clone();
    let mut loaded_config = app.loaded_config.clone();
    let cwd = app.project_dir.clone();

    app.stream_task = Some(tokio::spawn(async move {
        let credential_store = CredentialStore::new(loaded_config.data_dir.clone());
        let catalog = navi_core::provider_catalog(&loaded_config.config);
        let mut updated_any = false;
        let mut synced_providers = Vec::new();
        let mut failed_providers = Vec::new();

        for provider_config in catalog {
            if let Some(api_key) =
                resolve_provider_api_key(&credential_store, &provider_config, &provider_config.id)
            {
                match OpenAiProvider::from_provider_config_with_key(&provider_config, api_key) {
                    Ok(provider) => match provider.list_models().await {
                        Ok(models) => {
                            if !models.is_empty() {
                                loaded_config
                                    .config
                                    .update_provider_models(&provider_config.id, &models);
                                updated_any = true;
                                synced_providers.push(provider_config.id.clone());
                            }
                        }
                        Err(e) => {
                            failed_providers.push(format!("{}: {}", provider_config.id, e));
                        }
                    },
                    Err(e) => {
                        failed_providers.push(format!("{}: {}", provider_config.id, e));
                    }
                }
            }
        }

        let message = if updated_any {
            let save_result = if let Some(_) = &loaded_config.project_config_path {
                navi_core::save_project_config(&cwd, &loaded_config.config)
            } else if let Some(global_path) = &loaded_config.global_config_path {
                navi_core::save_global_config(global_path, &loaded_config.config)
            } else {
                Err(anyhow::anyhow!("no config file path found to save"))
            };

            match save_result {
                Ok(path) => {
                    let synced_str = synced_providers.join(", ");
                    let mut msg = format!(
                        "Successfully synced models for: {synced_str}.\nSaved configuration to {}",
                        path.display()
                    );
                    if !failed_providers.is_empty() {
                        msg.push_str(&format!(
                            "\nFailed to sync some providers:\n- {}",
                            failed_providers.join("\n- ")
                        ));
                    }
                    msg
                }
                Err(e) => {
                    format!("Synced models, but failed to save configuration: {}", e)
                }
            }
        } else {
            if failed_providers.is_empty() {
                "No providers had credentials configured for model synchronization.".to_string()
            } else {
                format!(
                    "Failed to sync models:\n- {}",
                    failed_providers.join("\n- ")
                )
            }
        };

        let _ = tx.send(AsyncEvent::SyncCompleted {
            loaded_config,
            message,
        });
    }));
}

fn sync_provider_tui(app: &mut TuiApp, provider_id: &str) {
    if app.is_loading {
        return;
    }
    app.is_loading = true;
    app.loading_start = Some(Instant::now());

    app.messages.push(ChatMessage {
        status: Some("syncing".to_string()),
        ..ChatMessage::new(
            ChatRole::Assistant,
            format!("Syncing models for provider '{provider_id}'..."),
        )
    });

    let tx = app.async_tx.clone();
    let mut loaded_config = app.loaded_config.clone();
    let cwd = app.project_dir.clone();
    let target_provider = provider_id.to_string();

    app.stream_task = Some(tokio::spawn(async move {
        let credential_store = CredentialStore::new(loaded_config.data_dir.clone());
        let catalog = navi_core::provider_catalog(&loaded_config.config);

        let message = if let Some(provider_config) = catalog
            .iter()
            .find(|pc| canonical_provider_id(&pc.id) == canonical_provider_id(&target_provider))
        {
            if let Some(api_key) =
                resolve_provider_api_key(&credential_store, provider_config, &target_provider)
            {
                match OpenAiProvider::from_provider_config_with_key(provider_config, api_key) {
                    Ok(provider) => match provider.list_models().await {
                        Ok(models) if !models.is_empty() => {
                            loaded_config
                                .config
                                .update_provider_models(&target_provider, &models);

                            let save_result = if loaded_config.project_config_path.is_some() {
                                navi_core::save_project_config(&cwd, &loaded_config.config)
                            } else if let Some(global_path) = &loaded_config.global_config_path {
                                navi_core::save_global_config(global_path, &loaded_config.config)
                            } else {
                                Err(anyhow::anyhow!("no config file path found to save"))
                            };

                            match save_result {
                                Ok(path) => format!(
                                    "Synced {} models for '{target_provider}'.\nSaved to {}",
                                    models.len(),
                                    path.display()
                                ),
                                Err(e) => format!(
                                    "Synced models for '{target_provider}', but failed to save: {e}"
                                ),
                            }
                        }
                        Ok(_) => {
                            format!("No models returned by provider '{target_provider}'.")
                        }
                        Err(e) => format!("Failed to sync '{target_provider}': {e}"),
                    },
                    Err(e) => format!("Failed to initialize provider '{target_provider}': {e}"),
                }
            } else {
                format!(
                    "No API key configured for provider '{target_provider}'. Set it via ctrl+m."
                )
            }
        } else {
            format!("Provider '{target_provider}' not found in the catalog.")
        };

        let _ = tx.send(AsyncEvent::SyncCompleted {
            loaded_config,
            message,
        });
    }));
}

// ─── input editing helpers ─────────────────────────────────────────────────────
fn insert_input_char(app: &mut TuiApp, ch: char) {
    clamp_input_cursor(app);
    app.input.insert(app.input_cursor, ch);
    app.input_cursor += ch.len_utf8();
}

fn delete_input_previous_char(app: &mut TuiApp) {
    clamp_input_cursor(app);
    let Some(previous) = previous_char_boundary(&app.input, app.input_cursor) else {
        return;
    };
    app.input.drain(previous..app.input_cursor);
    app.input_cursor = previous;
}

fn delete_input_next_char(app: &mut TuiApp) {
    clamp_input_cursor(app);
    let Some(next) = next_char_boundary(&app.input, app.input_cursor) else {
        return;
    };
    app.input.drain(app.input_cursor..next);
}

fn move_input_previous_char(app: &mut TuiApp) {
    clamp_input_cursor(app);
    if let Some(previous) = previous_char_boundary(&app.input, app.input_cursor) {
        app.input_cursor = previous;
    }
}

fn move_input_next_char(app: &mut TuiApp) {
    clamp_input_cursor(app);
    if let Some(next) = next_char_boundary(&app.input, app.input_cursor) {
        app.input_cursor = next;
    }
}

fn move_input_previous_hump(app: &mut TuiApp) {
    clamp_input_cursor(app);
    app.input_cursor = previous_hump_boundary(&app.input, app.input_cursor);
}

fn move_input_next_hump(app: &mut TuiApp) {
    clamp_input_cursor(app);
    app.input_cursor = next_hump_boundary(&app.input, app.input_cursor);
}

fn move_input_previous_control_stop(app: &mut TuiApp) {
    clamp_input_cursor(app);
    app.input_cursor = previous_control_boundary(&app.input, app.input_cursor);
}

fn move_input_next_control_stop(app: &mut TuiApp) {
    clamp_input_cursor(app);
    app.input_cursor = next_control_boundary(&app.input, app.input_cursor);
}

fn delete_input_next_hump(app: &mut TuiApp) {
    clamp_input_cursor(app);
    let end = next_hump_boundary(&app.input, app.input_cursor);
    app.input.drain(app.input_cursor..end);
}

fn delete_input_previous_hump(app: &mut TuiApp) {
    clamp_input_cursor(app);
    let start = previous_hump_boundary(&app.input, app.input_cursor);
    app.input.drain(start..app.input_cursor);
    app.input_cursor = start;
}

fn delete_input_previous_space_word(app: &mut TuiApp) {
    clamp_input_cursor(app);
    let start = previous_space_word_boundary(&app.input, app.input_cursor);
    app.input.drain(start..app.input_cursor);
    app.input_cursor = start;
}

fn clamp_input_cursor(app: &mut TuiApp) {
    app.input_cursor = app.input_cursor.min(app.input.len());
    app.input_cursor = floor_char_boundary(&app.input, app.input_cursor);
}

// ─── text boundary helpers ─────────────────────────────────────────────────────
fn floor_char_boundary(value: &str, mut cursor: usize) -> usize {
    cursor = cursor.min(value.len());
    while !value.is_char_boundary(cursor) {
        cursor = cursor.saturating_sub(1);
    }
    cursor
}

fn previous_char_boundary(value: &str, cursor: usize) -> Option<usize> {
    value[..cursor]
        .char_indices()
        .last()
        .map(|(index, _)| index)
}

fn next_char_boundary(value: &str, cursor: usize) -> Option<usize> {
    value[cursor..]
        .char_indices()
        .nth(1)
        .map(|(index, _)| cursor + index)
        .or_else(|| (cursor < value.len()).then_some(value.len()))
}

fn previous_hump_boundary(value: &str, cursor: usize) -> usize {
    let chars = indexed_chars(value);
    let mut index = char_slot_at_byte(&chars, cursor);
    if index == 0 {
        return 0;
    }

    index -= 1;
    while index > 0 && is_separator(chars[index].1) {
        index -= 1;
    }
    while index > 0 && is_hump_continuation(&chars, index) {
        index -= 1;
    }

    chars.get(index).map(|(byte, _)| *byte).unwrap_or(0)
}

fn next_hump_boundary(value: &str, cursor: usize) -> usize {
    let chars = indexed_chars(value);
    let mut index = char_slot_at_byte(&chars, cursor);
    if index >= chars.len() {
        return value.len();
    }

    while index < chars.len() && is_separator(chars[index].1) {
        index += 1;
    }
    if index < chars.len() {
        index += 1;
    }
    while index < chars.len() && is_hump_continuation(&chars, index) {
        index += 1;
    }

    chars
        .get(index)
        .map(|(byte, _)| *byte)
        .unwrap_or(value.len())
}

fn previous_control_boundary(value: &str, cursor: usize) -> usize {
    let chars = indexed_chars(value);
    let mut index = char_slot_at_byte(&chars, cursor);
    if index == 0 {
        return 0;
    }

    index -= 1;
    if is_separator(chars[index].1) {
        return chars[index].0;
    }

    while index > 0 && is_hump_continuation(&chars, index) {
        index -= 1;
    }

    chars.get(index).map(|(byte, _)| *byte).unwrap_or(0)
}

fn next_control_boundary(value: &str, cursor: usize) -> usize {
    let chars = indexed_chars(value);
    let mut index = char_slot_at_byte(&chars, cursor);
    if index >= chars.len() {
        return value.len();
    }

    if is_separator(chars[index].1) {
        return next_char_boundary(value, cursor).unwrap_or(value.len());
    }

    index += 1;
    while index < chars.len() && is_hump_continuation(&chars, index) {
        index += 1;
    }

    chars
        .get(index)
        .map(|(byte, _)| *byte)
        .unwrap_or(value.len())
}

fn previous_space_word_boundary(value: &str, cursor: usize) -> usize {
    let chars = indexed_chars(value);
    let mut index = char_slot_at_byte(&chars, cursor);
    if index == 0 {
        return 0;
    }

    index -= 1;
    while index > 0 && chars[index].1.is_whitespace() {
        index -= 1;
    }
    while index > 0 && !chars[index - 1].1.is_whitespace() {
        index -= 1;
    }

    chars.get(index).map(|(byte, _)| *byte).unwrap_or(0)
}

fn indexed_chars(value: &str) -> Vec<(usize, char)> {
    value.char_indices().collect()
}

fn char_slot_at_byte(chars: &[(usize, char)], cursor: usize) -> usize {
    chars
        .iter()
        .position(|(byte, _)| *byte >= cursor)
        .unwrap_or(chars.len())
}

fn is_hump_continuation(chars: &[(usize, char)], index: usize) -> bool {
    let previous = chars[index - 1].1;
    let current = chars[index].1;
    let next = chars.get(index + 1).map(|(_, ch)| *ch);

    if is_separator(previous) || is_separator(current) {
        return false;
    }
    if previous.is_lowercase() && current.is_uppercase() {
        return false;
    }
    if previous.is_ascii_digit() != current.is_ascii_digit()
        && (previous.is_alphanumeric() || current.is_alphanumeric())
    {
        return false;
    }
    if previous.is_uppercase()
        && current.is_uppercase()
        && next.is_some_and(|next| next.is_lowercase())
    {
        return false;
    }

    true
}

fn is_separator(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '_' | '-' | '.' | '/' | '\\' | ':' | ';' | ',' | '(' | ')' | '[' | ']' | '{' | '}'
        )
}

// ─── rendering ─────────────────────────────────────────────────────────────────
fn render(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = frame.area();
    frame.render_widget(Block::new().style(Style::default().bg(BG)), area);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(6),    // chat area
            Constraint::Length(1), // breathing room between transcript and prompt
            Constraint::Length(7), // input area
        ])
        .split(area);

    render_chat_area(frame, app, vertical[0]);
    render_input(frame, app, vertical[2]);

    match app.mode {
        Mode::Commands => render_command_palette(frame, app, modal_rect(area, 68, 15)),
        Mode::Models => render_model_picker(frame, app, modal_rect(area, 72, 22)),
        Mode::ApiKeyEntry => render_api_key_entry(frame, app, modal_rect(area, 72, 11)),
        Mode::Thinking => render_thinking_picker(frame, app, modal_rect(area, 40, 10)),
        Mode::Sessions => render_sessions_picker(frame, app, modal_rect(area, 72, 16)),
        Mode::Settings => render_settings(frame, app, modal_rect(area, 50, 10)),
        Mode::Providers => render_provider_settings(frame, app, modal_rect(area, 76, 20)),
        Mode::Debug => render_debug_modal(frame, app, modal_rect(area, 76, 18)),
        Mode::Help => render_help_modal(frame, modal_rect(area, 62, 16)),
        Mode::Normal => {}
    }

    if !app.pending_approvals.is_empty() {
        render_tool_approval(frame, app, modal_rect(area, 72, 12));
    }

    render_notification(frame, app, area);
}

fn render_notification(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let Some(notification) = visible_notification(app) else {
        return;
    };

    let message_width = notification
        .message
        .chars()
        .count()
        .max(notification.title.chars().count())
        .saturating_add(8);
    let available_width = area.width.saturating_sub(4).max(1);
    let width = (message_width.clamp(26, 68) as u16).min(available_width);
    let height = area.height.min(3).max(1);
    let x = area.x + area.width.saturating_sub(width + 2);
    let y = area.y
        + area
            .height
            .saturating_sub(9)
            .min(area.height.saturating_sub(height));
    let rect = Rect::new(x, y, width, height);
    let inner = rect.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    frame.render_widget(Clear, rect);
    frame.render_widget(
        Block::new()
            .title(Line::from(vec![Span::styled(
                format!(" {} ", notification.title),
                Style::default().fg(PINK).add_modifier(Modifier::BOLD),
            )]))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(ACCENT))
            .style(Style::default().bg(PANEL)),
        rect,
    );
    frame.render_widget(
        Paragraph::new(notification.message.clone())
            .style(Style::default().fg(TEXT).bg(PANEL))
            .wrap(Wrap { trim: true }),
        inner,
    );
}

// ─── chat area ─────────────────────────────────────────────────────────────────
fn render_chat_area(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    if app.messages.is_empty() && !app.is_loading {
        let welcome = welcome_text(app, inner.width as usize);
        frame.render_widget(
            Paragraph::new(welcome)
                .style(Style::default().bg(BG))
                .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }

    let chat_width = inner.width as usize;
    ensure_chat_cache(app, chat_width);
    let cache = app.chat_render_cache.borrow();
    let rendered_lines = &cache.lines;

    // Apply scroll offset (from bottom)
    let visible_height = inner.height as usize;
    let total_lines = rendered_lines.len();
    let max_scroll = total_lines.saturating_sub(visible_height);
    let effective_scroll = app.scroll_offset.min(max_scroll);
    let start = total_lines
        .saturating_sub(visible_height)
        .saturating_sub(effective_scroll);
    let end = (start + visible_height).min(total_lines);

    let visible_lines: Vec<Line<'static>> = rendered_lines[start..end].to_vec();

    frame.render_widget(
        Paragraph::new(Text::from(visible_lines))
            .style(Style::default().bg(BG))
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn ensure_chat_cache(app: &TuiApp, chat_width: usize) {
    let signature = chat_render_signature(app);
    {
        let cache = app.chat_render_cache.borrow();
        if cache.width == chat_width
            && cache.full_tool_view == app.full_tool_view
            && cache.show_thinking == app.show_thinking
            && cache.signature == signature
        {
            return;
        }
    }

    let lines = build_chat_lines(app, chat_width);
    let mut cache = app.chat_render_cache.borrow_mut();
    cache.width = chat_width;
    cache.full_tool_view = app.full_tool_view;
    cache.show_thinking = app.show_thinking;
    cache.signature = signature;
    cache.lines = lines;
}

fn chat_render_signature(app: &TuiApp) -> String {
    let mut signature = String::with_capacity(app.messages.len() * 48);
    signature.push_str(if app.full_tool_view {
        "full|"
    } else {
        "compact|"
    });
    signature.push_str(if app.show_thinking { "think|" } else { "hide|" });
    for msg in &app.messages {
        signature.push(match msg.role {
            ChatRole::User => 'u',
            ChatRole::Assistant => 'a',
        });
        signature.push(':');
        signature.push_str(&msg.content.len().to_string());
        signature.push(':');
        signature.push_str(&msg.thinking_content.len().to_string());
        signature.push(':');
        signature.push_str(msg.status.as_deref().unwrap_or_default());
        signature.push(':');
        signature.push_str(msg.usage_label.as_deref().unwrap_or_default());
        signature.push(':');
        signature.push_str(&msg.elapsed_ms.unwrap_or_default().to_string());
        signature.push(':');
        signature.push_str(msg.model_label.as_deref().unwrap_or_default());
        signature.push(':');
        signature.push_str(msg.provider_label.as_deref().unwrap_or_default());
        if msg.is_compact_summary {
            signature.push_str(":compact");
        }
        if let Some(result) = &msg.tool_result {
            signature.push(':');
            signature.push_str(if result.ok { "ok" } else { "err" });
        }
        signature.push('|');
    }
    signature
}

fn build_chat_lines(app: &TuiApp, chat_width: usize) -> Vec<Line<'static>> {
    build_chat_lines_for_messages(
        app.messages.iter(),
        chat_width,
        app.full_tool_view,
        app.show_thinking,
    )
}

fn build_chat_lines_for_messages<'a>(
    messages: impl IntoIterator<Item = &'a ChatMessage>,
    chat_width: usize,
    full_tool_view: bool,
    show_thinking: bool,
) -> Vec<Line<'static>> {
    let mut rendered_lines: Vec<Line<'static>> = Vec::new();

    for msg in messages {
        if is_empty_tool_placeholder(msg) {
            continue;
        }
        if !rendered_lines.is_empty() {
            rendered_lines.push(Line::from(""));
        }

        match msg.role {
            ChatRole::User => {
                rendered_lines.extend(render_markdown_lines(
                    &msg.content,
                    chat_width.saturating_sub(4),
                    USER_ACCENT,
                    TEXT,
                    false,
                ));
            }
            ChatRole::Assistant => {
                if msg.is_compact_summary {
                    rendered_lines.push(Line::from(vec![
                        Span::styled(
                            " ◈ compacted ",
                            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            "─".repeat(chat_width.saturating_sub(14)),
                            Style::default().fg(GHOST),
                        ),
                    ]));
                }
                if let Some((invocation, result)) = tool_result_parts(msg) {
                    if full_tool_view {
                        rendered_lines.extend(render_markdown_lines(
                            &tool_full_content(invocation, result),
                            chat_width.saturating_sub(2),
                            TEXT,
                            TEXT,
                            false,
                        ));
                    } else {
                        rendered_lines.push(render_compact_tool_line(invocation, result));
                    }
                } else {
                    if show_thinking && !msg.thinking_content.is_empty() {
                        rendered_lines.extend(render_markdown_lines(
                            &msg.thinking_content,
                            chat_width.saturating_sub(4),
                            MUTED,
                            MUTED,
                            true,
                        ));
                        if !msg.content.is_empty() {
                            rendered_lines.push(Line::from(""));
                        }
                    }
                    rendered_lines.extend(render_markdown_lines(
                        &msg.content,
                        chat_width.saturating_sub(2),
                        TEXT,
                        TEXT,
                        false,
                    ));
                }

                if let (Some(model_label), Some(provider_label)) =
                    (&msg.model_label, &msg.provider_label)
                {
                    let elapsed = msg
                        .elapsed_ms
                        .map(|ms| {
                            if ms < 1000 {
                                format!("{ms}ms")
                            } else {
                                format!("{:.1}s", ms as f64 / 1000.0)
                            }
                        })
                        .unwrap_or_default();

                    let status = msg
                        .status
                        .as_ref()
                        .map(|status| format!(" • {status}"))
                        .unwrap_or_default();
                    let usage = msg
                        .usage_label
                        .as_ref()
                        .map(|usage| format!(" • {usage}"))
                        .unwrap_or_default();
                    let attr_text =
                        format!("◇ {model_label} via {provider_label} {elapsed}{status}{usage}");
                    let attr_len = attr_text.chars().count();
                    let dash_count = chat_width.saturating_sub(attr_len + 2);
                    let dashes: String = std::iter::repeat('─').take(dash_count).collect();

                    rendered_lines.push(Line::from(vec![
                        Span::styled(format!(" {attr_text} "), Style::default().fg(MUTED)),
                        Span::styled(dashes, Style::default().fg(GHOST)),
                    ]));
                }
            }
        }
    }
    rendered_lines
}

fn is_empty_tool_placeholder(message: &ChatMessage) -> bool {
    message.role == ChatRole::Assistant
        && message.content.trim().is_empty()
        && message.thinking_content.trim().is_empty()
        && message.status.as_deref().is_some_and(|status| {
            status.starts_with("tool:")
                || status.starts_with("approval:")
                || status == "thinking"
                || status == "receiving"
        })
}

fn tool_result_parts(message: &ChatMessage) -> Option<(&ToolInvocation, &ToolResult)> {
    match (&message.tool_invocation, &message.tool_result) {
        (Some(invocation), Some(result)) => Some((invocation, result)),
        _ => None,
    }
}

fn render_compact_tool_line(invocation: &ToolInvocation, result: &ToolResult) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "● ",
            Style::default().fg(if result.ok { Color::Green } else { Color::Red }),
        ),
        Span::styled(
            tool_compact_text(invocation, result),
            Style::default().fg(TEXT),
        ),
    ])
}

fn render_markdown_lines(
    text: &str,
    max_width: usize,
    marker_color: Color,
    text_color: Color,
    italic: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code = false;
    let mut language = String::new();
    let show_marker = marker_color != text_color || italic;

    let raw_lines = text.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < raw_lines.len() {
        let raw_line = raw_lines[index];
        let trimmed = raw_line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            in_code = !in_code;
            language = if in_code {
                rest.split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string()
            } else {
                String::new()
            };
            lines.push(markdown_boundary_line(
                if in_code { rest.trim() } else { "" },
                show_marker,
                marker_color,
            ));
            index += 1;
            continue;
        }

        if in_code {
            lines.push(code_line(raw_line, &language, show_marker, marker_color));
            index += 1;
            continue;
        }

        if is_table_line(trimmed) {
            let mut table_rows = Vec::new();
            while index < raw_lines.len() && is_table_line(raw_lines[index].trim_start()) {
                let table_line = raw_lines[index].trim_start();
                if !is_table_separator(table_line) {
                    table_rows.push(table_line.to_string());
                }
                index += 1;
            }
            lines.extend(table_block_lines(&table_rows, show_marker, marker_color));
            continue;
        }

        let wrapped = wrap_text(raw_line, max_width);
        for line in wrapped {
            lines.push(text_line(
                line,
                show_marker,
                marker_color,
                text_color,
                italic,
            ));
        }
        index += 1;
    }

    if text.is_empty() {
        lines.push(text_line(
            String::new(),
            show_marker,
            marker_color,
            text_color,
            italic,
        ));
    }

    lines
}

fn text_line(
    text: String,
    show_marker: bool,
    marker_color: Color,
    text_color: Color,
    italic: bool,
) -> Line<'static> {
    let mut spans = marker_spans(show_marker, marker_color);
    if !italic {
        if let Some(markdown_line) = markdown_prose_line(&text, text_color) {
            spans.extend(markdown_line);
            return Line::from(spans);
        }
    }

    let mut style = Style::default().fg(text_color);
    if italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    spans.push(Span::styled(text, style));
    Line::from(spans)
}

fn markdown_prose_line(text: &str, fallback: Color) -> Option<Vec<Span<'static>>> {
    let trimmed = text.trim_start();
    let indent = text.len().saturating_sub(trimmed.len());
    let mut spans = Vec::new();
    if indent > 0 {
        spans.push(Span::styled(
            " ".repeat(indent),
            Style::default().fg(fallback),
        ));
    }

    let heading = trimmed.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&heading) && trimmed.chars().nth(heading) == Some(' ') {
        let prefix = match heading {
            1 => "█ ",
            2 => "▣ ",
            3 => "◆ ",
            _ => "◇ ",
        };
        spans.push(Span::styled(
            prefix,
            Style::default().fg(PINK).add_modifier(Modifier::BOLD),
        ));
        spans.extend(
            inline_text_spans(&trimmed[heading + 1..], TEXT)
                .into_iter()
                .map(|mut span| {
                    span.style = span.style.add_modifier(Modifier::BOLD);
                    span
                }),
        );
        return Some(spans);
    }

    if let Some(rest) = trimmed.strip_prefix("> ") {
        spans.push(Span::styled(
            "▌ ",
            Style::default().fg(PINK).add_modifier(Modifier::BOLD),
        ));
        spans.extend(inline_text_spans(rest, MUTED));
        return Some(spans);
    }

    if trimmed.starts_with('|') && trimmed.ends_with('|') {
        spans.extend(table_row_spans(&table_cells(trimmed), &[]));
        return Some(spans);
    }

    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        spans.push(Span::styled(
            "• ",
            Style::default().fg(PINK).add_modifier(Modifier::BOLD),
        ));
        spans.extend(inline_text_spans(&trimmed[2..], fallback));
        return Some(spans);
    }

    if let Some((marker, rest)) = ordered_list_marker(trimmed) {
        spans.push(Span::styled(
            marker,
            Style::default().fg(PINK).add_modifier(Modifier::BOLD),
        ));
        spans.extend(inline_text_spans(rest, fallback));
        return Some(spans);
    }

    let inline = inline_text_spans(trimmed, fallback);
    (inline.len() > 1).then(|| {
        spans.extend(inline);
        spans
    })
}

fn is_table_line(text: &str) -> bool {
    text.starts_with('|') && text.ends_with('|') && text.matches('|').count() >= 2
}

fn is_table_separator(text: &str) -> bool {
    is_table_line(text)
        && table_cells(text).iter().all(|cell| {
            let cell = cell.trim();
            !cell.is_empty() && cell.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
        })
}

fn table_cells(text: &str) -> Vec<String> {
    text.trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn table_block_lines(
    table_rows: &[String],
    show_marker: bool,
    marker_color: Color,
) -> Vec<Line<'static>> {
    let rows = table_rows
        .iter()
        .map(|row| table_cells(row))
        .collect::<Vec<_>>();
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0; column_count];
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(rendered_inline_width(cell));
        }
    }

    rows.iter()
        .enumerate()
        .map(|(row_index, cells)| {
            let mut spans = marker_spans(show_marker, marker_color);
            spans.extend(table_row_spans_with_header(cells, &widths, row_index == 0));
            Line::from(spans)
        })
        .collect()
}

fn table_row_spans(cells: &[String], widths: &[usize]) -> Vec<Span<'static>> {
    table_row_spans_with_header(cells, widths, false)
}

fn table_row_spans_with_header(
    cells: &[String],
    widths: &[usize],
    header: bool,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ", Style::default().fg(GHOST)));
        }
        let mut style = Style::default().fg(if header { CODE_TYPE } else { TEXT });
        if header {
            style = style.add_modifier(Modifier::BOLD);
        }
        spans.extend(inline_text_spans(
            cell,
            if header { CODE_TYPE } else { TEXT },
        ));
        let width = widths.get(index).copied().unwrap_or(0);
        let padding = width.saturating_sub(rendered_inline_width(cell));
        if padding > 0 {
            spans.push(Span::styled(" ".repeat(padding), style));
        }
    }
    spans
}

fn rendered_inline_width(text: &str) -> usize {
    inline_text_spans(text, TEXT)
        .iter()
        .map(|span| span.content.chars().count())
        .sum()
}

fn ordered_list_marker(text: &str) -> Option<(String, &str)> {
    let digit_len = text.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digit_len == 0 {
        return None;
    }

    let after_digits = text.get(digit_len..)?;
    let marker_len = if after_digits.starts_with(". ") || after_digits.starts_with(") ") {
        digit_len + 2
    } else {
        return None;
    };

    Some((text[..marker_len].to_string(), &text[marker_len..]))
}

fn inline_text_spans(text: &str, fallback: Color) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut plain = String::new();
    let mut index = 0;

    while index < text.len() {
        let rest = &text[index..];

        if let Some((marker_len, content, modifier, color)) = inline_delimited(rest) {
            push_plain_span(&mut spans, &mut plain, fallback);
            spans.push(Span::styled(
                content.to_string(),
                Style::default().fg(color).add_modifier(modifier),
            ));
            index += marker_len + content.len() + marker_len;
            continue;
        }

        if let Some((label, url, consumed)) = inline_link(rest) {
            push_plain_span(&mut spans, &mut plain, fallback);
            spans.push(Span::styled(
                label.to_string(),
                Style::default().fg(CODE_TYPE).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" ({url})"),
                Style::default().fg(MUTED),
            ));
            index += consumed;
            continue;
        }

        if let Some(ch) = rest.chars().next() {
            plain.push(ch);
            index += ch.len_utf8();
        } else {
            break;
        }
    }
    push_plain_span(&mut spans, &mut plain, fallback);
    spans
}

fn inline_delimited(rest: &str) -> Option<(usize, &str, Modifier, Color)> {
    let patterns = [
        ("`", Modifier::empty(), CODE_STRING),
        ("**", Modifier::BOLD, TEXT),
        ("__", Modifier::BOLD, TEXT),
        ("*", Modifier::ITALIC, MUTED),
        ("_", Modifier::ITALIC, MUTED),
    ];

    for (marker, modifier, color) in patterns {
        if let Some(after_start) = rest.strip_prefix(marker) {
            if let Some(end) = after_start.find(marker) {
                if end > 0 {
                    return Some((marker.len(), &after_start[..end], modifier, color));
                }
            }
        }
    }

    None
}

fn inline_link(rest: &str) -> Option<(&str, &str, usize)> {
    let after_open = rest.strip_prefix('[')?;
    let label_end = after_open.find("](")?;
    let label = &after_open[..label_end];
    let after_label = &after_open[label_end + 2..];
    let url_end = after_label.find(')')?;
    let url = &after_label[..url_end];
    if label.is_empty() || url.is_empty() {
        return None;
    }
    Some((label, url, 1 + label_end + 2 + url_end + 1))
}

fn push_plain_span(spans: &mut Vec<Span<'static>>, plain: &mut String, fallback: Color) {
    if plain.is_empty() {
        return;
    }
    spans.push(Span::styled(
        std::mem::take(plain),
        Style::default().fg(fallback),
    ));
}

fn markdown_boundary_line(language: &str, show_marker: bool, marker_color: Color) -> Line<'static> {
    let mut spans = marker_spans(show_marker, marker_color);
    let label = if language.is_empty() {
        "```".to_string()
    } else {
        format!("```{language}")
    };
    spans.push(Span::styled(label, Style::default().fg(GHOST)));
    Line::from(spans)
}

fn code_line(
    raw_line: &str,
    language: &str,
    show_marker: bool,
    marker_color: Color,
) -> Line<'static> {
    let mut spans = marker_spans(show_marker, marker_color);
    spans.extend(highlight_code_line(raw_line, language));
    Line::from(spans)
}

fn marker_spans(show_marker: bool, marker_color: Color) -> Vec<Span<'static>> {
    if show_marker {
        vec![Span::styled("│ ", Style::default().fg(marker_color))]
    } else {
        Vec::new()
    }
}

fn highlight_code_line(raw_line: &str, language: &str) -> Vec<Span<'static>> {
    let syntax_set = syntax_set();
    let syntax = syntax_set
        .find_syntax_by_token(language)
        .or_else(|| syntax_set.find_syntax_by_extension(language))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
    let mut highlighter = HighlightLines::new(syntax, syntax_theme());

    match highlighter.highlight_line(raw_line, syntax_set) {
        Ok(ranges) => ranges
            .into_iter()
            .map(|(style, text)| Span::styled(text.to_string(), syntect_style(style)))
            .collect(),
        Err(_) => vec![Span::styled(
            raw_line.to_string(),
            Style::default().fg(TEXT),
        )],
    }
}

fn syntect_style(style: SyntectStyle) -> Style {
    Style::default().fg(lain_code_color(style))
}

fn lain_code_color(style: SyntectStyle) -> Color {
    let color = style.foreground;
    if style
        .font_style
        .contains(syntect::highlighting::FontStyle::ITALIC)
        || (color.r < 118 && color.g < 118 && color.b < 118)
    {
        CODE_COMMENT
    } else if style
        .font_style
        .contains(syntect::highlighting::FontStyle::BOLD)
    {
        CODE_FUNC
    } else if color.r > 190 && color.b > 165 && color.g < 170 {
        CODE_KEYWORD
    } else if color.g > color.r.saturating_add(25) && color.g > color.b.saturating_add(5) {
        Color::Rgb(143, 232, 173)
    } else if color.b > color.r.saturating_add(25) && color.g > color.r.saturating_add(10) {
        CODE_TYPE
    } else if color.b > color.r.saturating_add(25) {
        CODE_NUMBER
    } else if color.r > 175 && color.g > 145 && color.b < 145 {
        CODE_CONST
    } else if color.r > 180 && color.b > 95 && color.g < 135 {
        CODE_OPERATOR
    } else if color.r < 175 && color.g < 175 && color.b < 175 {
        CODE_PUNCT
    } else if color.r > 200 && color.g > 200 && color.b > 200 {
        TEXT
    } else {
        Color::Rgb(
            boost_code_channel(color.r),
            boost_code_channel(color.g),
            boost_code_channel(color.b),
        )
    }
}

fn boost_code_channel(value: u8) -> u8 {
    value.max(96).saturating_add(22)
}

fn syntax_set() -> &'static SyntaxSet {
    static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn syntax_theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let themes = ThemeSet::load_defaults();
        themes
            .themes
            .get("base16-ocean.dark")
            .or_else(|| themes.themes.values().next())
            .cloned()
            .unwrap_or_default()
    })
}

fn welcome_text(app: &TuiApp, width: usize) -> Text<'static> {
    let mut lines = Vec::new();
    let logo_width = NAVI_COMPACT_LOGO
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let project = project_label();
    let model = app.loaded_config.config.model.name.clone();
    let provider = selected_provider_label(app).to_string();
    let thinking = app.thinking_level.label();
    let context = format!("{}%", app.total_tokens_estimate.min(100_000) / 1000);
    let status_width = [
        project.chars().count() + 10,
        model.chars().count() + provider.chars().count() + 9,
        thinking.len() + 13,
        context.len() + 9,
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    let content_width = logo_width + 6 + status_width;
    let left_pad = width.saturating_sub(content_width) / 2;

    lines.push(Line::from(""));

    for (index, logo_line) in NAVI_COMPACT_LOGO.iter().enumerate() {
        let color = match (app.tick / 5 + index as u64) % 4 {
            0 => PINK,
            1 => ACCENT,
            2 => Color::Rgb(236, 218, 255),
            _ => Color::Rgb(132, 20, 204),
        };
        let mut spans = vec![Span::styled(
            format!("{}{logo_line}", " ".repeat(left_pad)),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )];
        if let Some(status) =
            welcome_status_line(index, &project, &provider, &model, thinking, &context)
        {
            spans.push(Span::raw("      "));
            spans.extend(status);
        }
        lines.push(Line::from(spans));
    }

    Text::from(lines)
}

fn welcome_status_line(
    index: usize,
    project: &str,
    provider: &str,
    model: &str,
    thinking: &str,
    context: &str,
) -> Option<Vec<Span<'static>>> {
    match index {
        0 => Some(vec![
            Span::styled("project ", Style::default().fg(MUTED)),
            Span::styled(
                project.to_string(),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]),
        1 => Some(vec![
            Span::styled("model   ", Style::default().fg(MUTED)),
            Span::styled(
                model.to_string(),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]),
        2 => Some(vec![
            Span::styled("via     ", Style::default().fg(MUTED)),
            Span::styled(
                provider.to_string(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        ]),
        3 => Some(vec![
            Span::styled("thinking ", Style::default().fg(MUTED)),
            Span::styled(thinking.to_string(), Style::default().fg(TEXT)),
        ]),
        4 => Some(vec![
            Span::styled("context ", Style::default().fg(MUTED)),
            Span::styled(context.to_string(), Style::default().fg(TEXT)),
        ]),
        _ => None,
    }
}

fn project_label() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "~".to_string())
}

fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    let max_width = max_width.max(10);
    let mut lines = Vec::new();

    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }

        let mut current_line = String::new();
        for word in paragraph.split_whitespace() {
            if current_line.is_empty() {
                current_line = word.to_string();
            } else if current_line.chars().count() + 1 + word.chars().count() <= max_width {
                current_line.push(' ');
                current_line.push_str(word);
            } else {
                lines.push(current_line);
                current_line = word.to_string();
            }
        }
        if !current_line.is_empty() {
            lines.push(current_line);
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

// ─── input ─────────────────────────────────────────────────────────────────────
fn render_input(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    let input_lines = visible_input_lines(input_lines(app), rows[0].height as usize);
    frame.render_widget(
        Paragraph::new(Text::from(input_lines))
            .style(Style::default().bg(BG))
            .wrap(Wrap { trim: false })
            .block(Block::new().borders(Borders::NONE)),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(shortcut_tips(app, rows[1].width as usize)).style(Style::default().bg(BG)),
        rows[1],
    );
}

fn visible_input_lines(lines: Vec<Line<'_>>, height: usize) -> Vec<Line<'_>> {
    let height = height.max(1);
    let start = lines.len().saturating_sub(height);
    lines.into_iter().skip(start).collect()
}

fn input_lines(app: &TuiApp) -> Vec<Line<'_>> {
    let prompt = "> ";
    let continuation = " ".repeat(prompt.chars().count());
    let mut spans = vec![Span::styled(
        prompt,
        Style::default().fg(SIGNAL).add_modifier(Modifier::BOLD),
    )];

    if app.input.is_empty() {
        spans.push(cursor_span(" "));
        let placeholder = if app.is_loading {
            " Thinking..."
        } else {
            " Ready!"
        };
        spans.push(Span::styled(placeholder, Style::default().fg(MUTED)));
        return vec![Line::from(spans)];
    }

    let cursor = app.input_cursor.min(app.input.len());
    let cursor = floor_char_boundary(&app.input, cursor);
    let (before, rest) = app.input.split_at(cursor);
    spans.push(Span::styled(before, Style::default().fg(TEXT)));

    if rest.is_empty() {
        spans.push(cursor_span(" "));
    } else {
        let next = next_char_boundary(&app.input, cursor).unwrap_or(app.input.len());
        let (cursor_text, after) = app.input[cursor..].split_at(next - cursor);
        spans.push(cursor_span(cursor_text));
        spans.push(Span::styled(after, Style::default().fg(TEXT)));
    }

    split_input_spans(spans, &continuation)
}

fn split_input_spans<'a>(spans: Vec<Span<'a>>, continuation: &str) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    let mut current = Vec::new();

    for span in spans {
        let content = span.content.clone();
        let style = span.style;
        let mut parts = content.split('\n').peekable();
        while let Some(part) = parts.next() {
            if !part.is_empty() {
                current.push(Span::styled(part.to_string(), style));
            }
            if parts.peek().is_some() {
                lines.push(Line::from(current));
                current = Vec::new();
                current.push(Span::raw(continuation.to_string()));
            }
        }
    }

    if !current.is_empty() || lines.is_empty() {
        lines.push(Line::from(current));
    }

    lines
}

fn shortcut_tips(app: &TuiApp, width: usize) -> Line<'static> {
    let items = [
        ("?", "for shortcuts", TEXT),
        ("ctrl+p", "commands", TEXT),
        ("ctrl+c", "quit", TEXT),
    ];

    let mut spans = vec![Span::styled(" ", Style::default().fg(MUTED))];
    let mut used = 3usize;

    for (index, (key, label, key_color)) in items.iter().enumerate() {
        let item_width = key.chars().count()
            + if label.is_empty() {
                0
            } else {
                1 + label.chars().count()
            };
        let separator_width = if index == 0 { 0 } else { 5 };
        if used + separator_width + item_width > width {
            break;
        }
        if index > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(GHOST)));
            used += separator_width;
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::default().fg(*key_color).add_modifier(Modifier::BOLD),
        ));
        used += key.chars().count();
        if !label.is_empty() {
            spans.push(Span::styled(
                format!(" {label}"),
                Style::default().fg(MUTED),
            ));
            used += 1 + label.chars().count();
        }
    }

    let compact_state = &app.compact_state;
    let pct = compact_state.context_percentage();
    let threshold = compact_state.threshold_level();
    let pct_label = format!(" {pct:.0}%");
    let pct_color = match threshold {
        navi_core::CompactThreshold::CircuitOpen => SIGNAL,
        navi_core::CompactThreshold::Error => SIGNAL,
        navi_core::CompactThreshold::Warning => ACCENT,
        navi_core::CompactThreshold::Normal => MUTED,
    };
    let threshold_label = match threshold {
        navi_core::CompactThreshold::CircuitOpen => " ⚠circuit",
        navi_core::CompactThreshold::Error => " ⚠compact",
        navi_core::CompactThreshold::Warning => " ~compact",
        navi_core::CompactThreshold::Normal => "",
    };
    let context_text = format!("ctx:{pct_label}{threshold_label}");
    let context_width = context_text.chars().count();
    if used + context_width + 2 < width {
        let padding = width.saturating_sub(used + context_width + 1);
        spans.push(Span::styled(
            " ".repeat(padding),
            Style::default().fg(MUTED),
        ));
        spans.push(Span::styled(format!("ctx:"), Style::default().fg(MUTED)));
        spans.push(Span::styled(pct_label, Style::default().fg(pct_color)));
        if !threshold_label.is_empty() {
            spans.push(Span::styled(
                threshold_label.to_string(),
                Style::default().fg(pct_color),
            ));
        }
    }

    Line::from(spans)
}

fn cursor_span(value: &str) -> Span<'_> {
    Span::styled(
        value,
        Style::default()
            .fg(BG)
            .bg(SIGNAL)
            .add_modifier(Modifier::BOLD),
    )
}

// ─── api key entry modal ───────────────────────────────────────────────────────
fn render_api_key_entry(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = Block::new()
        .title(Line::from(vec![Span::styled(
            " Enter API Key ",
            Style::default().fg(SIGNAL),
        )]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().fg(TEXT).bg(PANEL));
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // provider name
            Constraint::Length(1), // env var hint
            Constraint::Length(1), // blank
            Constraint::Length(1), // label
            Constraint::Length(1), // key input
            Constraint::Length(1), // blank
            Constraint::Length(1), // status
            Constraint::Length(1), // help
        ])
        .split(inner);

    let provider_id = selected_or_pending_provider_id(app);
    let provider_label = selected_or_pending_provider_label(app);
    let env_var = current_provider_env_var(app);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Provider:  ", Style::default().fg(MUTED)),
            Span::styled(
                format!("{provider_label} ({provider_id})"),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]))
        .style(Style::default().bg(PANEL)),
        rows[0],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Env var:   ", Style::default().fg(MUTED)),
            Span::styled(env_var, Style::default().fg(GHOST)),
        ]))
        .style(Style::default().bg(PANEL)),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Paste your API key:",
            Style::default().fg(MUTED),
        )))
        .style(Style::default().bg(PANEL)),
        rows[3],
    );

    // Key input field with cursor
    let key_display = api_key_input_line(app, rows[4].width as usize);
    frame.render_widget(
        Paragraph::new(key_display).style(Style::default().bg(PANEL)),
        rows[4],
    );

    // Status
    let status = if provider_has_api_key(app, &provider_id) {
        Line::from(Span::styled(
            "● Provider connected",
            Style::default().fg(SIGNAL),
        ))
    } else if app
        .pending_model_selection
        .and_then(|index| app.models.get(index))
        .is_some_and(|model| model_can_run_publicly(&model.provider_id, &model.name))
    {
        Line::from(Span::styled(
            "● Free model access available without key",
            Style::default().fg(SIGNAL),
        ))
    } else {
        Line::from(Span::styled(
            "○ No key configured",
            Style::default().fg(RED),
        ))
    };
    frame.render_widget(
        Paragraph::new(status).style(Style::default().bg(PANEL)),
        rows[6],
    );

    frame.render_widget(
        Paragraph::new("enter save  •  esc cancel").style(Style::default().fg(MUTED).bg(PANEL)),
        rows[7],
    );
}

// ─── thinking picker ───────────────────────────────────────────────────────────
fn render_tool_approval(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let Some(req) = app.pending_approvals.first() else {
        return;
    };
    let default_inv;
    let invocation = if let Some(inv) = app.tool_invocations.get(&req.id) {
        inv
    } else {
        default_inv = ToolInvocation {
            id: req.id.clone(),
            tool_name: "unknown".to_string(),
            input: serde_json::json!({ "summary": req.summary }),
        };
        &default_inv
    };
    frame.render_widget(Clear, area);
    let block = modal_block("Tool Approval");
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    frame.render_widget(block, area);

    let input = serde_json::to_string_pretty(&invocation.input)
        .unwrap_or_else(|_| invocation.input.to_string());
    let text = Text::from(vec![
        Line::from(vec![
            Span::styled("Tool: ", Style::default().fg(MUTED)),
            Span::styled(
                invocation.tool_name.clone(),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            truncate_display(&input, 420),
            Style::default().fg(SIGNAL),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("y", Style::default().fg(TEXT).add_modifier(Modifier::BOLD)),
            Span::styled(" approve  •  ", Style::default().fg(MUTED)),
            Span::styled("n", Style::default().fg(TEXT).add_modifier(Modifier::BOLD)),
            Span::styled(" deny  •  ", Style::default().fg(MUTED)),
            Span::styled(
                "ctrl+g",
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" yolo mode", Style::default().fg(MUTED)),
        ]),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(PANEL)),
        inner,
    );
}

fn render_thinking_picker(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Thinking Mode");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(1)])
        .split(inner);

    let items = THINKING_OPTIONS
        .iter()
        .enumerate()
        .map(|(index, level)| {
            let selected = index == app.selected_thinking;
            let current = *level == app.thinking_level;
            let style = if selected {
                Style::default()
                    .fg(Color::White)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT).bg(PANEL)
            };

            let marker = if current { "● " } else { "  " };
            ListItem::new(Span::styled(format!("{}{}", marker, level.label()), style)).style(style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(List::new(items).style(Style::default().bg(PANEL)), rows[0]);
    frame.render_widget(
        Paragraph::new("↑↓ choose  •  enter confirm  •  esc cancel")
            .style(Style::default().fg(MUTED).bg(PANEL)),
        rows[1],
    );
}

fn render_settings(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Settings");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(1)])
        .split(inner);

    let settings_list = [
        ("Show Reasoning", Some(app.show_thinking)),
        ("Verbose Tool Output", Some(app.full_tool_view)),
        ("Provider Accounts", None),
    ];

    let items = settings_list
        .iter()
        .enumerate()
        .map(|(index, (label, val))| {
            let selected = index == app.selected_setting;
            let style = if selected {
                Style::default()
                    .fg(Color::White)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT).bg(PANEL)
            };

            let prefix = match val {
                Some(true) => "[x] ",
                Some(false) => "[ ] ",
                None => "› ",
            };
            ListItem::new(Span::styled(format!("{}{}", prefix, label), style)).style(style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(List::new(items).style(Style::default().bg(PANEL)), rows[0]);
    frame.render_widget(
        Paragraph::new("↑↓ choose  •  enter configure/toggle  •  esc close")
            .style(Style::default().fg(MUTED).bg(PANEL)),
        rows[1],
    );
}

fn render_provider_settings(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    frame.render_widget(modal_block("Provider Accounts"), area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(10),
            Constraint::Length(2),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new("Configure API keys or OAuth sign-in for supported providers.")
            .style(Style::default().fg(MUTED).bg(PANEL)),
        rows[0],
    );

    let providers = navi_core::provider_catalog(&app.loaded_config.config);
    let height = rows[1].height as usize;
    let start = app.provider_settings_scroll.min(providers.len());
    let end = (start + height).min(providers.len());
    let items = providers[start..end]
        .iter()
        .enumerate()
        .map(|(offset, provider)| {
            let index = start + offset;
            let selected = index == app.selected_provider_setting;
            let status = provider_auth_status(app, provider);
            let oauth = if provider_supports_oauth(&provider.id) {
                "OAuth"
            } else {
                "API key"
            };
            let line = format!(
                "{:<18} {:<12} {:<10} {}",
                provider.label, status.label, oauth, provider.description
            );
            let style = if selected {
                Style::default()
                    .fg(Color::White)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else if status.configured {
                Style::default().fg(SIGNAL).bg(PANEL)
            } else {
                Style::default().fg(MUTED).bg(PANEL)
            };
            ListItem::new(Span::styled(line, style)).style(style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(List::new(items).style(Style::default().bg(PANEL)), rows[1]);

    frame.render_widget(
        Paragraph::new("enter/k API key  •  o OAuth  •  r sync models  •  esc settings")
            .style(Style::default().fg(MUTED).bg(PANEL)),
        rows[2],
    );
}

fn render_debug_modal(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Debug");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(12), Constraint::Length(1)])
        .split(inner);

    let active_state = if app.stream_task.is_some() {
        "streaming"
    } else if app.tool_task.is_some() {
        "tool"
    } else if app.is_loading {
        "loading"
    } else {
        "idle"
    };
    let provider = selected_provider_label(app);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Log file: ", Style::default().fg(MUTED)),
            Span::styled(
                app.log_path.display().to_string(),
                Style::default().fg(TEXT),
            ),
        ]),
        Line::from(vec![
            Span::styled("Session:  ", Style::default().fg(MUTED)),
            Span::styled(app.session_id.0.clone(), Style::default().fg(TEXT)),
        ]),
        Line::from(vec![
            Span::styled("Project:  ", Style::default().fg(MUTED)),
            Span::styled(
                app.project_dir.display().to_string(),
                Style::default().fg(TEXT),
            ),
        ]),
        Line::from(vec![
            Span::styled("Model:    ", Style::default().fg(MUTED)),
            Span::styled(
                format!("{} via {}", app.loaded_config.config.model.name, provider),
                Style::default().fg(TEXT),
            ),
        ]),
        Line::from(vec![
            Span::styled("API key:  ", Style::default().fg(MUTED)),
            Span::styled(
                current_provider_credential_status(app),
                Style::default().fg(ACCENT),
            ),
        ]),
        Line::from(vec![
            Span::styled("State:    ", Style::default().fg(MUTED)),
            Span::styled(active_state, Style::default().fg(ACCENT)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Recent diagnostics",
            Style::default().fg(PINK),
        )),
    ];
    if app.diagnostics.is_empty() {
        lines.push(Line::from(Span::styled("none", Style::default().fg(MUTED))));
    } else {
        for diagnostic in app.diagnostics.iter().rev().take(8) {
            lines.push(Line::from(Span::styled(
                diagnostic.clone(),
                Style::default().fg(TEXT),
            )));
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(TEXT).bg(PANEL))
            .wrap(Wrap { trim: false }),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new("esc close").style(Style::default().fg(MUTED).bg(PANEL)),
        rows[1],
    );
}

fn render_help_modal(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Shortcuts");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(12), Constraint::Length(1)])
        .split(inner);
    let shortcuts = [
        ("ctrl+p", "commands"),
        ("ctrl+m", "models"),
        ("ctrl+n", "new layer"),
        ("ctrl+s", "memory"),
        ("ctrl+o", "compact/full tool output"),
        ("ctrl+d", "debug"),
        ("ctrl+g", "toggle YOLO mode"),
        ("ctrl+enter", "send prompt"),
        ("enter", "new line"),
        ("ctrl+j", "new line"),
        ("/", "commands when input is empty"),
        ("?", "shortcuts"),
        ("esc", "cancel/close"),
    ];
    let lines = shortcuts
        .iter()
        .map(|(key, label)| {
            Line::from(vec![
                Span::styled(format!("{key:<12}"), Style::default().fg(SIGNAL)),
                Span::styled(*label, Style::default().fg(TEXT)),
            ])
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(TEXT).bg(PANEL))
            .wrap(Wrap { trim: false }),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new("enter/?/esc close").style(Style::default().fg(MUTED).bg(PANEL)),
        rows[1],
    );
}

// ─── sessions picker ───────────────────────────────────────────────────────────
fn render_sessions_picker(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Memory");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(1)])
        .split(inner);

    if app.saved_sessions.is_empty() {
        frame.render_widget(
            Paragraph::new(Text::from(vec![
                Line::from(""),
                Line::from(Span::styled(
                    "No saved sessions",
                    Style::default().fg(MUTED),
                )),
            ]))
            .style(Style::default().bg(PANEL)),
            rows[0],
        );
    } else {
        let items = app
            .saved_sessions
            .iter()
            .enumerate()
            .map(|(index, snapshot)| {
                let selected = index == app.selected_session;
                let style = if selected {
                    Style::default()
                        .fg(Color::White)
                        .bg(ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(TEXT).bg(PANEL)
                };

                let project = snapshot
                    .project
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| snapshot.project.to_string_lossy().to_string());
                let title = snapshot
                    .title
                    .as_deref()
                    .and_then(clean_session_title)
                    .unwrap_or_else(|| project.clone());
                let timestamp = format_session_timestamp(snapshot.updated_at);
                let event_count = snapshot.events.len();
                let label = format!("{timestamp}  {title}  ·  {project}  ·  {event_count} events");

                ListItem::new(Span::styled(label, style)).style(style)
            })
            .collect::<Vec<_>>();

        frame.render_widget(List::new(items).style(Style::default().bg(PANEL)), rows[0]);
    }

    frame.render_widget(
        Paragraph::new("↑↓ choose  •  enter load  •  del delete  •  esc cancel")
            .style(Style::default().fg(MUTED).bg(PANEL)),
        rows[1],
    );
}

fn api_key_input_line(app: &TuiApp, _max_width: usize) -> Line<'_> {
    let mut spans = vec![Span::styled("> ", Style::default().fg(SIGNAL))];

    if app.api_key_input.is_empty() {
        spans.push(Span::styled(" ", Style::default().fg(BG).bg(SIGNAL)));
        spans.push(Span::styled(" sk-...", Style::default().fg(GHOST)));
        return Line::from(spans);
    }

    let cursor = app.api_key_cursor.min(app.api_key_input.len());
    let (before, rest) = app.api_key_input.split_at(cursor);

    // Mask the middle of the key for display
    let display_before = mask_key_segment(before);
    spans.push(Span::styled(display_before, Style::default().fg(TEXT)));

    if rest.is_empty() {
        spans.push(Span::styled(" ", Style::default().fg(BG).bg(SIGNAL)));
    } else {
        let next =
            next_char_boundary(&app.api_key_input, cursor).unwrap_or(app.api_key_input.len());
        let (cursor_ch, after) = rest.split_at(next - cursor);
        spans.push(Span::styled(cursor_ch, Style::default().fg(BG).bg(SIGNAL)));
        let display_after = mask_key_segment(after);
        spans.push(Span::styled(display_after, Style::default().fg(TEXT)));
    }

    Line::from(spans)
}

fn mask_key_segment(segment: &str) -> String {
    // Show first 6 and last 4 chars, mask the rest
    let chars: Vec<char> = segment.chars().collect();
    if chars.len() <= 12 {
        return segment.to_string();
    }
    let mut result = String::new();
    for (i, ch) in chars.iter().enumerate() {
        if i < 6 || i >= chars.len() - 4 {
            result.push(*ch);
        } else {
            result.push('•');
        }
    }
    result
}

// ─── command palette ───────────────────────────────────────────────────────────
fn render_command_palette(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Commands");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(6),
            Constraint::Length(1),
        ])
        .split(inner);

    let filter = if app.command_filter.is_empty() {
        "type to filter"
    } else {
        app.command_filter.as_str()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(SIGNAL)),
            Span::styled(filter, Style::default().fg(MUTED)),
        ]))
        .style(Style::default().bg(PANEL)),
        rows[0],
    );

    let commands = filtered_commands(app);
    let command_width = rows[1].width as usize;
    let items = commands
        .iter()
        .enumerate()
        .map(|(index, command)| {
            let selected = index == app.selected_command;
            let style = if selected {
                Style::default()
                    .fg(Color::White)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT).bg(PANEL)
            };

            let shortcut = command.shortcut.unwrap_or("");
            ListItem::new(Span::styled(
                command_row(command.label, shortcut, command_width),
                style,
            ))
            .style(style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(List::new(items).style(Style::default().bg(PANEL)), rows[1]);
    frame.render_widget(
        Paragraph::new("tab/↑↓ choose  •  enter confirm  •  esc cancel")
            .style(Style::default().fg(MUTED).bg(PANEL)),
        rows[2],
    );
}

// ─── model picker ──────────────────────────────────────────────────────────────
fn render_model_picker(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    frame.render_widget(Clear, area);
    let block = modal_block("Switch Protocol");
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(inner);

    let filter_text = if app.model_filter.is_empty() {
        "search providers or models"
    } else {
        app.model_filter.as_str()
    };
    frame.render_widget(
        Paragraph::new(Text::from(vec![Line::from(vec![
            Span::styled("> ", Style::default().fg(SIGNAL)),
            Span::styled(
                filter_text,
                Style::default().fg(if app.model_filter.is_empty() {
                    MUTED
                } else {
                    TEXT
                }),
            ),
        ])]))
        .style(Style::default().bg(PANEL)),
        rows[0],
    );

    let list_rows = build_model_rows(app);
    let list_area = rows[1];
    let row_width = list_area.width as usize;

    let selected_row = selected_model_in_rows(&list_rows, app.selected_model).unwrap_or(0);
    let mut list_state = ListState::default()
        .with_offset(app.model_scroll)
        .with_selected(Some(selected_row));

    let items = list_rows
        .iter()
        .map(|row| match row {
            ListRow::Header { label, .. } => {
                let header_style = Style::default()
                    .fg(TEXT)
                    .bg(PANEL)
                    .add_modifier(Modifier::BOLD);
                let refresh_style = Style::default().fg(GHOST).bg(PANEL);

                let mut spans = vec![Span::styled(format!("  {}", label), header_style)];
                spans.push(Span::styled("  ↻ tab", refresh_style));
                ListItem::new(Line::from(spans)).style(header_style)
            }
            ListRow::Model { index } => {
                let model = &app.models[*index];
                let selected = *index == app.selected_model;
                let configured = model.name == app.loaded_config.config.model.name
                    && canonical_provider_id(&model.provider_id)
                        == canonical_provider_id(&app.loaded_config.config.model.provider);
                let style = if selected {
                    Style::default()
                        .fg(Color::White)
                        .bg(ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(TEXT).bg(PANEL)
                };

                ListItem::new(Span::styled(
                    model_row_simple(model.name.as_str(), configured, row_width),
                    style,
                ))
                .style(style)
            }
        })
        .collect::<Vec<_>>();

    frame.render_stateful_widget(
        List::new(items).style(Style::default().bg(PANEL)),
        list_area,
        &mut list_state,
    );
    frame.render_widget(
        Paragraph::new(
            "type search  •  ↑↓ choose  •  ctrl+e edit setup  •  tab refresh provider  •  ctrl+r refresh all  •  enter confirm  •  esc exit",
        )
        .style(Style::default().fg(MUTED).bg(PANEL)),
        rows[2],
    );
}

// ─── shared helpers ────────────────────────────────────────────────────────────
fn modal_block(title: &'static str) -> Block<'static> {
    Block::new()
        .title(Line::from(vec![
            Span::styled(format!(" {title} "), Style::default().fg(RED)),
            Span::styled("  online", Style::default().fg(MUTED)),
        ]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().fg(TEXT).bg(PANEL))
}

fn truncate_display(value: &str, max_chars: usize) -> String {
    let mut result = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        result.push_str("\n<truncated>");
    }
    result
}

fn command_row(label: &str, shortcut: &str, width: usize) -> String {
    let shortcut_width = 12usize.min(width.saturating_sub(1));
    let label_width = width.saturating_sub(shortcut_width + 1);
    format!(
        "{:<label_width$} {:<shortcut_width$}",
        fit_text(label, label_width),
        fit_text(shortcut, shortcut_width)
    )
}

fn model_row_simple(name: &str, configured: bool, width: usize) -> String {
    let marker_width = 3usize.min(width);
    let name_width = width.saturating_sub(marker_width + 4);
    let marker = if configured { "✓" } else { "" };

    format!(
        "    {:<name_width$} {:<marker_width$}",
        fit_text(name, name_width),
        marker
    )
}

fn selected_provider_label(app: &TuiApp) -> &str {
    let current_provider = canonical_provider_id(&app.loaded_config.config.model.provider);
    app.models
        .iter()
        .find(|model| canonical_provider_id(&model.provider_id) == current_provider)
        .map(|model| model.provider_label.as_str())
        .unwrap_or(app.loaded_config.config.model.provider.as_str())
}

fn fit_text(value: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let mut text = value.chars().take(width).collect::<String>();
    if value.chars().count() > width && width > 1 {
        text.pop();
        text.push('…');
    }
    text
}

fn modal_rect(area: Rect, max_width: u16, height: u16) -> Rect {
    let width = area.width.saturating_sub(8).min(max_width).max(40);
    let height = area.height.saturating_sub(4).min(height).max(10);
    centered_rect(area, width, height)
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(area.height.saturating_sub(height) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(area.width.saturating_sub(width) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(vertical[1])[1]
}

fn filtered_commands(app: &TuiApp) -> Vec<CommandItem> {
    let filter = app.command_filter.trim().to_lowercase();
    let commands = COMMANDS
        .iter()
        .copied()
        .filter(|command| filter.is_empty() || command.label.to_lowercase().contains(&filter))
        .collect::<Vec<_>>();

    if commands.is_empty() {
        COMMANDS.to_vec()
    } else {
        commands
    }
}

// ─── model picker rows (provider-grouped) ─────────────────────────────────────
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum ListRow {
    Header {
        label: String,
        description: String,
        provider_id: String,
    },
    Model {
        index: usize,
    },
}

fn build_model_rows(app: &TuiApp) -> Vec<ListRow> {
    let filter = app.model_filter.trim().to_lowercase();

    // Group visible models by provider label
    let mut rows = Vec::new();
    let mut current_provider: Option<&str> = None;

    for (index, model) in app.models.iter().enumerate() {
        if !filter.is_empty()
            && !model.name.to_lowercase().contains(&filter)
            && !model.provider_id.to_lowercase().contains(&filter)
            && !model.provider_label.to_lowercase().contains(&filter)
            && !model.provider_description.to_lowercase().contains(&filter)
        {
            continue;
        }
        if current_provider != Some(model.provider_label.as_str()) {
            current_provider = Some(model.provider_label.as_str());
            rows.push(ListRow::Header {
                label: model.provider_label.clone(),
                description: model.provider_description.clone(),
                provider_id: model.provider_id.clone(),
            });
        }
        rows.push(ListRow::Model { index });
    }

    rows
}

fn first_model_index(rows: &[ListRow]) -> Option<usize> {
    rows.iter().find_map(|row| match row {
        ListRow::Model { index } => Some(*index),
        ListRow::Header { .. } => None,
    })
}

fn selected_model_in_rows(rows: &[ListRow], selected_model: usize) -> Option<usize> {
    rows.iter().position(|row| match row {
        ListRow::Model { index } => *index == selected_model,
        ListRow::Header { .. } => false,
    })
}

fn next_model_index(app: &TuiApp, rows: &[ListRow]) -> usize {
    let Some(current) = selected_model_in_rows(rows, app.selected_model) else {
        // Current selection not in visible rows — jump to first model
        return rows
            .iter()
            .find_map(|row| match row {
                ListRow::Model { index } => Some(*index),
                _ => None,
            })
            .unwrap_or(app.selected_model);
    };

    // Find next model row after current
    rows.iter()
        .skip(current + 1)
        .find_map(|row| match row {
            ListRow::Model { index } => Some(*index),
            _ => None,
        })
        .unwrap_or(app.selected_model)
}

fn previous_model_index(app: &TuiApp, rows: &[ListRow]) -> usize {
    let Some(current) = selected_model_in_rows(rows, app.selected_model) else {
        return rows
            .iter()
            .find_map(|row| match row {
                ListRow::Model { index } => Some(*index),
                _ => None,
            })
            .unwrap_or(app.selected_model);
    };

    // Find previous model row before current
    rows.iter()
        .take(current)
        .rev()
        .find_map(|row| match row {
            ListRow::Model { index } => Some(*index),
            _ => None,
        })
        .unwrap_or(app.selected_model)
}

fn sync_scroll_to_selection(app: &mut TuiApp, rows: &[ListRow], visible_rows: u16) {
    let Some(selected_row) = selected_model_in_rows(rows, app.selected_model) else {
        return;
    };

    let visible_rows = usize::from(visible_rows).max(1);
    if selected_row < app.model_scroll {
        app.model_scroll = selected_row;
    } else {
        let bottom = app
            .model_scroll
            .saturating_add(visible_rows.saturating_sub(1));
        if selected_row >= bottom {
            app.model_scroll = selected_row.saturating_sub(visible_rows.saturating_sub(4));
        }
    }

    let max_scroll = rows.len().saturating_sub(visible_rows);
    app.model_scroll = app.model_scroll.min(max_scroll);
}

// ─── persistence ───────────────────────────────────────────────────────────────
fn load_saved_sessions(store: &SessionStore) -> Vec<SessionSnapshot> {
    store.list()
}

fn save_current_session(app: &mut TuiApp) {
    if app.messages.is_empty() && app.events.is_empty() {
        return;
    }
    let now = navi_core::session::current_unix_timestamp();
    let snapshot = SessionSnapshot {
        id: app.session_id.clone(),
        title: session_title_from_events(&app.events),
        project: app.project_dir.clone(),
        created_at: session_created_at(&app.session_id).unwrap_or(now),
        updated_at: now,
        events: app.events.clone(),
        memory: None,
    };
    if let Err(err) = app.session_store.save(&snapshot) {
        eprintln!("failed to save session: {err:#}");
    }
    if app.loaded_config.config.memory.session_memory_enabled {
        if let Some(summary) = &app.compact_state.summary {
            if let Err(err) = app.session_store.add_memory_entry(
                &app.project_dir,
                &app.session_id,
                summary.clone(),
            ) {
                tracing::warn!("failed to save project memory: {err:#}");
            }
        }
    }
    app.session_id = SessionStore::create_id();
    app.events.clear();
}

fn session_title_from_events(events: &[AgentEvent]) -> Option<String> {
    events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ModelOutput { text, .. } => title_from_model_text(text),
            _ => None,
        })
        .or_else(|| {
            events.iter().find_map(|event| match event {
                AgentEvent::UserTaskSubmitted { text } => title_from_user_text(text),
                _ => None,
            })
        })
}

fn title_from_model_text(text: &str) -> Option<String> {
    let heading = text.lines().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            Some(trimmed.trim_start_matches('#').trim())
        } else {
            None
        }
    });

    heading
        .and_then(clean_session_title)
        .or_else(|| text.lines().find_map(clean_session_title))
}

fn title_from_user_text(text: &str) -> Option<String> {
    clean_session_title(text)
}

fn clean_session_title(text: &str) -> Option<String> {
    let cleaned = text
        .trim()
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('\'')
        .trim_start_matches(['#', '-', '*', '>'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    if cleaned.is_empty() {
        return None;
    }

    Some(truncate_title(&cleaned, 72))
}

fn truncate_title(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut out = text
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
}

fn session_created_at(session_id: &SessionId) -> Option<u64> {
    session_id
        .0
        .strip_prefix("session-")?
        .parse::<u128>()
        .ok()
        .map(|millis| (millis / 1000) as u64)
}

fn format_session_timestamp(timestamp: u64) -> String {
    if timestamp == 0 {
        return "unknown time".to_string();
    }

    let (year, month, day, hour, minute) = unix_timestamp_parts(timestamp);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

fn unix_timestamp_parts(timestamp: u64) -> (i64, u32, u32, u32, u32) {
    let days = (timestamp / 86_400) as i64;
    let seconds = timestamp % 86_400;
    let hour = (seconds / 3_600) as u32;
    let minute = ((seconds % 3_600) / 60) as u32;
    let (year, month, day) = civil_from_days(days);
    (year, month, day, hour, minute)
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month as u32, day as u32)
}

fn save_preferences(app: &mut TuiApp) {
    app.loaded_config.config.model.name = app
        .models
        .get(app.selected_model)
        .map(|m| m.name.clone())
        .unwrap_or_else(|| app.loaded_config.config.model.name.clone());
    app.loaded_config.config.model.provider = app
        .models
        .get(app.selected_model)
        .map(|m| m.provider_id.clone())
        .unwrap_or_else(|| app.loaded_config.config.model.provider.clone());

    let global_path = app
        .loaded_config
        .global_config_path
        .as_ref()
        .expect("global config path");
    if let Err(err) = save_global_config(global_path, &app.loaded_config.config) {
        eprintln!("failed to save preferences: {err:#}");
    }
}

fn load_session(app: &mut TuiApp, snapshot: &SessionSnapshot) {
    app.messages.clear();
    reset_system_context(app);
    app.events.clear();
    app.total_tokens_estimate = 0;

    let mut tool_invocations = std::collections::HashMap::new();

    for event in &snapshot.events {
        match event {
            AgentEvent::UserTaskSubmitted { text } => {
                let word_count = text.split_whitespace().count();
                app.total_tokens_estimate += word_count * 4 / 3;
                app.messages
                    .push(ChatMessage::new(ChatRole::User, text.clone()));
                app.conversation_history
                    .push(ModelMessage::user(text.clone()));
            }
            AgentEvent::ModelOutput { text, thinking } => {
                let word_count = text.split_whitespace().count();
                app.total_tokens_estimate += word_count * 4 / 3;
                app.messages.push(ChatMessage {
                    thinking_content: thinking.clone().unwrap_or_default(),
                    ..ChatMessage::new(ChatRole::Assistant, text.clone())
                });
                app.conversation_history
                    .push(ModelMessage::assistant(text.clone()));
            }
            AgentEvent::ToolRequested(invocation) => {
                tool_invocations.insert(invocation.id.clone(), invocation.clone());
            }
            AgentEvent::ToolCompleted(result) => {
                if let Some(invocation) = tool_invocations.get(&result.invocation_id) {
                    app.messages.push(ChatMessage {
                        status: Some("tool result".to_string()),
                        tool_invocation: Some(invocation.clone()),
                        tool_result: Some(result.clone()),
                        ..ChatMessage::new(ChatRole::Assistant, String::new())
                    });
                }
            }
            _ => {}
        }
        app.events.push(event.clone());
    }

    app.scroll_offset = 0;
    app.input.clear();
    app.input_cursor = 0;
}

// ─── tests ─────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_app(input: &str) -> TuiApp {
        let mut app = TuiApp::new(
            LoadedConfig {
                config: navi_core::NaviConfig::default(),
                global_config_path: None,
                project_config_path: None,
                data_dir: PathBuf::from("/tmp/navi-test"),
            },
            PathBuf::from("/tmp/test-project"),
            None,
        );
        app.input = input.to_string();
        app.input_cursor = app.input.len();
        app.mode = Mode::Normal;
        app
    }

    fn app_with_missing_provider_key() -> TuiApp {
        let mut config = navi_core::NaviConfig::default();
        config.model.provider = "test-provider".to_string();
        config.model.name = "test-large".to_string();
        config.providers = vec![navi_core::ProviderConfig {
            id: "test-provider".to_string(),
            label: "Test Provider".to_string(),
            description: "test provider".to_string(),
            kind: navi_core::ProviderKind::OpenAiChatCompletions,
            api_key_env: "NAVI_TEST_MISSING_PROVIDER_KEY".to_string(),
            base_url: Some("https://example.com/v1".to_string()),
            models: vec![navi_core::ProviderModelConfig {
                name: "test-large".to_string(),
                task_size: navi_core::ModelTaskSize::Large,
                context_window_tokens: None,
            }],
            ..Default::default()
        }];

        TuiApp::new(
            LoadedConfig {
                config,
                global_config_path: None,
                project_config_path: None,
                data_dir: PathBuf::from("/tmp/navi-test-missing-key"),
            },
            PathBuf::from("/tmp/test-project"),
            None,
        )
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn camel_hump_next_boundary_splits_identifiers_and_words() {
        let value = "fooBar_bazQUX99 alpha";

        assert_eq!(next_hump_boundary(value, 0), 3);
        assert_eq!(next_hump_boundary(value, 3), 6);
        assert_eq!(next_hump_boundary(value, 7), 10);
        assert_eq!(next_hump_boundary(value, 10), 13);
        assert_eq!(next_hump_boundary(value, 13), 15);
        assert_eq!(next_hump_boundary(value, 15), value.len());
    }

    #[test]
    fn camel_hump_previous_boundary_splits_identifiers_and_words() {
        let value = "fooBar_bazQUX99 alpha";

        assert_eq!(previous_hump_boundary(value, value.len()), 16);
        assert_eq!(previous_hump_boundary(value, 16), 13);
        assert_eq!(previous_hump_boundary(value, 13), 10);
        assert_eq!(previous_hump_boundary(value, 10), 7);
        assert_eq!(previous_hump_boundary(value, 7), 3);
        assert_eq!(previous_hump_boundary(value, 3), 0);
    }

    #[test]
    fn char_boundary_helpers_handle_multibyte_input() {
        let value = "abçDef";
        let after_cedilla = "abç".len();

        assert_eq!(next_hump_boundary(value, 0), after_cedilla);
        assert_eq!(next_char_boundary(value, 2), Some(after_cedilla));
        assert_eq!(previous_char_boundary(value, after_cedilla), Some(2));
        assert_eq!(floor_char_boundary(value, after_cedilla - 1), 2);
    }

    #[test]
    fn control_backspace_aliases_delete_previous_camel_hump() {
        for code in [
            KeyCode::Backspace,
            KeyCode::Char('h'),
            KeyCode::Char('w'),
            KeyCode::Char('\u{7f}'),
        ] {
            let mut app = test_app("cargo test -p navi_tui");
            handle_normal_key(&mut app, code, KeyModifiers::CONTROL);
            assert_eq!(app.input, "cargo test -p navi_");
            assert_eq!(app.input_cursor, "cargo test -p navi_".len());

            handle_normal_key(&mut app, code, KeyModifiers::CONTROL);
            assert_eq!(app.input, "cargo test -p ");
            assert_eq!(app.input_cursor, "cargo test -p ".len());
        }
    }

    #[test]
    fn alt_backspace_deletes_until_previous_space_not_separator() {
        for code in [
            KeyCode::Backspace,
            KeyCode::Char('h'),
            KeyCode::Char('\u{7f}'),
        ] {
            let mut app = test_app("cargo test -p navi_tui");
            handle_normal_key(&mut app, code, KeyModifiers::ALT);
            assert_eq!(app.input, "cargo test -p ");
            assert_eq!(app.input_cursor, "cargo test -p ".len());

            handle_normal_key(&mut app, code, KeyModifiers::ALT);
            assert_eq!(app.input, "cargo test ");
            assert_eq!(app.input_cursor, "cargo test ".len());
        }
    }

    #[test]
    fn alt_comma_and_period_move_by_camel_humps() {
        let mut app = test_app("fooBar");

        handle_normal_key(&mut app, KeyCode::Char(','), KeyModifiers::ALT);
        assert_eq!(app.input_cursor, 3);

        handle_normal_key(&mut app, KeyCode::Char('.'), KeyModifiers::ALT);
        assert_eq!(app.input_cursor, 6);
    }

    #[test]
    fn control_arrows_stop_at_camel_humps_and_special_characters() {
        let mut app = test_app("fooBar_baz");
        app.input_cursor = 0;

        handle_normal_key(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(app.input_cursor, 3);

        handle_normal_key(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(app.input_cursor, 6);

        handle_normal_key(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(app.input_cursor, 7);

        handle_normal_key(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(app.input_cursor, 10);

        handle_normal_key(&mut app, KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(app.input_cursor, 7);

        handle_normal_key(&mut app, KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(app.input_cursor, 6);

        handle_normal_key(&mut app, KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(app.input_cursor, 3);

        handle_normal_key(&mut app, KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(app.input_cursor, 0);
    }

    #[test]
    fn wrap_text_handles_long_lines() {
        let text = "Hello world this is a very long line that should wrap properly";
        let lines = wrap_text(text, 20);
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(line.chars().count() <= 20);
        }
    }

    #[test]
    fn wrap_text_preserves_newlines() {
        let text = "Line one\nLine two\nLine three";
        let lines = wrap_text(text, 50);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "Line one");
        assert_eq!(lines[1], "Line two");
        assert_eq!(lines[2], "Line three");
    }

    #[test]
    fn markdown_renderer_wraps_plain_text() {
        let lines = render_markdown_lines("hello world from navi", 12, TEXT, TEXT, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(rendered, vec!["hello world", "from navi"]);
    }

    #[test]
    fn markdown_renderer_preserves_fenced_code_blocks() {
        let lines = render_markdown_lines(
            "before\n```rust\nfn main() {}\n```\nafter",
            80,
            TEXT,
            TEXT,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec!["before", "```rust", "fn main() {}", "```", "after"]
        );
    }

    #[test]
    fn markdown_renderer_handles_unclosed_fence() {
        let lines = render_markdown_lines("```unknown\n  value", 80, TEXT, TEXT, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(rendered, vec!["```unknown", "  value"]);
    }

    #[test]
    fn markdown_renderer_renders_inline_markup() {
        let lines = render_markdown_lines(
            "**NAVI** is `wired` and [documented](https://example.test)",
            120,
            TEXT,
            TEXT,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec!["NAVI is wired and documented (https://example.test)"]
        );
        assert!(
            lines[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn markdown_renderer_handles_lists_and_quotes() {
        let lines = render_markdown_lines(
            "1. **Architecture**\n> signal in prose",
            120,
            TEXT,
            TEXT,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(rendered, vec!["1. Architecture", "▌ signal in prose"]);
    }

    #[test]
    fn markdown_renderer_consumes_headings_and_table_pipes() {
        let lines = render_markdown_lines(
            "## Project Overview\n\n| Crate | Purpose |\n|---|---|\n| `navi-cli` | Entry binary |",
            120,
            TEXT,
            TEXT,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "▣ Project Overview",
                "",
                "Crate     Purpose     ",
                "navi-cli  Entry binary",
            ]
        );
        assert!(!rendered.iter().any(|line| line.contains("##")));
        assert!(!rendered.iter().skip(2).any(|line| line.contains('|')));
    }

    #[test]
    fn code_highlighting_uses_varied_colors() {
        let spans = highlight_code_line("fn main() { let value = \"x\"; }", "rust");
        let mut colors = Vec::new();
        for color in spans.iter().filter_map(|span| span.style.fg) {
            if !colors.contains(&color) {
                colors.push(color);
            }
        }

        assert!(colors.len() >= 3);
    }

    #[test]
    fn submit_without_provider_adds_error_message() {
        let mut app = test_app("hello");
        app.model_provider = None;
        submit_message(&mut app);
        assert_eq!(app.messages.len(), 2); // user + error
        assert_eq!(app.messages[0].role, ChatRole::User);
        assert_eq!(app.messages[1].role, ChatRole::Assistant);
        assert!(app.messages[1].content.contains("No API key"));
    }

    #[test]
    fn missing_api_key_does_not_open_prompt_on_startup() {
        let app = app_with_missing_provider_key();

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.model_provider.is_none());
        assert!(app.pending_model_selection.is_none());
    }

    #[test]
    fn selecting_model_without_provider_key_opens_key_prompt() {
        let mut app = app_with_missing_provider_key();
        app.mode = Mode::Models;

        handle_model_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(app.mode, Mode::ApiKeyEntry);
        assert_eq!(app.pending_model_selection, Some(app.selected_model));
    }

    #[test]
    fn model_picker_filters_by_model_and_provider_text() {
        let mut app = test_app("");
        open_model_picker(&mut app);

        app.model_filter = "gemini".to_string();
        let rows = build_model_rows(&app);
        assert!(rows.iter().any(|row| match row {
            ListRow::Header { label, .. } => label.contains("Gemini"),
            ListRow::Model { .. } => false,
        }));
        assert!(rows.iter().any(|row| match row {
            ListRow::Model { index } => app.models[*index].name.contains("gemini"),
            ListRow::Header { .. } => false,
        }));
    }

    #[test]
    fn model_scroll_sync_does_not_underflow_near_top() {
        let mut app = test_app("");
        open_model_picker(&mut app);
        let rows = build_model_rows(&app);
        let (selected_row, selected_model) = rows
            .iter()
            .enumerate()
            .find_map(|(row, item)| match item {
                ListRow::Model { index } if row >= 13 => Some((row, *index)),
                _ => None,
            })
            .expect("model near viewport edge");
        app.selected_model = selected_model;
        app.model_scroll = 0;

        sync_scroll_to_selection(&mut app, &rows, 14);

        assert!(app.model_scroll <= selected_row);
    }

    #[test]
    fn model_scroll_sync_clamps_large_scroll_values() {
        let mut app = test_app("");
        open_model_picker(&mut app);
        let rows = build_model_rows(&app);
        app.selected_model = first_model_index(&rows).expect("model");
        app.model_scroll = usize::MAX;

        sync_scroll_to_selection(&mut app, &rows, 14);

        assert!(app.model_scroll <= rows.len().saturating_sub(14));
    }

    #[test]
    fn enter_and_shift_enter_insert_newlines() {
        let mut app = test_app("one");

        handle_normal_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        insert_input_char(&mut app, 't');
        insert_input_char(&mut app, 'w');
        insert_input_char(&mut app, 'o');
        handle_normal_key(&mut app, KeyCode::Enter, KeyModifiers::SHIFT);
        insert_input_char(&mut app, 't');
        insert_input_char(&mut app, 'h');
        insert_input_char(&mut app, 'r');
        insert_input_char(&mut app, 'e');
        insert_input_char(&mut app, 'e');

        assert_eq!(app.input, "one\ntwo\nthree");
        assert_eq!(app.input_cursor, app.input.len());
    }

    #[test]
    fn ctrl_enter_sends_non_empty_message() {
        let mut app = test_app("one");
        app.model_provider = None;
        handle_key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
        assert_eq!(app.messages[0].content, "one");
        assert!(app.input.is_empty());

        let mut app = test_app("two");
        app.model_provider = None;
        handle_key(&mut app, KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert_eq!(app.messages[0].content, "two");
        assert!(app.input.is_empty());

        let mut app = test_app("three");
        app.model_provider = None;
        handle_key(&mut app, KeyCode::Char('\n'), KeyModifiers::CONTROL);
        assert_eq!(app.messages[0].content, "three");
        assert!(app.input.is_empty());

        let mut app = test_app("four");
        app.model_provider = None;
        handle_key(&mut app, KeyCode::Char('\r'), KeyModifiers::CONTROL);
        assert_eq!(app.messages[0].content, "four");
        assert!(app.input.is_empty());
    }

    #[test]
    fn empty_ctrl_enter_does_not_open_models() {
        let mut app = test_app("");

        handle_key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.messages.is_empty());
    }

    #[test]
    fn ctrl_o_toggles_full_tool_view() {
        let mut app = test_app("");
        assert!(!app.full_tool_view);

        handle_key(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert!(app.full_tool_view);
        assert!(app.notification.is_some());

        handle_key(&mut app, KeyCode::Char('O'), KeyModifiers::CONTROL);
        assert!(!app.full_tool_view);
    }

    #[test]
    fn slash_opens_commands_and_question_mark_opens_help() {
        let mut app = test_app("");
        assert_eq!(app.mode, Mode::Normal);

        // '?' with empty input opens shortcuts help
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
        assert_eq!(app.mode, Mode::Help);

        handle_help_key(&mut app, KeyCode::Char('?'));
        assert_eq!(app.mode, Mode::Normal);

        // Esc goes back to normal
        app.mode = Mode::Normal;

        // '/' with empty input opens command palette
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        assert_eq!(app.mode, Mode::Commands);

        // Escape goes back to normal
        app.mode = Mode::Normal;

        // Pressing '?' when input is NOT empty inserts it as text
        app.input = "hello".to_string();
        app.input_cursor = 5;
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.input, "hello?");
    }

    #[test]
    fn session_title_prefers_model_heading() {
        let events = vec![
            AgentEvent::UserTaskSubmitted {
                text: "make a dashboard".to_string(),
            },
            AgentEvent::ModelOutput {
                text: "## Cyberpunk Analytics Dashboard\n\nImplemented.".to_string(),
                thinking: None,
            },
        ];

        assert_eq!(
            session_title_from_events(&events).as_deref(),
            Some("Cyberpunk Analytics Dashboard")
        );
    }

    #[test]
    fn session_timestamp_formats_date_and_time() {
        assert_eq!(format_session_timestamp(0), "unknown time");
        assert_eq!(format_session_timestamp(1_700_000_000), "2023-11-14 22:13");
    }

    #[test]
    fn yolo_toggle_uses_notification_not_chat() {
        let mut app = test_app("");
        let message_count = app.messages.len();

        handle_key(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL);

        assert!(app.yolo_mode);
        assert_eq!(app.messages.len(), message_count);
        let notification = app.notification.as_ref().expect("notification");
        assert_eq!(notification.title, "Tools");
        assert!(notification.message.contains("YOLO mode enabled"));
    }

    #[test]
    fn notification_expires_after_ttl() {
        let mut app = test_app("");
        app.notification = Some(Notification {
            title: "Tools".to_string(),
            message: "YOLO mode enabled.".to_string(),
            created_at: Instant::now() - NOTIFICATION_TTL - Duration::from_millis(1),
            ttl: NOTIFICATION_TTL,
        });

        assert!(expire_notification(&mut app));

        assert!(app.notification.is_none());
    }

    #[test]
    fn settings_toggles_thinking_visibility() {
        let mut app = test_app("");
        app.mode = Mode::Settings;
        app.selected_setting = 0;
        assert!(app.show_thinking);

        handle_settings_key(&mut app, KeyCode::Enter);
        assert!(!app.show_thinking);
        assert!(app.notification.is_some());
    }

    #[test]
    fn esc_closes_modal_without_canceling_active_model() {
        let mut app = test_app("");
        app.mode = Mode::Settings;
        app.is_loading = true;

        assert!(!handle_key(&mut app, KeyCode::Esc, KeyModifiers::empty()));

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.is_loading);
    }

    #[test]
    fn ctrl_d_opens_debug_modal() {
        let mut app = test_app("");

        assert!(!handle_key(
            &mut app,
            KeyCode::Char('d'),
            KeyModifiers::CONTROL
        ));

        assert_eq!(app.mode, Mode::Debug);
        assert!(app.log_path.ends_with("logs/navi.log"));
    }

    #[test]
    fn tool_compact_text_is_one_line_with_status() {
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "grep".to_string(),
            input: serde_json::json!({ "pattern": "NAVI" }),
        };
        let ok_result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!({ "matches": [] }),
        };
        let err_result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: false,
            output: serde_json::json!({ "error": "denied" }),
        };

        assert_eq!(
            tool_compact_text(&invocation, &ok_result),
            "grep called · success"
        );
        assert_eq!(
            tool_compact_text(&invocation, &err_result),
            "grep called · error"
        );
        assert!(!tool_compact_text(&invocation, &ok_result).contains('\n'));
    }

    #[test]
    fn tool_full_content_sanitizes_read_file_without_json_io() {
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({ "path": "Cargo.toml" }),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!({
                "path": "Cargo.toml",
                "content": "[workspace]\n",
                "truncated": false,
            }),
        };

        let content = tool_full_content(&invocation, &result);
        assert!(content.contains("read_file called · success"));
        assert!(content.contains("View Cargo.toml"));
        assert!(content.contains("[workspace]"));
        assert!(!content.contains("Input"));
        assert!(!content.contains("Output"));
        assert!(!content.contains("\"path\""));
    }

    #[test]
    fn read_file_tool_full_content_uses_fenced_code_for_highlighting() {
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({ "path": "src/lib.rs" }),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!({
                "path": "src/lib.rs",
                "content": "fn main() {}\n",
            }),
        };

        let content = tool_full_content(&invocation, &result);

        assert!(content.contains("```rust"));
        assert!(content.contains("fn main() {}"));
    }

    #[test]
    fn write_file_tool_full_content_uses_edit_summary() {
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "write_file".to_string(),
            input: serde_json::json!({
                "path": "src/index.html",
                "content": "<!doctype html>\n<html></html>\n"
            }),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!({
                "path": "src/index.html",
                "bytes": 16,
            }),
        };

        let content = tool_full_content(&invocation, &result);

        assert!(content.contains("write_file called · success"));
        assert!(content.contains("Edited src/index.html (+2 -0)"));
        assert!(!content.contains("Input"));
        assert!(!content.contains("Output"));
        assert!(!content.contains("```json"));
        assert!(!content.contains("<!doctype html>"));
    }

    #[test]
    fn completed_tool_removes_empty_tool_placeholder() {
        let mut app = test_app("");
        app.messages.push(ChatMessage {
            model_label: Some("model".to_string()),
            provider_label: Some("provider".to_string()),
            status: Some("tool: read_file".to_string()),
            ..ChatMessage::new(ChatRole::Assistant, String::new())
        });

        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({ "path": "Cargo.toml" }),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!({ "path": "Cargo.toml", "content": "" }),
        };

        app.tool_invocations
            .insert(invocation.id.clone(), invocation.clone());
        app.running_tools
            .insert(invocation.id.clone(), invocation.clone());

        // Process ToolCompleted event logic directly as in the main event loop
        app.running_tools.remove(&result.invocation_id);
        if let Some(invocation) = app.tool_invocations.get(&result.invocation_id).cloned() {
            remove_active_tool_placeholder(&mut app);
            app.messages.push(ChatMessage {
                status: Some("tool result".to_string()),
                tool_invocation: Some(invocation.clone()),
                tool_result: Some(result.clone()),
                ..ChatMessage::new(ChatRole::Assistant, String::new())
            });
        }

        assert_eq!(
            app.messages
                .iter()
                .filter(|message| message.status.as_deref() == Some("tool result"))
                .count(),
            1
        );
        assert!(!app.messages.iter().any(is_empty_tool_placeholder));
    }

    #[test]
    fn compact_tool_render_hides_full_input_and_output() {
        let mut app = test_app("");
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "list_files".to_string(),
            input: serde_json::json!({ "path": "/tmp/project" }),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!({
                "files": ["/tmp/project/package.json", "/tmp/project/src/App.tsx"]
            }),
        };
        app.messages.push(ChatMessage {
            status: Some("tool result".to_string()),
            tool_invocation: Some(invocation),
            tool_result: Some(result),
            ..ChatMessage::new(
                ChatRole::Assistant,
                "stale full tool content should not render in compact mode\n\nInput\nOutput"
                    .to_string(),
            )
        });

        let text = build_chat_lines(&app, 80)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("list_files called · success"));
        assert!(!text.contains("Input"));
        assert!(!text.contains("Output"));
        assert!(!text.contains("stale full tool content"));
    }

    #[test]
    fn full_tool_render_generates_sanitized_metadata_view() {
        let mut app = test_app("");
        app.full_tool_view = true;
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "grep".to_string(),
            input: serde_json::json!({ "pattern": "NAVI" }),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: false,
            output: serde_json::json!({ "error": "denied" }),
        };
        app.messages.push(ChatMessage {
            status: Some("tool result".to_string()),
            tool_invocation: Some(invocation),
            tool_result: Some(result),
            ..ChatMessage::new(ChatRole::Assistant, String::new())
        });

        let text = build_chat_lines(&app, 80)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("grep called · error"));
        assert!(text.contains("denied"));
        assert!(!text.contains("Input"));
        assert!(!text.contains("Output"));
        assert!(!text.contains("```json"));
    }

    #[test]
    fn apply_patch_tool_full_content_uses_edit_summary() {
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "apply_patch".to_string(),
            input: serde_json::json!({
                "patch": "*** Begin Patch\n*** Update File: crates/navi-tui/src/lib.rs\n@@\n-    old\n+    new\n+    added\n*** End Patch\n"
            }),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!({ "status": 0 }),
        };

        let content = tool_full_content(&invocation, &result);

        assert!(content.contains("apply_patch called · success"));
        assert!(content.contains("Edited crates/navi-tui/src/lib.rs (+2 -1)"));
        assert!(!content.contains("*** Begin Patch"));
        assert!(!content.contains("Input"));
        assert!(!content.contains("Output"));
    }

    #[tokio::test]
    async fn command_palette_sync_models_starts_sync() {
        let mut app = test_app("");
        app.command_filter = "sync".to_string();
        app.selected_command = 0;

        let commands = filtered_commands(&app);
        assert!(
            commands
                .iter()
                .any(|c| matches!(c.action, CommandAction::SyncModels))
        );

        sync_models_tui(&mut app);

        assert!(app.is_loading);
        assert!(app.loading_start.is_some());
        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.messages[0].content, "Syncing models from providers...");
        assert_eq!(app.messages[0].status, Some("syncing".to_string()));
    }

    #[tokio::test]
    async fn model_picker_tab_triggers_per_provider_sync() {
        let mut app = test_app("");
        app.mode = Mode::Models;

        let provider_id = app.models[app.selected_model].provider_id.clone();

        // Press Tab to trigger per-provider sync
        handle_model_key(&mut app, KeyCode::Tab, KeyModifiers::NONE);

        assert!(app.is_loading);
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.messages.len(), 1);
        assert!(
            app.messages[0].content.contains(&provider_id),
            "Tab sync message should mention the provider: got '{}'",
            app.messages[0].content
        );
    }

    #[tokio::test]
    async fn model_picker_ctrl_r_triggers_all_provider_sync() {
        let mut app = test_app("");
        app.mode = Mode::Models;

        // Press Ctrl+r to trigger all-provider sync
        handle_model_key(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);

        assert!(app.is_loading);
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.messages[0].content, "Syncing models from providers...");
    }

    #[test]
    fn model_picker_ctrl_e_opens_provider_setup() {
        let mut app = test_app("");
        app.mode = Mode::Models;
        let selected = app.selected_model;

        handle_model_key(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);

        assert_eq!(app.mode, Mode::ApiKeyEntry);
        assert_eq!(app.pending_model_selection, Some(selected));
        assert!(app.api_key_input.is_empty());
        assert_eq!(app.api_key_cursor, 0);
    }

    #[test]
    fn model_error_is_rendered_as_separate_message() {
        let mut app = test_app("");
        app.messages.push(ChatMessage {
            status: Some("tool result".to_string()),
            ..ChatMessage::new(
                ChatRole::Assistant,
                "✓ write_file called · success".to_string(),
            )
        });
        app.messages.push(ChatMessage {
            status: Some("thinking".to_string()),
            ..ChatMessage::new(ChatRole::Assistant, String::new())
        });
        app.is_loading = true;
        app.skip_next_model_done = true;

        handle_model_error(
            &mut app,
            "provider request failed with 400 Bad Request".to_string(),
        );

        assert_eq!(app.messages[0].status.as_deref(), Some("tool result"));
        assert_eq!(app.messages[2].status.as_deref(), Some("error"));
        assert!(app.messages[2].content.contains("400"));
        assert!(!app.is_loading);
        assert!(!app.skip_next_model_done);
    }

    #[tokio::test]
    async fn transient_model_error_retries_without_final_error() {
        let mut app = test_app("");
        app.model_provider = None;
        app.messages.push(ChatMessage {
            status: Some("thinking".to_string()),
            ..ChatMessage::new(ChatRole::Assistant, String::new())
        });
        app.is_loading = true;

        handle_model_error(
            &mut app,
            "failed to read chat completions stream: unexpected EOF during chunk size line"
                .to_string(),
        );

        assert_eq!(app.model_retry_attempts, 1);
        assert!(app.is_loading);
        assert!(app.stream_task.is_some());
        assert!(
            app.messages
                .iter()
                .any(|message| message.status.as_deref() == Some("retrying"))
        );
        assert!(
            app.messages
                .iter()
                .all(|message| message.status.as_deref() != Some("thinking"))
        );
    }

    #[test]
    fn model_retry_delay_uses_rate_limit_backoff_without_requested_delay() {
        let delay = model_retry_delay(
            "API error 429 Too Many Requests: {\"status\":429} (requested delay: None)",
            2,
        );

        assert_eq!(delay, Duration::from_secs(20));
    }

    #[test]
    fn model_retry_delay_uses_requested_delay_when_present() {
        let delay = model_retry_delay(
            "API error 429 Too Many Requests: {} (requested delay: Some(1500ms))",
            1,
        );

        assert_eq!(delay, Duration::from_millis(1500));
    }

    #[test]
    fn model_retry_delay_caps_large_requested_delay() {
        let delay = model_retry_delay(
            "API error 429 Too Many Requests: {} (requested delay: Some(64649s))",
            1,
        );

        assert_eq!(delay, Duration::from_secs(60));
    }

    #[test]
    fn free_usage_limit_error_does_not_schedule_retry() {
        let mut app = test_app("");
        app.messages.push(ChatMessage {
            status: Some("thinking".to_string()),
            ..ChatMessage::new(ChatRole::Assistant, String::new())
        });
        app.is_loading = true;

        handle_model_error(
            &mut app,
            "API error 429 Too Many Requests: {\"type\":\"error\",\"error\":{\"type\":\"FreeUsageLimitError\",\"message\":\"Rate limit exceeded.\"}} (requested delay: Some(64649s))".to_string(),
        );

        assert_eq!(app.model_retry_attempts, 0);
        assert!(!app.is_loading);
        assert!(app.stream_task.is_none());
        assert!(
            app.messages
                .last()
                .unwrap()
                .content
                .contains("Usage limit reached for")
        );
        assert!(
            app.messages
                .last()
                .unwrap()
                .content
                .contains("select a non-free model")
        );
    }

    #[test]
    fn opencode_zen_model_names_are_canonicalized_for_api_requests() {
        assert_eq!(
            provider_request_model_name("opencode", "DeepSeek V4 Flash Free"),
            "deepseek-v4-flash-free"
        );
        assert_eq!(
            provider_request_model_name("opencode-zen", "opencode/Nemotron 3 Super Free"),
            "nemotron-3-super-free"
        );
        assert_eq!(
            provider_request_model_name("openrouter", "DeepSeek V4 Flash Free"),
            "DeepSeek V4 Flash Free"
        );
    }

    #[test]
    fn opencode_free_models_can_use_public_access_without_key() {
        let app = test_app("");
        let model = ModelOption {
            name: "deepseek-v4-flash-free".to_string(),
            provider_id: "opencode".to_string(),
            provider_label: "OpenCode Zen".to_string(),
            provider_description: "Recommended".to_string(),
            task_size: navi_core::ModelTaskSize::Small,
            context_window_tokens: None,
        };

        assert!(model_is_available_for_selection(&app, &model));
        assert_eq!(
            provider_request_model_name("opencode", "deepseek-v4-flash-free"),
            "deepseek-v4-flash-free"
        );
    }

    #[test]
    fn escape_cancels_active_tool_task_state() {
        let mut app = test_app("");
        app.is_loading = true;
        app.skip_next_model_done = true;
        app.messages.push(ChatMessage {
            status: Some("tool: bash".to_string()),
            ..ChatMessage::new(ChatRole::Assistant, String::new())
        });

        let should_quit = handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        assert!(!should_quit);
        assert!(!app.is_loading);
        assert!(!app.skip_next_model_done);
        assert_eq!(
            active_assistant_message(&mut app).and_then(|message| message.status.clone()),
            Some("cancelled".to_string())
        );
    }

    #[test]
    fn mask_key_hides_middle_characters() {
        let short = "sk-abc";
        assert_eq!(mask_key_segment(short), "sk-abc");

        let long = "sk-proj-abcdefghijklmnop";
        let masked = mask_key_segment(long);
        assert!(masked.starts_with("sk-pro"));
        assert!(masked.ends_with("mnop"));
        assert!(masked.contains('•'));
    }
}
