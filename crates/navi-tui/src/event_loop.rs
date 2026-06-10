use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind, KeyboardEnhancementFlags};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use ratatui::backend::Backend;
use ratatui::prelude::{CrosstermBackend, Terminal};

use crate::app::TuiApp;
use crate::chat::submit_message;
use crate::dispatch::handle_async_event;
use crate::input::{insert_api_key_text, insert_input_text};
use crate::keybindings::handle_key;
use crate::mouse::handle_mouse;
use crate::notifications::{expire_notification, visible_notification};
use crate::persistence::{save_current_session, save_preferences};
use crate::state::Mode;
use crate::view::render;

// ─── input source abstraction ──────────────────────────────────────────────
//
// Production uses [`CrosstermInput`] (wraps `crossterm::event::poll`/`read`).
// Tests in `crate::testing` plug in a `VecInput` that yields a queued
// sequence of events, so the loop can be driven deterministically against
// a `TestBackend` terminal.
pub trait InputSource {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool>;
    fn read(&mut self) -> io::Result<Event>;
}

/// Production input source backed by `crossterm::event`.
pub struct CrosstermInput;

impl InputSource for CrosstermInput {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        event::poll(timeout)
    }

    fn read(&mut self) -> io::Result<Event> {
        event::read()
    }
}

// ─── entry point (sync — no nested runtime) ────────────────────────────────────
// The caller (navi-cli `#[tokio::main]`) already owns a multi-thread tokio
// runtime, so `tokio::spawn` works from inside this synchronous event loop.
// We must NOT create a second runtime here.
pub fn run(app: TuiApp) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;

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

    let mut input = CrosstermInput;
    let mut app = app;
    let result = run_loop(&mut terminal, &mut app, &mut input);

    // Restore keyboard mode before leaving.
    if enhanced_keyboard {
        execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags)?;
    }
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    disable_raw_mode()?;
    terminal.show_cursor()?;

    result
}

/// The TUI's main loop, factored out so it can be tested with a `TestBackend`
/// and an in-memory input source.
///
/// Exits when [`crate::keybindings::handle_key`] signals quit (returns `true`)
/// or when the harness's drive-loop helper decides the test has run long
/// enough.
pub fn run_loop<B, I>(terminal: &mut Terminal<B>, app: &mut TuiApp, input: &mut I) -> Result<()>
where
    B: Backend,
    <B as Backend>::Error: Sync + Send + 'static,
    I: InputSource,
{
    // If input was pre-filled from CLI task, submit on first frame
    if !app.input.trim().is_empty() && app.mode == Mode::Normal {
        submit_message(app);
    }

    let mut needs_draw = true;
    loop {
        if needs_draw {
            terminal.draw(|frame| render(frame, app))?;
            app.advance_tick();
            needs_draw = false;
        }

        if expire_notification(app) {
            needs_draw = true;
        }

        // Check for async model stream events (non-blocking)
        while let Some(event) = app.try_recv_async_event() {
            needs_draw = true;
            handle_async_event(app, event);
        }

        let timeout = if app.is_loading {
            Duration::from_millis(16)
        } else if app.messages.is_empty() || visible_notification(app).is_some() {
            Duration::from_millis(80)
        } else {
            Duration::from_millis(250)
        };

        if input.poll(timeout)? {
            match input.read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    needs_draw = true;
                    if handle_key(app, key.code, key.modifiers) {
                        break;
                    }
                }
                Event::Mouse(mouse_event) => {
                    needs_draw = true;
                    handle_mouse(app, mouse_event);
                }
                Event::Paste(content) => {
                    needs_draw = true;
                    handle_paste(app, &content);
                }
                _ => {}
            }
        } else if app.is_loading || app.messages.is_empty() || visible_notification(app).is_some() {
            needs_draw = true;
        }
    }

    save_current_session(app);
    save_preferences(app);

    Ok(())
}

/// Handle a bracketed paste event by inserting the full text into the active
/// input field.
fn handle_paste(app: &mut TuiApp, content: &str) {
    match app.mode {
        Mode::Normal => insert_input_text(app, content),
        Mode::ApiKeyEntry => insert_api_key_text(app, content),
        _ => {}
    }
}
