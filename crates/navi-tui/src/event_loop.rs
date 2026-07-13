use std::io;
use std::panic;
use std::sync::Once;
use std::time::Duration;

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
    // Probe Kitty/Sixel/iTerm2 after alternate-screen entry (before event reads).
    crate::view::terminal_graphics::install_session_graphics(
        crate::view::terminal_graphics::TerminalGraphics::detect(),
    );
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
    disable_extended_keyboard_protocols(w)?;
    enable_focus_tracking(w)?;
    enable_mouse_capture(w)
}

/// Aggressively disable all extended keyboard protocols that can corrupt
/// input parsing. Terminals like Kitty, WezTerm, and Ghostty enable enhanced
/// keyboard protocols by default or inherit them from the parent shell. When
/// these protocols are active, key events are encoded as CSI sequences
/// (e.g. `CSI 99;133u` for the 'c' key) that crossterm cannot parse, causing
/// raw protocol bytes to leak into the TUI as garbage characters.
///
/// This function sends every known disable sequence:
/// - `\x1B[>0u`  — set Kitty keyboard flags to 0 (full disable)
/// - `\x1B[<u`   — pop Kitty keyboard flags (belt-and-suspenders)
/// - `\x1B[>4;0m` — disable xterm modifyOtherKeys
/// - `\x1B[?2004h` is NOT sent (we enable bracketed paste separately)
fn disable_extended_keyboard_protocols(w: &mut impl io::Write) -> io::Result<()> {
    write!(w, "\x1B[>4;0m\x1B[>0u\x1B[<u")?;
    w.flush()
}

/// Enable terminal focus tracking (DEC 1004). When the user returns to the
/// NAVI window, the terminal sends `FocusGained` (`\x1B[I`), which we use
/// to re-disable extended keyboard protocols that the terminal may have
/// re-activated while we were away.
fn enable_focus_tracking(w: &mut impl io::Write) -> io::Result<()> {
    write!(w, "\x1B[?1004h")?;
    w.flush()
}

fn reset_terminal_input_modes(w: &mut impl io::Write) -> io::Result<()> {
    disable_mouse_capture(w)?;
    execute!(w, DisableBracketedPaste, PopKeyboardEnhancementFlags)?;
    disable_extended_keyboard_protocols(w)
}

/// Enable mouse clicks/scroll/drag and free motion for image-chip hover.
fn enable_mouse_capture(w: &mut impl io::Write) -> io::Result<()> {
    // Normal tracking: report button press/release (?1000h)
    // Button-event tracking: report drag while a button is held (?1002h)
    // Any-event tracking: free mouse-motion for image hover open/leave (?1003h)
    // SGR extended coordinates: supports >223 columns/rows (?1006h)
    //
    // ?1003 is required so leaving `[Image N]` can close the lightbox. Motion
    // handlers must stay cheap (only redraw when hover state actually changes).
    write!(w, "\x1B[?1000h\x1B[?1002h\x1B[?1003h\x1B[?1006h")?;
    w.flush()
}

/// Disable mouse modes defensively.
fn disable_mouse_capture(w: &mut impl io::Write) -> io::Result<()> {
    // Disable every common mouse mode defensively in case a previous session
    // or a child process left the terminal in motion/focus tracking.
    // Also disable focus tracking (?1004) here since it's a defensive reset.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_capture_enables_drag_and_free_motion_for_hover() {
        let mut out = Vec::new();

        enable_mouse_capture(&mut out).expect("enable mouse capture");
        let text = String::from_utf8(out).expect("utf8 escape sequences");

        assert!(text.contains("\x1B[?1000h"));
        assert!(text.contains("\x1B[?1002h"));
        assert!(text.contains("\x1B[?1003h"));
        assert!(text.contains("\x1B[?1006h"));
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

    #[test]
    fn leaked_terminal_sequence_filter_drops_sgr_mouse_reports() {
        let mut filter = LeakedTerminalSequenceFilter::default();

        for ch in "\u{1b}[<0;49;31M".chars() {
            assert!(filter.should_drop(&crossterm::event::KeyCode::Char(ch)));
        }

        assert!(!filter.should_drop(&crossterm::event::KeyCode::Char('a')));
    }

    #[test]
    fn leaked_terminal_sequence_filter_drops_rxvt_mouse_reports() {
        let mut filter = LeakedTerminalSequenceFilter::default();

        for ch in "\u{1b}[32;45;31M".chars() {
            assert!(filter.should_drop(&crossterm::event::KeyCode::Char(ch)));
        }

        assert!(!filter.should_drop(&crossterm::event::KeyCode::Char('a')));
    }

    #[test]
    fn leaked_terminal_sequence_filter_ignores_normal_input() {
        let mut filter = LeakedTerminalSequenceFilter::default();

        assert!(!filter.should_drop(&crossterm::event::KeyCode::Char('[')));
        assert!(!filter.should_drop(&crossterm::event::KeyCode::Char('<')));
        assert!(!filter.should_drop(&crossterm::event::KeyCode::Char('M')));
    }

    #[test]
    fn leaked_terminal_sequence_filter_drops_osc_with_st_terminator() {
        let mut filter = LeakedTerminalSequenceFilter::default();

        // OSC 11 response terminated with ST (ESC \)
        for ch in "\u{1b}]11;rgb:1a1a/1b1b/2626\u{1b}\\".chars() {
            assert!(
                filter.should_drop(&crossterm::event::KeyCode::Char(ch)),
                "OSC char '{}' should be dropped",
                ch
            );
        }

        assert!(!filter.should_drop(&crossterm::event::KeyCode::Char('a')));
    }

    #[test]
    fn leaked_terminal_sequence_filter_drops_osc_with_bel_terminator() {
        let mut filter = LeakedTerminalSequenceFilter::default();

        // OSC title set terminated with BEL
        for ch in "\u{1b}]0;title\u{07}".chars() {
            assert!(
                filter.should_drop(&crossterm::event::KeyCode::Char(ch)),
                "OSC char '{}' should be dropped",
                ch
            );
        }

        assert!(!filter.should_drop(&crossterm::event::KeyCode::Char('a')));
    }

    #[test]
    fn leaked_terminal_sequence_filter_drops_ss3_sequences() {
        let mut filter = LeakedTerminalSequenceFilter::default();

        for ch in "\u{1b}OA".chars() {
            assert!(
                filter.should_drop(&crossterm::event::KeyCode::Char(ch)),
                "SS3 char '{}' should be dropped",
                ch
            );
        }

        assert!(!filter.should_drop(&crossterm::event::KeyCode::Char('a')));
    }

    #[test]
    fn terminal_input_reset_fully_disables_kitty_keyboard() {
        let mut out = Vec::new();

        reset_terminal_input_modes(&mut out).expect("reset terminal input modes");
        let text = String::from_utf8(out).expect("utf8 escape sequences");

        // \x1B[>0u sets Kitty flags to 0 (full disable)
        assert!(text.contains("\x1B[>0u"));
        // \x1B[<u pops one level (belt-and-suspenders)
        assert!(text.contains("\x1B[<u"));
    }

    #[test]
    fn enter_terminal_modes_disables_kitty_keyboard_on_entry() {
        let mut out = Vec::new();

        enter_terminal_modes(&mut out).expect("enter terminal modes");
        let text = String::from_utf8(out).expect("utf8 escape sequences");

        // Kitty keyboard protocol must be disabled when entering the TUI,
        // not just on exit. Otherwise crossterm cannot parse key events
        // encoded as CSI 99;133u and raw bytes leak into the input field.
        assert!(
            text.contains("\x1B[>0u"),
            "enter_terminal_modes must disable Kitty keyboard protocol on entry"
        );
    }

    #[test]
    fn leaked_filter_does_not_swallow_bare_escape_key() {
        let mut filter = LeakedTerminalSequenceFilter::default();

        // KeyCode::Esc means the user pressed Escape — it must NOT be
        // swallowed by the filter, otherwise the next keypress would be
        // consumed as part of a "leaked sequence".
        assert!(
            !filter.should_drop(&crossterm::event::KeyCode::Esc),
            "KeyCode::Esc is a real Escape press, not a leaked sequence"
        );
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
    let mut leaked_terminal_sequence_filter = LeakedTerminalSequenceFilter::default();
    loop {
        // composer expand/collapse animation.
        let input_width = terminal
            .size()
            .map(|s| s.width.saturating_sub(4) as usize)
            .unwrap_or(80);
        let composer_animating = crate::view::input::advance_composer_animation(app, input_width);
        // Pulse ◆/◇ for in-flight tools, turn loading, and background commands.
        // Without this the event loop only redraws on input/events, so the
        // running diamond freezes on the first frame.
        let bg_running = app.background_commands.iter().any(|c| c.is_running());
        let activity_animating = app.is_loading
            || !app.running_tools.is_empty()
            || bg_running
            || (matches!(
                app.mode,
                crate::state::Mode::BackgroundCommands
                    | crate::state::Mode::BackgroundCommandOutput
            ) && bg_running);

        if needs_draw || composer_animating || activity_animating {
            terminal.draw(|frame| render(frame, app))?;
            app.advance_tick();
            needs_draw = false;
        }

        if expire_notification(app) {
            needs_draw = true;
        }

        if crate::view::image_preview::poll_image_hover_close(app) {
            needs_draw = true;
        }

        // Check for async model stream events (non-blocking)
        while let Some(event) = app.try_recv_async_event() {
            needs_draw = true;
            handle_async_event(app, event);
        }

        let mut timeout = if activity_animating || composer_animating {
            // ~30fps is enough for a 320ms pulse frame and keeps CPU low.
            Duration::from_millis(33)
        } else if app.messages.is_empty() || visible_notification(app).is_some() {
            Duration::from_millis(80)
        } else {
            Duration::from_millis(250)
        };
        // Wake promptly to apply the image-hover leave grace period.
        if let Some(deadline) = app.image_hover_close_deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining < timeout {
                timeout = remaining.max(Duration::from_millis(16));
            }
        }

        if input.poll(timeout)? {
            match input.read()? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    needs_draw = true;
                    // Some terminals emit spurious KeyCode::Esc as part of
                    // mouse sequence parsing during rapid clicks (especially
                    // double-clicks). If an Esc arrives within 150ms of a
                    // mouse event, swallow it so it doesn't open the
                    // ConfirmCancelTurn modal and cancel the active turn.
                    if key.code == crossterm::event::KeyCode::Esc
                        && app.last_mouse_event.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(150))
                    {
                        app.last_mouse_event = None;
                        continue;
                    }
                    if leaked_terminal_sequence_filter.should_drop(&key.code) {
                        continue;
                    }
                    if handle_key(app, key.code, key.modifiers) {
                        break;
                    }
                }
                Event::Mouse(mouse_event) => {
                    app.last_mouse_event = Some(std::time::Instant::now());
                    // Free-motion (?1003) can fire often; only redraw when UI state changes.
                    if handle_mouse(app, mouse_event) {
                        needs_draw = true;
                    }
                }
                Event::Paste(content) => {
                    needs_draw = true;
                    handle_paste(app, &content);
                }
                Event::Resize(_, _) => {
                    needs_draw = true;
                }
                Event::FocusGained => {
                    // The user returned to the NAVI window. Force a full
                    // redraw to recover from any terminal state corruption
                    // that happened while we were away (alternate screen
                    // may have been partially overwritten, cursor position
                    // may be wrong, etc).
                    leaked_terminal_sequence_filter.reset();
                    terminal.clear()?;
                    needs_draw = true;
                }
                Event::FocusLost => {
                    leaked_terminal_sequence_filter.reset();
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

#[derive(Default)]
pub struct LeakedTerminalSequenceFilter {
    state: LeakedTerminalSequenceState,
    bytes_seen: usize,
}

#[derive(Default, PartialEq)]
enum LeakedTerminalSequenceState {
    #[default]
    Idle,
    Escape,
    Csi,
    Osc,
    OscEsc,
    Ss3,
}

impl LeakedTerminalSequenceFilter {
    pub fn should_drop(&mut self, code: &crossterm::event::KeyCode) -> bool {
        // Only KeyCode::Char can carry leaked protocol bytes. KeyCode::Esc
        // is a structured event from crossterm — it means the user actually
        // pressed Escape, not that a sequence leaked. Treating Esc as the
        // start of a leaked sequence would swallow legitimate Escape presses
        // and then consume the next real keypress as "part of the sequence".
        let ch: char = match code {
            crossterm::event::KeyCode::Char(c) => *c,
            _ => {
                // If we're in the middle of consuming a leaked sequence,
                // drop any non-char event too — crossterm may have partially
                // parsed a corrupted sequence into a structured key (e.g.
                // Home, Left). Letting these through causes cursor jumping.
                if self.state != LeakedTerminalSequenceState::Idle {
                    self.bytes_seen += 1;
                    let overlong = self.bytes_seen > 256;
                    if overlong {
                        self.reset();
                    }
                    return true;
                }
                self.reset();
                return false;
            }
        };

        match self.state {
            LeakedTerminalSequenceState::Idle => {
                if ch == '\u{1b}' {
                    self.state = LeakedTerminalSequenceState::Escape;
                    self.bytes_seen = 1;
                    return true;
                }
                false
            }
            LeakedTerminalSequenceState::Escape => {
                self.bytes_seen += 1;
                if ch == '[' {
                    self.state = LeakedTerminalSequenceState::Csi;
                    true
                } else if ch == ']' {
                    self.state = LeakedTerminalSequenceState::Osc;
                    true
                } else if ch == 'O' {
                    self.state = LeakedTerminalSequenceState::Ss3;
                    true
                } else if ch == 'P' {
                    // DCS (Device Control String) — consume until ST or BEL
                    self.state = LeakedTerminalSequenceState::Osc;
                    true
                } else {
                    self.reset();
                    false
                }
            }
            LeakedTerminalSequenceState::Csi => {
                self.bytes_seen += 1;
                let finished = ('@'..='~').contains(&ch);
                let overlong = self.bytes_seen > 64;
                if finished || overlong {
                    self.reset();
                }
                true
            }
            LeakedTerminalSequenceState::Osc => {
                self.bytes_seen += 1;
                // OSC terminates with BEL (\x07) or ST (ESC \)
                let overlong = self.bytes_seen > 256;
                if ch == '\u{07}' {
                    self.reset();
                } else if ch == '\u{1b}' {
                    self.state = LeakedTerminalSequenceState::OscEsc;
                } else if overlong {
                    self.reset();
                }
                true
            }
            LeakedTerminalSequenceState::OscEsc => {
                self.bytes_seen += 1;
                // We saw ESC inside an OSC. The next char should be '\'
                // (completing ST = ESC \). Regardless of what it is, the
                // OSC is now done — either it was the ST terminator or a
                // broken sequence. Reset and drop.
                self.reset();
                true
            }
            LeakedTerminalSequenceState::Ss3 => {
                self.bytes_seen += 1;
                // SS3 sequences are exactly one char after 'O' (e.g. \x1bOA)
                self.reset();
                true
            }
        }
    }

    fn reset(&mut self) {
        self.state = LeakedTerminalSequenceState::Idle;
        self.bytes_seen = 0;
    }
}

/// Handle a bracketed paste event. In normal mode, tries to read an image
/// from the clipboard first (Ctrl+V paste); falls back to inserting text.
fn handle_paste(app: &mut TuiApp, content: &str) {
    use crate::clipboard::try_read_clipboard_image;
    use crate::notifications::show_notification;

    match app.mode {
        Mode::Normal => {
            if !app.is_loading {
                if let Some(image) = crate::clipboard::try_read_image_from_path(content) {
                    app.pending_images.push(image);
                    let tag = format!("[Image {}]", app.pending_images.len());
                    insert_input_text(app, &tag);
                    show_notification(app, "Image", format!("Attached as {}", tag));
                    return;
                }

                if let Some(image) = try_read_clipboard_image() {
                    app.pending_images.push(image);
                    let tag = format!("[Image {}]", app.pending_images.len());
                    insert_input_text(app, &tag);
                    show_notification(app, "Image", format!("Attached as {}", tag));
                    return;
                }
            }
            if !content.is_empty() {
                insert_input_text(app, content);
            }
        }
        Mode::ApiKeyEntry => insert_api_key_text(app, content),
        Mode::QueuedMessageEdit => crate::input::insert_queued_edit_text(app, content),
        _ => {}
    }
}
