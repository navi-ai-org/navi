use std::io;
use std::panic;
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind,
    PopKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
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
        maybe_refresh_terminal_input_modes();
        event::poll(timeout)
    }

    fn read(&mut self) -> io::Result<Event> {
        event::read()
    }
}

struct TerminalModeGuard {
    active: bool,
}

impl TerminalModeGuard {
    fn enter() -> Result<Self> {
        let mut guard = Self { active: false };
        enable_raw_mode()?;
        guard.active = true;
        let mut stdout = io::stdout();
        if let Err(err) = enter_terminal_modes(&mut stdout) {
            let _ = guard.restore();
            return Err(err.into());
        }
        Ok(guard)
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        restore_terminal_modes_best_effort()
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

// ─── entry point (sync — no nested runtime) ────────────────────────────────────
// The caller (navi-cli `#[tokio::main]`) already owns a multi-thread tokio
// runtime, so `tokio::spawn` works from inside this synchronous event loop.
// We must NOT create a second runtime here.
pub fn run(app: TuiApp) -> Result<()> {
    install_terminal_restore_panic_hook();
    let mut terminal_modes = TerminalModeGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut input = CrosstermInput;
    let mut app = app;
    let result = run_loop(&mut terminal, &mut app, &mut input);

    let cursor_result = terminal.show_cursor();
    let restore_result = terminal_modes.restore();

    cursor_result?;
    restore_result?;

    result
}

fn enter_terminal_modes(w: &mut impl io::Write) -> io::Result<()> {
    reset_terminal_input_modes(w)?;
    execute!(w, EnterAlternateScreen, EnableBracketedPaste)?;
    enable_mouse_capture(w)
}

fn reset_terminal_input_modes(w: &mut impl io::Write) -> io::Result<()> {
    disable_mouse_capture(w)?;
    execute!(w, DisableBracketedPaste, PopKeyboardEnhancementFlags)?;
    // Reset xterm modifyOtherKeys and Kitty keyboard protocol as well; a
    // previous run or child process may have left the terminal encoding normal
    // control keys as escape sequences instead of key events.
    write!(w, "\x1B[>4;0m\x1B[<u")?;
    w.flush()
}

/// Enable mouse clicks/scroll with SGR coordinates, but without motion/drag
/// tracking. The TUI only needs click/scroll events for interactions.
fn enable_mouse_capture(w: &mut impl io::Write) -> io::Result<()> {
    // Normal tracking: report button press/release (?1000h)
    // SGR extended coordinates: supports >223 columns/rows (?1006h)
    // Intentionally omitting ?1002h/?1003h because the TUI does not need
    // mouse-motion events.
    write!(w, "\x1B[?1000h\x1B[?1006h")?;
    w.flush()
}

/// Disable mouse modes defensively.
fn disable_mouse_capture(w: &mut impl io::Write) -> io::Result<()> {
    // Disable every common mouse mode defensively in case a previous session
    // or a child process left the terminal in motion/focus tracking.
    write!(
        w,
        "\x1B[?9l\x1B[?1000l\x1B[?1001l\x1B[?1002l\x1B[?1003l\x1B[?1004l\x1B[?1005l\x1B[?1006l\x1B[?1015l"
    )?;
    w.flush()
}

fn restore_terminal_modes_best_effort() -> io::Result<()> {
    let mut first_error = None;
    let mut stdout = io::stdout();

    remember_error(&mut first_error, reset_terminal_input_modes(&mut stdout));
    remember_error(
        &mut first_error,
        execute!(stdout, LeaveAlternateScreen, DisableBracketedPaste),
    );
    remember_error(&mut first_error, disable_raw_mode());

    match first_error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

fn remember_error(slot: &mut Option<io::Error>, result: io::Result<()>) {
    if let Err(err) = result
        && slot.is_none()
    {
        *slot = Some(err);
    }
}

static TERMINAL_RESTORE_PANIC_HOOK: Once = Once::new();

fn install_terminal_restore_panic_hook() {
    TERMINAL_RESTORE_PANIC_HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let _ = restore_terminal_modes_best_effort();
            previous(info);
        }));
    });
}

const TERMINAL_MODE_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
static LAST_TERMINAL_MODE_REFRESH: OnceLock<Mutex<Instant>> = OnceLock::new();

fn maybe_refresh_terminal_input_modes() {
    let now = Instant::now();
    let last = LAST_TERMINAL_MODE_REFRESH.get_or_init(|| Mutex::new(now));
    let Ok(mut last) = last.try_lock() else {
        return;
    };
    if now.duration_since(*last) < TERMINAL_MODE_REFRESH_INTERVAL {
        return;
    }

    let mut stdout = io::stdout();
    let _ = enable_raw_mode();
    let _ = reset_terminal_input_modes(&mut stdout);
    let _ = execute!(stdout, EnableBracketedPaste);
    let _ = enable_mouse_capture(&mut stdout);
    *last = now;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_capture_does_not_enable_motion_tracking() {
        let mut out = Vec::new();

        enable_mouse_capture(&mut out).expect("enable mouse capture");
        let text = String::from_utf8(out).expect("utf8 escape sequences");

        assert!(text.contains("\x1B[?1000h"));
        assert!(text.contains("\x1B[?1006h"));
        assert!(!text.contains("\x1B[?1002h"));
        assert!(!text.contains("\x1B[?1003h"));
    }

    #[test]
    fn mouse_capture_disable_clears_motion_tracking_modes() {
        let mut out = Vec::new();

        disable_mouse_capture(&mut out).expect("disable mouse capture");
        let text = String::from_utf8(out).expect("utf8 escape sequences");

        assert!(text.contains("\x1B[?9l"));
        assert!(text.contains("\x1B[?1003l"));
        assert!(text.contains("\x1B[?1002l"));
        assert!(text.contains("\x1B[?1004l"));
        assert!(text.contains("\x1B[?1000l"));
    }

    #[test]
    fn terminal_input_reset_clears_keyboard_protocols() {
        let mut out = Vec::new();

        reset_terminal_input_modes(&mut out).expect("reset terminal input modes");
        let text = String::from_utf8(out).expect("utf8 escape sequences");

        assert!(text.contains("\x1B[?1003l"));
        assert!(text.contains("\x1B[?1002l"));
        assert!(text.contains("\x1B[>4;0m"));
        assert!(text.contains("\x1B[<u"));
    }
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
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
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

/// Handle a bracketed paste event. In normal mode, tries to read an image
/// from the clipboard first (Ctrl+V paste); falls back to inserting text.
fn handle_paste(app: &mut TuiApp, content: &str) {
    use crate::clipboard::try_read_clipboard_image;
    use crate::notifications::show_notification;

    match app.mode {
        Mode::Normal => {
            if !app.is_loading {
                if let Some(image) =
                    crate::clipboard::try_read_image_from_path(app.image_picker.as_ref(), content)
                {
                    let label = image.label();
                    app.pending_images.push(image);
                    show_notification(
                        app,
                        "Image",
                        format!("{} attached via drag and drop", label),
                    );
                    return;
                }

                if let Some(image) = try_read_clipboard_image(app.image_picker.as_ref()) {
                    let label = image.label();
                    app.pending_images.push(image);
                    show_notification(app, "Image", format!("{} attached via paste", label));
                }
            }
            if !content.is_empty() {
                insert_input_text(app, content);
            }
        }
        Mode::ApiKeyEntry => insert_api_key_text(app, content),
        _ => {}
    }
}
