//! Internal testing harness for the TUI.
//!
//! This module is `pub` so integration tests in `tests/` can drive the TUI
//! through its real input and rendering path. It is marked `#[doc(hidden)]`
//! because it is not a stable consumer-facing API.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::TuiApp;

// Re-exports for the public test surface.
pub use crate::dispatch::AsyncEvent;
pub use crate::state::{ChatMessage, ChatRole, Mode};
pub use navi_sdk::{
    AgentEvent, AgentMode, AgentRunState, ApprovalDecision, ApprovalRequest, EngineDriver,
    LoadedConfig, NaviConfig, RuntimeEvent, RuntimeEventKind, ToolInvocation, ToolResult,
};

mod mock_engine;
pub use mock_engine::{EngineCall, MockEngine};

mod vec_input;
pub use vec_input::VecInput;

/// Configuration for constructing a [`Harness`].
#[derive(Clone, Debug)]
pub struct TestConfig {
    pub width: u16,
    pub height: u16,
    pub input: String,
    pub project_dir: PathBuf,
    pub data_dir: PathBuf,
    pub git_branch: Option<String>,
    pub context_window: u64,
    pub yolo_mode: bool,
    pub full_tool_view: bool,
    pub show_thinking: bool,
    pub provider_configured: bool,
    pub session_id: String,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            width: 80,
            height: 24,
            input: String::new(),
            project_dir: PathBuf::from("/tmp/navi-test-project"),
            data_dir: PathBuf::from("/tmp/navi-test-data"),
            git_branch: Some("main".to_string()),
            context_window: 200_000,
            yolo_mode: false,
            full_tool_view: false,
            show_thinking: false,
            provider_configured: true,
            session_id: "session-test".to_string(),
        }
    }
}

/// A test harness for the TUI: owns a [`TuiApp`] and a [`TestBackend`] terminal,
/// and provides high-level operations for driving the TUI through its real
/// input and rendering path.
pub struct Harness {
    terminal: Terminal<TestBackend>,
    app: TuiApp,
}

impl Harness {
    /// Build a harness with the given config. The terminal is sized
    /// `config.width × config.height`. The real [`NaviEngine`] is built and
    /// installed; tests that need to drive the engine should use
    /// [`Harness::with_engine`] instead.
    pub fn new(config: TestConfig) -> Self {
        let app = TuiApp::new(
            LoadedConfig {
                config: NaviConfig::default(),
                global_config_path: None,
                project_config_path: None,
                data_dir: config.data_dir.clone(),
            },
            config.project_dir.clone(),
            None,
        )
        .expect("test app");
        Self::from_app(config, app)
    }

    /// Build a harness with a caller-supplied [`EngineDriver`]. The real
    /// engine is **not** built — the harness is initialised with whatever
    /// the caller passes (typically a [`MockEngine`]). This is the entry
    /// point for scenario tests that drive the TUI end-to-end against a
    /// controllable engine.
    pub fn with_engine(config: TestConfig, engine: Arc<dyn EngineDriver>) -> Self {
        let mut app = TuiApp::new(
            LoadedConfig {
                config: NaviConfig::default(),
                global_config_path: None,
                project_config_path: None,
                data_dir: config.data_dir.clone(),
            },
            config.project_dir.clone(),
            None,
        )
        .expect("test app");
        app.set_engine(engine);
        Self::from_app(config, app)
    }

    fn from_app(config: TestConfig, mut app: TuiApp) -> Self {
        app.git_branch = config.git_branch;
        app.compact_state.context_window = config.context_window;
        app.provider_configured = config.provider_configured;
        app.yolo_mode = config.yolo_mode;
        app.full_tool_view = config.full_tool_view;
        app.show_thinking = config.show_thinking;
        app.input = config.input.clone();
        app.input_cursor = config.input.len();
        app.mode = Mode::Normal;
        app.session_id = navi_core::SessionId::new(config.session_id);

        let backend = TestBackend::new(config.width, config.height);
        let terminal = Terminal::new(backend).expect("terminal");
        Self { terminal, app }
    }

    /// Width of the test terminal.
    pub fn width(&self) -> u16 {
        self.terminal.backend().buffer().area.width
    }

    /// Height of the test terminal.
    pub fn height(&self) -> u16 {
        self.terminal.backend().buffer().area.height
    }

    /// Send a single key event through the real keybinding layer.
    pub fn press(&mut self, key: KeyCode, modifiers: KeyModifiers) -> &mut Self {
        crate::keybindings::handle_key(&mut self.app, key, modifiers);
        self
    }

    /// Type a string as a sequence of character keypresses.
    pub fn type_text(&mut self, text: &str) -> &mut Self {
        for ch in text.chars() {
            crate::keybindings::handle_key(&mut self.app, KeyCode::Char(ch), KeyModifiers::NONE);
        }
        self
    }

    /// Inject an [`AsyncEvent`] from the async bridge into the app.
    pub fn inject(&mut self, event: AsyncEvent) -> &mut Self {
        crate::dispatch::handle_async_event(&mut self.app, event);
        self
    }

    /// Render the current app state to the test terminal buffer.
    pub fn render(&mut self) -> &mut Self {
        self.terminal
            .draw(|frame| crate::view::render(frame, &self.app))
            .expect("draw");
        self
    }

    /// Return the rendered screen as plain text (one row per line, trailing
    /// whitespace stripped). Does not include ANSI/styling.
    pub fn buffer_text(&self) -> String {
        let buffer = self.terminal.backend().buffer();
        let area = buffer.area;
        let mut out = String::new();
        for y in 0..area.height {
            let mut line = String::new();
            for x in 0..area.width {
                if let Some(cell) = buffer.cell((x, y)) {
                    line.push_str(cell.symbol());
                }
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out.trim_end_matches('\n').to_string()
    }

    /// Return the symbol at `(x, y)`, or `None` if out of bounds.
    pub fn cell(&self, x: u16, y: u16) -> Option<String> {
        self.terminal
            .backend()
            .buffer()
            .cell((x, y))
            .map(|cell| cell.symbol().to_string())
    }

    /// Assert that the current buffer matches the golden snapshot at
    /// `tests/snapshots/{name}.txt`. If the file does not exist, it is created
    /// (and the test passes with a notice). To overwrite an existing snapshot,
    /// run with `UPDATE_SNAPSHOTS=1` in the environment.
    pub fn assert_screen(&self, name: &str) {
        let actual = self.buffer_text();
        let path = snapshot_path(name);
        let update = std::env::var("UPDATE_SNAPSHOTS").is_ok();

        if update || !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::write(&path, format!("{actual}\n")).expect("write snapshot");
            if !update {
                eprintln!("[snapshot] wrote new golden: {}", path.display());
            }
            return;
        }

        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing snapshot file: {}", path.display()));
        if actual != expected.trim_end_matches('\n') {
            panic!(
                "snapshot mismatch for `{name}`\n\
                 expected file: {}\n\
                 \n\
                 --- expected ---\n\
                 {expected}\
                 \n--- actual ---\n\
                 {actual}\n\
                 \n\
                 run with UPDATE_SNAPSHOTS=1 to overwrite.",
                path.display(),
            );
        }
    }

    /// Current modal mode.
    pub fn mode(&self) -> Mode {
        self.app.mode
    }

    /// Snapshot of the chat message log.
    pub fn messages(&self) -> &[ChatMessage] {
        &self.app.messages
    }

    /// True when a model stream task is in flight.
    pub fn is_loading(&self) -> bool {
        self.app.is_loading
    }

    /// Current input buffer.
    pub fn input(&self) -> &str {
        &self.app.input
    }

    /// Pending tool approval requests.
    pub fn pending_approvals(&self) -> &[ApprovalRequest] {
        &self.app.pending_approvals
    }

    /// Whether the app currently has a stream or tool task running.
    pub fn has_async_task(&self) -> bool {
        self.app.has_async_task()
    }

    /// Number of messages in the conversation history.
    pub fn conversation_history_len(&self) -> usize {
        self.app.conversation_history.len()
    }

    // ---- state helpers (pub because fields on TuiApp are pub(crate)) ----

    /// Set the loading state. When transitioning to `true`, the loading start
    /// is recorded; when transitioning to `false`, it is cleared.
    pub fn set_loading(&mut self, loading: bool) -> &mut Self {
        self.app.is_loading = loading;
        self.app.loading_start = if loading { Some(Instant::now()) } else { None };
        self
    }

    /// Push a chat message into the log.
    pub fn push_message(&mut self, msg: ChatMessage) -> &mut Self {
        self.app.messages.push(msg);
        self
    }

    /// Clear the chat message log.
    pub fn clear_messages(&mut self) -> &mut Self {
        self.app.messages.clear();
        self
    }

    /// Simulate the start of a streaming model response: push a user message
    /// and a thinking-placeholder assistant message, and set the loading state.
    pub fn begin_thinking_response(&mut self, user_text: &str) -> &mut Self {
        self.app
            .messages
            .push(ChatMessage::new(ChatRole::User, user_text.to_string()));
        let model_label = self.app.loaded_config.config.model.name.clone();
        self.app.messages.push(ChatMessage {
            model_label: Some(model_label),
            status: Some("thinking".to_string()),
            ..ChatMessage::new(ChatRole::Assistant, String::new())
        });
        self.set_loading(true)
    }

    /// Drain all pending [`AsyncEvent`]s from the TUI's async bridge and
    /// process them through the dispatch handler. Returns the number of
    /// events processed.
    pub fn drain_async_events(&mut self) -> usize {
        let mut count = 0;
        while let Some(event) = self.app.try_recv_async_event() {
            crate::dispatch::handle_async_event(&mut self.app, event);
            count += 1;
        }
        count
    }

    /// Submit the current input as a user message. This is the same path as
    /// pressing `ctrl+enter` in the real TUI; it spawns an async turn task on
    /// the current tokio runtime.
    ///
    /// For end-to-end tests, pair this with a `#[tokio::test]` harness so the
    /// spawned task actually runs.
    pub fn submit(&mut self) -> &mut Self {
        crate::chat::submit_message(&mut self.app);
        self
    }

    /// Drive the real TUI event loop with a scripted sequence of input
    /// events. The loop runs to completion, ending when the user sends a
    /// quit key (or when the events list is exhausted and the TUI is
    /// idle, depending on the input source's `stop_when_empty` setting).
    ///
    /// The loop draws to the harness's `TestBackend` terminal on every
    /// iteration, so you can `assert_screen` after this call.
    pub fn drive_loop(&mut self, events: Vec<crossterm::event::Event>) -> &mut Self {
        let mut input = VecInput::new();
        for e in events {
            input.push(e);
        }
        // Always end with ctrl+c so the loop has a deterministic exit.
        input.push(crossterm::event::Event::Key(
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('c'),
                crossterm::event::KeyModifiers::CONTROL,
            ),
        ));
        let _ = crate::event_loop::run_loop(&mut self.terminal, &mut self.app, &mut input);
        self
    }
}

fn snapshot_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.txt"))
}
