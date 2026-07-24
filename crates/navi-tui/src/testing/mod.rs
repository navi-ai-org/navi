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
pub use crate::event_loop::LeakedTerminalSequenceFilter;
pub use crate::state::{ChatMessage, ChatRole, Mode};
pub use navi_sdk::{
    AgentEvent, AgentRunState, ApprovalDecision, ApprovalRequest, EngineDriver, LoadedConfig,
    NaviConfig, RuntimeEvent, RuntimeEventKind, ToolInvocation, ToolResult,
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
        // Stable paths so screenshot goldens match. Unit tests that need
        // isolation use unique data_dir via `test_app` / custom TestConfig.
        let data_dir = PathBuf::from("/tmp/navi-test-data");
        let project_dir = PathBuf::from("/tmp/navi-test-project");
        let _ = std::fs::create_dir_all(&data_dir);
        let _ = std::fs::create_dir_all(&project_dir);
        Self {
            width: 80,
            height: 24,
            input: String::new(),
            project_dir,
            data_dir,
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

/// Test `LoadedConfig` with network side-effects disabled.
fn test_loaded_config(data_dir: PathBuf) -> LoadedConfig {
    let mut config = NaviConfig::default();
    config.updates.check_enabled = false;
    config.registry.update_enabled = false;
    LoadedConfig {
        config,
        global_config_path: None,
        project_config_path: None,
        data_dir,
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
    /// `config.width × config.height`.
    ///
    /// Uses a [`MockEngine`] so pure UI / screenshot tests skip the real
    /// [`navi_sdk::NaviEngine`] build. Drive the engine via
    /// [`Harness::with_engine`] when you need a custom mock.
    pub fn new(config: TestConfig) -> Self {
        Self::with_engine(config, Arc::new(MockEngine::new()))
    }

    /// Build a harness with a caller-supplied [`EngineDriver`]. The real
    /// engine is **not** built — the harness is initialised with whatever
    /// the caller passes (typically a [`MockEngine`]). This is the entry
    /// point for scenario tests that drive the TUI end-to-end against a
    /// controllable engine.
    pub fn with_engine(config: TestConfig, engine: Arc<dyn EngineDriver>) -> Self {
        let app = TuiApp::new_with_engine(
            test_loaded_config(config.data_dir.clone()),
            config.project_dir.clone(),
            None,
            engine,
        )
        .expect("test app");
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
        let terminal = &mut self.terminal;
        let app = &mut self.app;
        terminal
            .draw(|frame| crate::view::render(frame, app))
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
        let expected_trim = expected.trim_end_matches('\n');
        if actual != expected_trim {
            // Cap mismatch dumps so CI logs / OOM do not hold dual full screens.
            const MAX_DIFF_CHARS: usize = 2_000;
            let trunc = |s: &str| -> String {
                if s.len() <= MAX_DIFF_CHARS {
                    s.to_string()
                } else {
                    format!(
                        "{}\n… truncated {} more chars …",
                        &s[..MAX_DIFF_CHARS],
                        s.len() - MAX_DIFF_CHARS
                    )
                }
            };
            panic!(
                "snapshot mismatch for `{name}`\n\
                 expected file: {}\n\
                 expected len={}, actual len={}\n\
                 \n\
                 --- expected (truncated) ---\n\
                 {}\n\
                 \n--- actual (truncated) ---\n\
                 {}\n\
                 \n\
                 run with UPDATE_SNAPSHOTS=1 to overwrite.",
                path.display(),
                expected_trim.len(),
                actual.len(),
                trunc(expected_trim),
                trunc(&actual),
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

    /// Select a message block by message index (for UI visual tests).
    pub fn select_message_block(&mut self, message_index: usize) -> &mut Self {
        crate::chat_blocks::select_chat_block(
            &mut self.app,
            crate::state::ChatLineSource::Message(message_index),
        );
        self
    }

    /// Select a tool-result block by invocation id (for UI visual tests).
    pub fn select_tool_block(&mut self, invocation_id: impl Into<String>) -> &mut Self {
        crate::chat_blocks::select_chat_block(
            &mut self.app,
            crate::state::ChatLineSource::ToolResult(invocation_id.into()),
        );
        self
    }

    /// Replace the background-command list (for modal visual tests).
    pub fn set_background_commands(
        &mut self,
        commands: Vec<navi_sdk::BackgroundCommandSnapshot>,
    ) -> &mut Self {
        crate::background::replace_background_commands(&mut self.app, commands);
        self
    }

    /// Force-open a tool body by invocation id (user expand).
    pub fn expand_tool(&mut self, id: impl Into<String>) -> &mut Self {
        let id = id.into();
        self.app.collapsed_tool_results.remove(&id);
        self.app.expanded_tool_results.insert(id);
        self.app.chat_render_cache.borrow_mut().signature_hash = 0;
        self
    }

    /// Write the current buffer text to `path` for manual UI review.
    pub fn dump_screen(&self, path: impl AsRef<std::path::Path>) {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, format!("{}\n", self.buffer_text()));
    }

    /// Clear the chat message log.
    pub fn clear_messages(&mut self) -> &mut Self {
        self.app.messages.clear();
        self
    }

    /// Clear model picker rows for snapshots that intentionally assert the
    /// empty-model state independent of the built-in/public provider catalog.
    pub fn clear_models(&mut self) -> &mut Self {
        self.app.models.clear();
        self.app.authenticated_providers.clear();
        self.app.loaded_config.config.tui.recent_model_ids.clear();
        self
    }

    /// Clear saved sessions so session-picker snapshots are independent of
    /// integration-test state left behind in the shared temp data directory.
    pub fn clear_sessions(&mut self) -> &mut Self {
        self.app.saved_sessions.clear();
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
        // Reset request usage so screenshot tests see deterministic elapsed
        // times (e.g. `0ms` instead of flaky `0ms`/`1ms`) on fast machines.
        self.app.usage_state.reset_request_usage();
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

    /// Resize the test terminal to the given dimensions. This simulates a
    /// terminal window resize event (e.g. user resizes the window, or
    /// returns from a minimized/maximized state).
    pub fn resize_terminal(&mut self, width: u16, height: u16) -> &mut Self {
        self.terminal.backend_mut().resize(width, height);
        self
    }

    /// Get a mutable reference to the underlying terminal. Needed for
    /// tests that drive `run_loop` directly.
    pub fn terminal_mut(&mut self) -> &mut ratatui::Terminal<ratatui::backend::TestBackend> {
        &mut self.terminal
    }

    /// Get a mutable reference to the underlying app. Needed for tests
    /// that drive `run_loop` directly.
    pub fn app_mut(&mut self) -> &mut TuiApp {
        &mut self.app
    }

    /// Drive the TUI event loop with a `VecInput` that has already been
    /// populated with events. Unlike `drive_loop`, this does NOT append
    /// a Ctrl+C quit event — the input source's `stop_when_empty` setting
    /// controls when the loop exits.
    pub fn drive_loop_with_input(&mut self, input: &mut VecInput) -> &mut Self {
        let _ = crate::event_loop::run_loop(&mut self.terminal, &mut self.app, input);
        self
    }
}

fn snapshot_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.txt"))
}
