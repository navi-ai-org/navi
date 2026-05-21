use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use navi_core::{
    CredentialStore, LoadedConfig, ModelMessage, ModelOption, ModelProvider, ModelRequest,
    ModelResponse, ModelRole, ModelTaskSize, ThinkingConfig, available_model_options,
    resolve_provider_config, save_global_config, AgentEvent, SessionId, SessionSnapshot,
    SessionStore,
};
use navi_openai::OpenAiProvider;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{CrosstermBackend, Frame, Line, Span, Terminal};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

// ─── async bridge ──────────────────────────────────────────────────────────────
enum AsyncEvent {
    ModelResponse {
        response: ModelResponse,
        elapsed_ms: u64,
        model_label: String,
        provider_label: String,
    },
    ModelError {
        message: String,
    },
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
    large_task: bool,
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

    // provider
    model_provider: Option<Arc<dyn ModelProvider>>,

    // credentials
    credential_store: CredentialStore,
    api_key_input: String,
    api_key_cursor: usize,
    pending_model_selection: Option<usize>,
    vim_enabled: bool,
    vim_mode: VimMode,

    // stats
    total_tokens_estimate: usize,

    // persistence
    session_store: SessionStore,
    events: Vec<AgentEvent>,
    session_id: SessionId,
    project_dir: PathBuf,
    saved_sessions: Vec<SessionSnapshot>,
    selected_session: usize,
    session_scroll: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Commands,
    Models,
    ApiKeyEntry,
    Thinking,
    Sessions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VimMode {
    Insert,
    Normal,
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
    ToggleVimMode,
    OpenThinking,
    InitializeProject,
    Quit,
}

impl TuiApp {
    pub fn new(loaded_config: LoadedConfig, project_dir: PathBuf, task: Option<String>) -> Self {
        let models = available_model_options(&loaded_config.config);
        let selected_model = models
            .iter()
            .position(|model| {
                model.name == loaded_config.config.model.name
                    && model.provider_id == loaded_config.config.model.provider
            })
            .unwrap_or(0);

        let (async_tx, async_rx) = mpsc::unbounded_channel();
        let credential_store = CredentialStore::new(loaded_config.data_dir.clone());
        let model_provider = build_provider(&loaded_config, &credential_store);
        let session_store = SessionStore::new(loaded_config.data_dir.clone());
        let session_id = SessionStore::create_id();
        let saved_sessions = load_saved_sessions(&session_store);

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
            large_task: true,
            thinking_level: ThinkingLevel::High,
            selected_thinking: 1,
            tick: 0,
            messages: Vec::new(),
            scroll_offset: 0,
            is_loading: false,
            loading_start: None,
            conversation_history: vec![ModelMessage {
                role: ModelRole::System,
                content: default_system_prompt(),
            }],
            async_tx,
            async_rx,
            model_provider,
            credential_store,
            api_key_input: String::new(),
            api_key_cursor: 0,
            pending_model_selection: None,
            vim_enabled: false,
            vim_mode: VimMode::Insert,
            total_tokens_estimate: 0,
            session_store,
            events: Vec::new(),
            session_id,
            project_dir,
            saved_sessions,
            selected_session: 0,
            session_scroll: 0,
        };

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
        label: "Toggle Vim Mode",
        shortcut: None,
        action: CommandAction::ToggleVimMode,
    },
    CommandItem {
        label: "Thinking Mode",
        shortcut: None,
        action: CommandAction::OpenThinking,
    },
    CommandItem {
        label: "Initialize Layer",
        shortcut: None,
        action: CommandAction::InitializeProject,
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
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, mut app: TuiApp) -> Result<()> {
    // If input was pre-filled from CLI task, submit on first frame
    if !app.input.trim().is_empty() && app.mode == Mode::Normal {
        submit_message(&mut app);
    }

    loop {
        terminal.draw(|frame| render(frame, &app))?;
        app.tick = app.tick.wrapping_add(1);

        // Check for async model responses (non-blocking)
        while let Ok(event) = app.async_rx.try_recv() {
            match event {
                AsyncEvent::ModelResponse {
                    response,
                    elapsed_ms,
                    model_label,
                    provider_label,
                } => {
                    let word_count = response.text.split_whitespace().count();
                    app.total_tokens_estimate += word_count * 4 / 3;
                    app.messages.push(ChatMessage {
                        role: ChatRole::Assistant,
                        content: response.text.clone(),
                        model_label: Some(model_label),
                        provider_label: Some(provider_label),
                        elapsed_ms: Some(elapsed_ms),
                    });
                    app.conversation_history.push(ModelMessage {
                        role: ModelRole::Assistant,
                        content: response.text.clone(),
                    });
                    app.events.push(AgentEvent::ModelOutput {
                        text: response.text,
                    });
                    app.is_loading = false;
                    app.loading_start = None;
                    app.scroll_offset = 0;
                }
                AsyncEvent::ModelError { message } => {
                    app.messages.push(ChatMessage {
                        role: ChatRole::Assistant,
                        content: format!("⚠ Error: {message}"),
                        model_label: None,
                        provider_label: None,
                        elapsed_ms: None,
                    });
                    app.events.push(AgentEvent::Error {
                        message: message.clone(),
                    });
                    app.is_loading = false;
                    app.loading_start = None;
                }
            }
        }

        if event::poll(Duration::from_millis(80))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                if handle_key(&mut app, key.code, key.modifiers) {
                    break;
                }
            }
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

    let word_count = text.split_whitespace().count();
    app.total_tokens_estimate += word_count * 4 / 3;

    app.messages.push(ChatMessage {
        role: ChatRole::User,
        content: text.clone(),
        model_label: None,
        provider_label: None,
        elapsed_ms: None,
    });

    app.conversation_history.push(ModelMessage {
        role: ModelRole::User,
        content: text.clone(),
    });

    app.events.push(AgentEvent::UserTaskSubmitted { text: text.clone() });

    app.input.clear();
    app.input_cursor = 0;
    app.scroll_offset = 0;

    let Some(provider) = app.model_provider.clone() else {
        app.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            content:
                "⚠ No API key configured. Press ctrl+m, choose a protocol, then enter its key."
                    .to_string(),
            model_label: None,
            provider_label: None,
            elapsed_ms: None,
        });
        return;
    };

    app.is_loading = true;
    app.loading_start = Some(Instant::now());

    let request = ModelRequest {
        model: app.loaded_config.config.model.name.clone(),
        messages: app.conversation_history.clone(),
        thinking: app.thinking_level.into(),
    };

    let model_label = app.loaded_config.config.model.name.clone();
    let provider_label = selected_provider_label(app).to_string();
    let tx = app.async_tx.clone();

    tokio::spawn(async move {
        let start = Instant::now();
        match provider.complete(request).await {
            Ok(response) => {
                let elapsed_ms = start.elapsed().as_millis() as u64;
                let _ = tx.send(AsyncEvent::ModelResponse {
                    response,
                    elapsed_ms,
                    model_label,
                    provider_label,
                });
            }
            Err(err) => {
                let _ = tx.send(AsyncEvent::ModelError {
                    message: format!("{err:#}"),
                });
            }
        }
    });
}

fn build_provider(
    loaded_config: &LoadedConfig,
    credential_store: &CredentialStore,
) -> Option<Arc<dyn ModelProvider>> {
    let provider_config =
        resolve_provider_config(&loaded_config.config, &loaded_config.config.model.provider)?;

    // Try to resolve the key: env var first, then stored credential
    let api_key =
        credential_store.resolve_api_key(&provider_config.id, &provider_config.api_key_env)?;

    match OpenAiProvider::from_provider_config_with_key(&provider_config, api_key) {
        Ok(provider) => Some(Arc::new(provider)),
        Err(_) => None,
    }
}

fn rebuild_provider(app: &mut TuiApp) {
    app.model_provider = build_provider(&app.loaded_config, &app.credential_store);
}

fn provider_has_api_key(app: &TuiApp, provider_id: &str) -> bool {
    resolve_provider_config(&app.loaded_config.config, provider_id)
        .and_then(|provider_config| {
            app.credential_store
                .resolve_api_key(&provider_config.id, &provider_config.api_key_env)
        })
        .is_some()
}

fn apply_model_selection(app: &mut TuiApp, model_index: usize) {
    let Some(model) = app.models.get(model_index) else {
        return;
    };

    app.loaded_config.config.model.provider = model.provider_id.clone();
    app.loaded_config.config.model.name = model.name.clone();
    app.selected_model = model_index;
    app.model_scroll = 0;
    rebuild_provider(app);
}

fn selected_or_pending_provider_id(app: &TuiApp) -> String {
    app.pending_model_selection
        .and_then(|index| app.models.get(index))
        .map(|model| model.provider_id.clone())
        .unwrap_or_else(|| app.loaded_config.config.model.provider.clone())
}

fn selected_or_pending_provider_label(app: &TuiApp) -> String {
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
        app.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            content: format!("⚠ Failed to save key: {err:#}"),
            model_label: None,
            provider_label: None,
            elapsed_ms: None,
        });
    } else {
        app.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            content: format!(
                "✓ API key saved for provider \"{provider_id}\". Credentials stored securely."
            ),
            model_label: None,
            provider_label: None,
            elapsed_ms: None,
        });
    }

    if let Some(model_index) = app.pending_model_selection.take() {
        apply_model_selection(app, model_index);
    } else {
        rebuild_provider(app);
    }
    app.api_key_input.clear();
    app.api_key_cursor = 0;
    app.mode = Mode::Normal;
}

fn current_provider_env_var(app: &TuiApp) -> String {
    let provider_id = selected_or_pending_provider_id(app);
    resolve_provider_config(&app.loaded_config.config, &provider_id)
        .map(|p| p.api_key_env.clone())
        .unwrap_or_else(|| "API_KEY".to_string())
}

fn default_system_prompt() -> String {
    "You are NAVI, an autonomous code agent inside a terminal. Be concise and helpful. When showing code, use markdown code blocks."
        .to_string()
}

// ─── key handling ──────────────────────────────────────────────────────────────
fn handle_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('c') => return true,
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
            KeyCode::Char('j') => {
                insert_input_char(app, '\n');
                return false;
            }
            KeyCode::Enter => {
                if !app.input.trim().is_empty() && !app.is_loading {
                    submit_message(app);
                }
                return false;
            }
            KeyCode::Char('n') => {
                app.messages.clear();
                app.conversation_history = vec![ModelMessage {
                    role: ModelRole::System,
                    content: default_system_prompt(),
                }];
                app.input.clear();
                app.input_cursor = 0;
                app.scroll_offset = 0;
                app.total_tokens_estimate = 0;
                return false;
            }
            _ => {}
        }
    }

    match app.mode {
        Mode::Normal => handle_normal_key(app, code, modifiers),
        Mode::Commands => handle_command_key(app, code),
        Mode::Models => handle_model_key(app, code),
        Mode::ApiKeyEntry => handle_api_key_key(app, code, modifiers),
        Mode::Thinking => handle_thinking_key(app, code),
        Mode::Sessions => handle_sessions_key(app, code),
    }
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
            app.large_task = matches!(level, ThinkingLevel::Max | ThinkingLevel::High);
            app.mode = Mode::Normal;
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
            app.selected_session = (app.selected_session + 1).min(app.saved_sessions.len().saturating_sub(1));
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
                let path = app.session_store.root().join(format!("{}.json", snapshot.id.0));
                let _ = std::fs::remove_file(&path);
            }
            app.saved_sessions = load_saved_sessions(&app.session_store);
            app.selected_session = app.selected_session.min(app.saved_sessions.len().saturating_sub(1));
        }
        _ => {}
    }

    false
}

fn handle_normal_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {
    if app.vim_enabled && app.vim_mode == VimMode::Normal {
        return handle_vim_normal_key(app, code);
    }

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
            if app.vim_enabled {
                app.vim_mode = VimMode::Normal;
            } else {
                app.scroll_offset = 0;
            }
        }
        _ => {}
    }

    false
}

fn handle_vim_normal_key(app: &mut TuiApp, code: KeyCode) -> bool {
    match code {
        KeyCode::Char('i') => app.vim_mode = VimMode::Insert,
        KeyCode::Char('a') => {
            move_input_next_char(app);
            app.vim_mode = VimMode::Insert;
        }
        KeyCode::Char('A') => {
            app.input_cursor = app.input.len();
            app.vim_mode = VimMode::Insert;
        }
        KeyCode::Char('I') => {
            app.input_cursor = 0;
            app.vim_mode = VimMode::Insert;
        }
        KeyCode::Char('h') | KeyCode::Left => move_input_previous_char(app),
        KeyCode::Char('l') | KeyCode::Right => move_input_next_char(app),
        KeyCode::Char('b') => move_input_previous_hump(app),
        KeyCode::Char('w') => move_input_next_hump(app),
        KeyCode::Char('0') | KeyCode::Home => app.input_cursor = 0,
        KeyCode::Char('$') | KeyCode::End => app.input_cursor = app.input.len(),
        KeyCode::Char('x') | KeyCode::Delete => delete_input_next_char(app),
        KeyCode::Char('X') | KeyCode::Backspace => delete_input_previous_char(app),
        KeyCode::Char('o') => {
            app.input_cursor = app.input.len();
            insert_input_char(app, '\n');
            app.vim_mode = VimMode::Insert;
        }
        KeyCode::Enter => {
            if !app.input.trim().is_empty() && !app.is_loading {
                submit_message(app);
            } else if app.input.is_empty() {
                open_model_picker(app);
            }
        }
        KeyCode::Esc => {}
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

fn handle_model_key(app: &mut TuiApp, code: KeyCode) -> bool {
    let rows = build_model_rows(app);
    // List visible height is approximately modal height (22) minus decoration (~7 rows)
    let visible_rows = 14u16;
    match code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Tab => {
            app.large_task = !app.large_task;
            app.model_scroll = 0;
            app.selected_model =
                first_model_index(&build_model_rows(app)).unwrap_or(app.selected_model);
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
            if provider_has_api_key(app, &model.provider_id) {
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
            app.mode = Mode::Normal;
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
            app.conversation_history = vec![ModelMessage {
                role: ModelRole::System,
                content: default_system_prompt(),
            }];
            app.input.clear();
            app.input_cursor = 0;
            app.scroll_offset = 0;
            app.total_tokens_estimate = 0;
            app.mode = Mode::Normal;
        }
        CommandAction::SwitchModel => {
            open_model_picker(app);
        }
        CommandAction::ToggleVimMode => {
            app.vim_enabled = !app.vim_enabled;
            app.vim_mode = if app.vim_enabled {
                VimMode::Normal
            } else {
                VimMode::Insert
            };
            app.mode = Mode::Normal;
        }
        CommandAction::OpenThinking => {
            open_thinking_picker(app);
        }
        CommandAction::Sessions => {
            open_sessions_picker(app);
        }
        CommandAction::Quit => return true,
        _ => app.mode = Mode::Normal,
    }

    false
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
            Constraint::Length(6), // input area
        ])
        .split(area);

    render_chat_area(frame, app, vertical[0]);
    render_input(frame, app, vertical[1]);

    match app.mode {
        Mode::Commands => render_command_palette(frame, app, modal_rect(area, 68, 15)),
        Mode::Models => render_model_picker(frame, app, modal_rect(area, 72, 22)),
        Mode::ApiKeyEntry => render_api_key_entry(frame, app, modal_rect(area, 72, 11)),
        Mode::Thinking => render_thinking_picker(frame, app, modal_rect(area, 40, 10)),
        Mode::Sessions => render_sessions_picker(frame, app, modal_rect(area, 72, 16)),
        Mode::Normal => {}
    }
}

// ─── chat area ─────────────────────────────────────────────────────────────────
fn render_chat_area(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 0,
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

    // Build rendered lines from messages
    let chat_width = inner.width as usize;
    let mut rendered_lines: Vec<Line<'_>> = Vec::new();

    for msg in &app.messages {
        rendered_lines.push(Line::from(""));

        match msg.role {
            ChatRole::User => {
                let wrapped = wrap_text(&msg.content, chat_width.saturating_sub(4));
                for line_text in &wrapped {
                    rendered_lines.push(Line::from(vec![
                        Span::styled("│ ", Style::default().fg(USER_ACCENT)),
                        Span::styled(line_text.clone(), Style::default().fg(TEXT)),
                    ]));
                }
            }
            ChatRole::Assistant => {
                let wrapped = wrap_text(&msg.content, chat_width.saturating_sub(2));
                for line_text in &wrapped {
                    rendered_lines.push(Line::from(Span::styled(
                        line_text.clone(),
                        Style::default().fg(TEXT),
                    )));
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

                    let attr_text = format!("◇ {model_label} via {provider_label} {elapsed}");
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

    // Loading indicator
    if app.is_loading {
        rendered_lines.push(Line::from(""));
        let dots = match (app.tick / 3) % 4 {
            0 => "⠋",
            1 => "⠙",
            2 => "⠹",
            _ => "⠸",
        };
        let elapsed_str = app
            .loading_start
            .map(|start| {
                let secs = start.elapsed().as_secs();
                format!(" {secs}s")
            })
            .unwrap_or_default();
        rendered_lines.push(Line::from(vec![
            Span::styled(dots, Style::default().fg(SIGNAL)),
            Span::styled(
                format!(" thinking{elapsed_str}"),
                Style::default().fg(MUTED),
            ),
        ]));
    }

    // Apply scroll offset (from bottom)
    let visible_height = inner.height as usize;
    let total_lines = rendered_lines.len();
    let max_scroll = total_lines.saturating_sub(visible_height);
    let effective_scroll = app.scroll_offset.min(max_scroll);
    let start = total_lines
        .saturating_sub(visible_height)
        .saturating_sub(effective_scroll);
    let end = (start + visible_height).min(total_lines);

    let visible_lines: Vec<Line<'_>> = rendered_lines[start..end].to_vec();

    frame.render_widget(
        Paragraph::new(Text::from(visible_lines))
            .style(Style::default().bg(BG))
            .wrap(Wrap { trim: false }),
        inner,
    );
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
        vertical: 0,
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
    let prompt = if app.vim_enabled {
        match app.vim_mode {
            VimMode::Insert => "> i ",
            VimMode::Normal => "> n ",
        }
    } else {
        "> "
    };
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
    let vim_state = if app.vim_enabled {
        match app.vim_mode {
            VimMode::Insert => "vim:insert",
            VimMode::Normal => "vim:normal",
        }
    } else {
        "vim:off"
    };

    let items = [
        ("enter", "newline", TEXT),
        ("ctrl+enter", "send prompt", TEXT),
        ("ctrl+j", "newline", TEXT),
        ("ctrl+m", "models", TEXT),
        ("ctrl+p", "commands", TEXT),
        ("ctrl+c", "quit", TEXT),
        (vim_state, "", ACCENT),
    ];

    let mut spans = vec![Span::styled("   ", Style::default().fg(MUTED))];
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
            spans.push(Span::styled("  ·  ", Style::default().fg(GHOST)));
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
        .constraints([
            Constraint::Min(5),
            Constraint::Length(1),
        ])
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
            ListItem::new(Span::styled(
                format!("{}{}", marker, level.label()),
                style,
            ))
            .style(style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(List::new(items).style(Style::default().bg(PANEL)), rows[0]);
    frame.render_widget(
        Paragraph::new("↑↓ choose  •  enter confirm  •  esc cancel")
            .style(Style::default().fg(MUTED).bg(PANEL)),
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
        .constraints([
            Constraint::Min(10),
            Constraint::Length(1),
        ])
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

                let event_count = snapshot.events.len();
                let project = snapshot
                    .project
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| snapshot.project.to_string_lossy().to_string());
                let label = format!("{project}  ({event_count} events)");

                ListItem::new(Span::styled(label, style)).style(style)
            })
            .collect::<Vec<_>>();

        frame.render_widget(
            List::new(items).style(Style::default().bg(PANEL)),
            rows[0],
        );
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
            Constraint::Length(2),
            Constraint::Length(1),
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
        Paragraph::new(Text::from(vec![
            Line::from(vec![
                Span::styled("> ", Style::default().fg(SIGNAL)),
                Span::styled(
                    filter_text,
                    Style::default().fg(if app.model_filter.is_empty() {
                        MUTED
                    } else {
                        TEXT
                    }),
                ),
            ]),
            Line::from(Span::styled(
                if app.large_task {
                    "large, complex tasks"
                } else {
                    "quick, small tasks"
                },
                Style::default().fg(MUTED),
            )),
        ]))
        .style(Style::default().bg(PANEL)),
        rows[0],
    );

    let tabs = Line::from(vec![
        Span::styled(
            if app.large_task {
                "◉ Large Task"
            } else {
                "○ Large Task"
            },
            Style::default().fg(TEXT),
        ),
        Span::styled("    ", Style::default().fg(MUTED)),
        Span::styled(
            if app.large_task {
                "○ Small Task"
            } else {
                "◉ Small Task"
            },
            Style::default().fg(TEXT),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(tabs)
            .alignment(Alignment::Right)
            .style(Style::default().bg(PANEL)),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new("Provider protocols").style(Style::default().fg(MUTED).bg(PANEL)),
        rows[2],
    );

    let list_rows = build_model_rows(app);
    let list_area = rows[3];
    let row_width = list_area.width as usize;

    let selected_row = selected_model_in_rows(&list_rows, app.selected_model).unwrap_or(0);
    let mut list_state = ListState::default()
        .with_offset(app.model_scroll)
        .with_selected(Some(selected_row));

    let items = list_rows
        .iter()
        .map(|row| match row {
            ListRow::Header { label, description } => {
                let header_style = Style::default()
                    .fg(TEXT)
                    .bg(PANEL)
                    .add_modifier(Modifier::BOLD);
                let desc_style = Style::default().fg(MUTED).bg(PANEL);

                if description.is_empty() {
                    ListItem::new(Span::styled(format!("  {}", label), header_style))
                        .style(header_style)
                } else {
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("  {}", label), header_style),
                        Span::styled("  ", desc_style),
                        Span::styled(format!("({})", description), desc_style),
                    ]))
                    .style(header_style)
                }
            }
            ListRow::Model { index } => {
                let model = &app.models[*index];
                let selected = *index == app.selected_model;
                let configured = model.name == app.loaded_config.config.model.name
                    && model.provider_id == app.loaded_config.config.model.provider;
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
            "type search  •  ↑↓ choose  •  tab task size  •  enter confirm  •  esc exit",
        )
        .style(Style::default().fg(MUTED).bg(PANEL)),
        rows[4],
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
    app.models
        .iter()
        .find(|model| model.provider_id == app.loaded_config.config.model.provider)
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
enum ListRow {
    Header { label: String, description: String },
    Model { index: usize },
}

fn build_model_rows(app: &TuiApp) -> Vec<ListRow> {
    let task_size = if app.large_task {
        ModelTaskSize::Large
    } else {
        ModelTaskSize::Small
    };
    let filter = app.model_filter.trim().to_lowercase();

    // Group visible models by provider label
    let mut rows = Vec::new();
    let mut current_provider: Option<&str> = None;

    for (index, model) in app.models.iter().enumerate() {
        if model.task_size != task_size {
            continue;
        }
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
    if let Some(selected_row) = selected_model_in_rows(rows, app.selected_model) {
        let selected_row_u16 = selected_row as u16;
        if selected_row_u16 < app.model_scroll as u16 {
            app.model_scroll = selected_row;
        } else if selected_row_u16 >= app.model_scroll as u16 + visible_rows.saturating_sub(1) {
            app.model_scroll = (selected_row_u16 - visible_rows + 4) as usize;
        }
    }
}

// ─── persistence ───────────────────────────────────────────────────────────────
fn load_saved_sessions(store: &SessionStore) -> Vec<SessionSnapshot> {
    store.list()
}

fn save_current_session(app: &mut TuiApp) {
    if app.messages.is_empty() && app.events.is_empty() {
        return;
    }
    let snapshot = SessionSnapshot {
        id: app.session_id.clone(),
        project: app.project_dir.clone(),
        events: app.events.clone(),
    };
    if let Err(err) = app.session_store.save(&snapshot) {
        eprintln!("failed to save session: {err:#}");
    }
    app.session_id = SessionStore::create_id();
    app.events.clear();
}

fn save_preferences(app: &mut TuiApp) {
    app.loaded_config.config.model.name = app.models.get(app.selected_model)
        .map(|m| m.name.clone())
        .unwrap_or_else(|| app.loaded_config.config.model.name.clone());
    app.loaded_config.config.model.provider = app.models.get(app.selected_model)
        .map(|m| m.provider_id.clone())
        .unwrap_or_else(|| app.loaded_config.config.model.provider.clone());

    let global_path = app.loaded_config.global_config_path.as_ref().expect("global config path");
    if let Err(err) = save_global_config(global_path, &app.loaded_config.config) {
        eprintln!("failed to save preferences: {err:#}");
    }
}

fn load_session(app: &mut TuiApp, snapshot: &SessionSnapshot) {
    app.messages.clear();
    app.conversation_history = vec![ModelMessage {
        role: ModelRole::System,
        content: default_system_prompt(),
    }];
    app.events.clear();
    app.total_tokens_estimate = 0;

    for event in &snapshot.events {
        match event {
            AgentEvent::UserTaskSubmitted { text } => {
                let word_count = text.split_whitespace().count();
                app.total_tokens_estimate += word_count * 4 / 3;
                app.messages.push(ChatMessage {
                    role: ChatRole::User,
                    content: text.clone(),
                    model_label: None,
                    provider_label: None,
                    elapsed_ms: None,
                });
                app.conversation_history.push(ModelMessage {
                    role: ModelRole::User,
                    content: text.clone(),
                });
            }
            AgentEvent::ModelOutput { text } => {
                let word_count = text.split_whitespace().count();
                app.total_tokens_estimate += word_count * 4 / 3;
                app.messages.push(ChatMessage {
                    role: ChatRole::Assistant,
                    content: text.clone(),
                    model_label: None,
                    provider_label: None,
                    elapsed_ms: None,
                });
                app.conversation_history.push(ModelMessage {
                    role: ModelRole::Assistant,
                    content: text.clone(),
                });
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
                task_size: ModelTaskSize::Large,
            }],
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

        handle_model_key(&mut app, KeyCode::Enter);

        assert_eq!(app.mode, Mode::ApiKeyEntry);
        assert_eq!(app.pending_model_selection, Some(app.selected_model));
    }

    #[test]
    fn model_picker_filters_by_model_and_provider_text() {
        let mut app = test_app("");
        open_model_picker(&mut app);
        app.large_task = true;

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
    fn enter_shift_enter_and_ctrl_j_insert_newlines() {
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
        handle_key(&mut app, KeyCode::Char('j'), KeyModifiers::CONTROL);

        assert_eq!(app.input, "one\ntwo\nthree\n");
        assert_eq!(app.input_cursor, app.input.len());
    }

    #[test]
    fn ctrl_enter_sends_non_empty_message() {
        let mut app = test_app("one");
        app.model_provider = None;

        handle_key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);

        assert_eq!(app.messages[0].content, "one");
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
    fn command_palette_toggles_vim_mode() {
        let mut app = test_app("abc");
        app.command_filter = "vim".to_string();
        app.selected_command = 0;

        run_selected_command(&mut app);

        assert!(app.vim_enabled);
        assert_eq!(app.vim_mode, VimMode::Normal);
    }

    #[test]
    fn vim_normal_mode_moves_and_returns_to_insert() {
        let mut app = test_app("abc");
        app.vim_enabled = true;
        app.vim_mode = VimMode::Normal;
        app.input_cursor = app.input.len();

        handle_normal_key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
        assert_eq!(app.input_cursor, 2);

        handle_normal_key(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        assert_eq!(app.vim_mode, VimMode::Insert);
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
