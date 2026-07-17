use std::io;
use std::panic;
use std::sync::Once;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::Backend;
use ratatui::prelude::{CrosstermBackend, Terminal};

/// Kitty progressive-enhancement flags NAVI owns for this session.
///
/// - `DISAMBIGUATE_ESCAPE_CODES` — Ctrl/Shift/Alt chords become unambiguous CSI-u
/// - `REPORT_EVENT_TYPES` — Press/Repeat/Release (we only handle Press|Repeat)
///
/// Both flags are needed: without `REPORT_EVENT_TYPES`, some terminals stop
/// sending mouse wheel events as `Event::Mouse` and instead emit them as
/// arrow-key sequences, which makes the scroll handler select chat blocks
/// instead of scrolling the viewport.
const NAVI_KEYBOARD_FLAGS: KeyboardEnhancementFlags =
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        .union(KeyboardEnhancementFlags::REPORT_EVENT_TYPES);

/// True when running as an embedded multi-agent tile in NAVI Desktop.
///
/// Desktop owns selection via xterm.js and strips mouse/focus DEC modes on the
/// host path. Enabling crossterm mouse capture (`?1003h`) or focus tracking
/// (`?1004h`) here still causes hover redraw thrash and FocusGained `clear()`
/// flashes if anything leaks — so we skip those modes entirely when embedded.
fn is_desktop_tile() -> bool {
    matches!(
        std::env::var("NAVI_DESKTOP_TILE").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

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
    clear_leftover_terminal_modes(w)?;
    execute!(w, EnterAlternateScreen, EnableBracketedPaste)?;
    // Re-assert after alternate-screen entry — some terminals re-apply parent
    // Kitty flags when switching screens.
    enable_navi_keyboard_protocol(w)?;
    if is_desktop_tile() {
        // Embedded xterm tiles: no mouse capture / focus reports (desktop
        // selection + anti-flicker). Keyboard + paste still enabled above.
        return Ok(());
    }
    enable_focus_tracking(w)?;
    // Base mouse only (no free-motion). Free-motion is synced later when images
    // need hover — keeps multi-window sessions from leaking motion CSI.
    enable_mouse_capture(w, false)
}

/// Clear parent Kitty stack without the historical no-op (`>0u` then immediate pop).
///
/// Kitty progressive enhancement is a stack:
/// - `CSI > flags u` — push
/// - `CSI < u` / `<1u` — pop
/// - `CSI = flags u` — set in place
fn clear_parent_keyboard_stack(w: &mut impl io::Write) -> io::Result<()> {
    write!(
        w,
        concat!(
            "\x1B[>4;0m",            // xterm modifyOtherKeys off
            "\x1B[<u\x1B[<u\x1B[<u", // pop parent stack levels
            "\x1B[=0u",              // set flags to 0 in place (sticks)
        )
    )?;
    w.flush()
}

/// Own the Kitty keyboard protocol for this session (Grok-style negotiate).
fn enable_navi_keyboard_protocol(w: &mut impl io::Write) -> io::Result<()> {
    clear_parent_keyboard_stack(w)?;
    // Emits `CSI > 3 u` for DISAMBIGUATE | REPORT_EVENT_TYPES.
    execute!(w, PushKeyboardEnhancementFlags(NAVI_KEYBOARD_FLAGS))
}

/// Lightweight reassert on FocusGained: pop our previous push, re-push, restore
/// paste/focus/mouse. Avoids thrashing the parent stack with a full clear every
/// focus cycle (multi-window long sessions).
fn reassert_terminal_input_modes(w: &mut impl io::Write, free_motion: bool) -> io::Result<()> {
    let _ = execute!(w, PopKeyboardEnhancementFlags);
    execute!(w, PushKeyboardEnhancementFlags(NAVI_KEYBOARD_FLAGS))?;
    execute!(w, EnableBracketedPaste)?;
    if is_desktop_tile() {
        return Ok(());
    }
    enable_focus_tracking(w)?;
    enable_mouse_capture(w, free_motion)
}

fn enable_focus_tracking(w: &mut impl io::Write) -> io::Result<()> {
    write!(w, "\x1B[?1004h")?;
    w.flush()
}

fn clear_leftover_terminal_modes(w: &mut impl io::Write) -> io::Result<()> {
    disable_mouse_capture(w)?;
    let _ = execute!(w, DisableBracketedPaste, PopKeyboardEnhancementFlags);
    clear_parent_keyboard_stack(w)
}

fn reset_terminal_input_modes(w: &mut impl io::Write) -> io::Result<()> {
    disable_mouse_capture(w)?;
    execute!(w, DisableBracketedPaste, PopKeyboardEnhancementFlags)?;
    // Leave the shell in classic keyboard mode.
    clear_parent_keyboard_stack(w)
}

/// Whether free mouse motion (?1003) is useful right now.
///
/// Free-motion is the main multi-window leak source. Only enable it when image
/// hover can actually fire (pending chips, chat images, or an open lightbox).
pub(crate) fn wants_mouse_free_motion(app: &TuiApp) -> bool {
    app.image_hover.is_some()
        || !app.pending_images.is_empty()
        || app.messages.iter().any(|m| !m.images.is_empty())
}

/// Enable mouse capture.
///
/// Match crossterm's [`EnableMouseCapture`] set: press/release (`1000`),
/// button-drag (`1002`), any-motion (`1003`), RXVT coords (`1015`), SGR
/// (`1006`). Previously we disabled free-motion (`1003`) except for image
/// hover — that broke text drag-select on terminals that only report motion
/// under `1003`, so selection never extended past the click cell.
///
/// `free_motion` is kept for API compatibility with callers that still track
/// image-hover intent; the wire modes are always the full capture set.
fn enable_mouse_capture(w: &mut impl io::Write, _free_motion: bool) -> io::Result<()> {
    // Prefer the crossterm command so we stay aligned with its parser
    // expectations (includes 1015 + 1003 which our hand-rolled CSI omitted).
    execute!(w, EnableMouseCapture)
}

/// Sync free-motion on/off when image state changes.
///
/// With full mouse capture always enabled, this only tracks the flag for
/// diagnostics / future selective mode — reasserting capture is still cheap
/// and keeps multi-window focus recovery healthy.
pub(crate) fn sync_mouse_free_motion(app: &mut TuiApp) -> io::Result<bool> {
    if is_desktop_tile() {
        // Never enable mouse capture in embedded desktop tiles.
        app.mouse_free_motion = false;
        return Ok(false);
    }
    let want = wants_mouse_free_motion(app);
    if app.mouse_free_motion == want {
        return Ok(false);
    }
    let mut stdout = io::stdout();
    enable_mouse_capture(&mut stdout, want)?;
    app.mouse_free_motion = want;
    Ok(true)
}

/// Disable mouse modes defensively.
fn disable_mouse_capture(w: &mut impl io::Write) -> io::Result<()> {
    // Crossterm's disable, then a hard reset for any extra modes we may have
    // inherited from a parent session.
    let _ = execute!(w, DisableMouseCapture);
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
    use std::sync::Mutex;

    /// Serialize tests that mutate `NAVI_DESKTOP_TILE` (process-global env).
    static DESKTOP_TILE_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_desktop_tile_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _guard = DESKTOP_TILE_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // SAFETY: held under DESKTOP_TILE_ENV_LOCK; restored before unlock.
        unsafe {
            match value {
                Some(v) => std::env::set_var("NAVI_DESKTOP_TILE", v),
                None => std::env::remove_var("NAVI_DESKTOP_TILE"),
            }
        }
        let out = f();
        unsafe {
            std::env::remove_var("NAVI_DESKTOP_TILE");
        }
        out
    }

    #[test]
    fn mouse_capture_enables_full_crossterm_set() {
        let mut out = Vec::new();
        enable_mouse_capture(&mut out, false).expect("enable mouse capture");
        let text = String::from_utf8(out).expect("utf8 escape sequences");

        // Matches crossterm EnableMouseCapture (needed for drag-select).
        assert!(text.contains("\x1B[?1000h"));
        assert!(text.contains("\x1B[?1002h"));
        assert!(text.contains("\x1B[?1003h")); // any-motion — required for reliable drag
        assert!(text.contains("\x1B[?1006h"));
        assert!(text.contains("\x1B[?1015h"));
    }

    #[test]
    fn mouse_capture_free_motion_flag_still_enables_capture() {
        let mut out = Vec::new();
        enable_mouse_capture(&mut out, true).expect("enable free motion");
        let text = String::from_utf8(out).expect("utf8");
        assert!(text.contains("\x1B[?1003h"));
        assert!(text.contains("\x1B[?1002h"));
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
        assert!(text.contains("\x1B[<1u") || text.contains("\x1B[<u"));
        assert!(text.contains("\x1B[=0u"));
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
    fn terminal_input_reset_leaves_classic_keyboard_for_shell() {
        let mut out = Vec::new();
        reset_terminal_input_modes(&mut out).expect("reset");
        let text = String::from_utf8(out).expect("utf8");
        assert!(text.contains("\x1B[=0u"));
        assert!(
            !text.contains("\x1B[>0u\x1B[<u") && !text.contains("\x1B[>0u\x1B[<1u"),
            "must not push-0 then immediate pop: {text:?}"
        );
    }

    #[test]
    fn enter_terminal_modes_pushes_kitty_and_enables_full_mouse_capture() {
        with_desktop_tile_env(None, || {
            let mut out = Vec::new();
            enter_terminal_modes(&mut out).expect("enter");
            let text = String::from_utf8(out).expect("utf8");

            // DISAMBIGUATE|REPORT_EVENT_TYPES → >3u
            assert!(
                text.contains("\x1B[>3u"),
                "must PushKeyboardEnhancementFlags: {text:?}"
            );
            assert!(text.contains("\x1B[=0u"));
            assert!(text.contains("\x1B[>4;0m"));
            assert!(text.contains("\x1B[?1004h"));
            assert!(text.contains("\x1B[?1000h"));
            // Full crossterm mouse set (drag-select needs 1002 + 1003).
            assert!(text.contains("\x1B[?1002h"));
            assert!(text.contains("\x1B[?1003h"));
            assert!(text.contains("\x1B[?1006h"));
        });
    }

    #[test]
    fn reassert_pops_then_pushes_keyboard_enhancement() {
        with_desktop_tile_env(None, || {
            let mut out = Vec::new();
            reassert_terminal_input_modes(&mut out, false).expect("reassert");
            let text = String::from_utf8(out).expect("utf8");
            assert!(text.contains("\x1B[<1u") || text.contains("\x1B[<u"));
            assert!(text.contains("\x1B[>3u"));
            assert!(text.contains("\x1B[?1004h"));
        });
    }

    #[test]
    fn desktop_tile_skips_mouse_and_focus_modes() {
        with_desktop_tile_env(Some("1"), || {
            let mut out = Vec::new();
            enter_terminal_modes(&mut out).expect("enter");
            let text = String::from_utf8(out).expect("utf8");

            // Keyboard + paste still negotiated.
            assert!(
                text.contains("\x1B[>3u"),
                "must still push Kitty keyboard: {text:?}"
            );
            // No mouse capture / focus tracking — desktop xterm owns those paths.
            assert!(
                !text.contains("\x1B[?1003h"),
                "must not enable any-motion mouse: {text:?}"
            );
            assert!(
                !text.contains("\x1B[?1000h"),
                "must not enable mouse press tracking: {text:?}"
            );
            assert!(
                !text.contains("\x1B[?1004h"),
                "must not enable focus tracking: {text:?}"
            );
        });
    }

    #[test]
    fn desktop_tile_reassert_skips_mouse_and_focus() {
        with_desktop_tile_env(Some("1"), || {
            let mut out = Vec::new();
            reassert_terminal_input_modes(&mut out, true).expect("reassert");
            let text = String::from_utf8(out).expect("utf8");

            assert!(text.contains("\x1B[>3u"));
            assert!(!text.contains("\x1B[?1003h"));
            assert!(!text.contains("\x1B[?1004h"));
        });
    }

    #[test]
    fn parent_keyboard_clear_does_not_undo_itself() {
        let mut out = Vec::new();
        clear_parent_keyboard_stack(&mut out).expect("clear");
        let text = String::from_utf8(out).expect("utf8");
        assert!(text.contains("\x1B[=0u"));
        assert!(
            !text.contains("\x1B[>0u\x1B[<u") && !text.contains("\x1B[>0u\x1B[<1u"),
            "clear must not push 0 and immediately pop: {text:?}"
        );
    }

    #[test]
    fn wants_mouse_free_motion_only_with_images() {
        let app = crate::tests::test_app("");
        assert!(!wants_mouse_free_motion(&app));
    }

    #[test]
    fn leaked_filter_does_not_swallow_bare_escape_key() {
        let mut filter = LeakedTerminalSequenceFilter::default();
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
        let activity_animating = app.is_loading || !app.running_tools.is_empty() || bg_running;

        if needs_draw || composer_animating || activity_animating {
            terminal.draw(|frame| render(frame, app))?;
            app.advance_tick();
            needs_draw = false;
        }

        if expire_notification(app) {
            needs_draw = true;
        }

        // Finished plans leave the topbar after 1 minute (or when replaced by create).
        if crate::plan_progress::maybe_dismiss_completed_plan(app) {
            needs_draw = true;
        }

        if crate::view::image_preview::poll_image_hover_close(app) {
            needs_draw = true;
        }

        // Toggle ?1003 only while image hover can fire (pending/chat images or
        // open lightbox). Avoids free-motion CSI leaks across multi-window use.
        let _ = sync_mouse_free_motion(app);

        // Long-running streams do not always include a live Usage chunk. Keep
        // account-backed providers fresh while work is active (and while the
        // Usage modal is visible) without polling on every frame.
        if (app.is_loading || app.mode == Mode::Usage)
            && crate::usage::refresh_account_usage_if_due(app)
        {
            needs_draw = true;
        }

        // Check for async model stream events (non-blocking)
        while let Some(event) = app.try_recv_async_event() {
            needs_draw = true;
            handle_async_event(app, event);
        }

        let done_plan_pending = app
            .active_plan
            .as_ref()
            .is_some_and(|p| p.is_done() && p.completed_at.is_some());
        let mut timeout = if activity_animating || composer_animating {
            // ~30fps is enough for a 320ms pulse frame and keeps CPU low.
            Duration::from_millis(33)
        } else if app.messages.is_empty() || visible_notification(app).is_some() {
            Duration::from_millis(80)
        } else if done_plan_pending {
            // Wake often enough to dismiss a finished plan near the 60s mark.
            Duration::from_millis(500)
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
                        && app
                            .last_mouse_event
                            .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(150))
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
                    // Multi-window solid path:
                    // 1. Re-negotiate keyboard/paste/focus/mouse (parent may
                    //    have restored Kitty flags while unfocused).
                    // 2. Reset leak-filter mid-sequence state.
                    // 3. Full redraw for alternate-screen recovery.
                    app.terminal_focused = true;
                    leaked_terminal_sequence_filter.reset();
                    let free_motion = wants_mouse_free_motion(app);
                    app.mouse_free_motion = free_motion;
                    let mut stdout = io::stdout();
                    let _ = reassert_terminal_input_modes(&mut stdout, free_motion);
                    terminal.clear()?;
                    needs_draw = true;
                }
                Event::FocusLost => {
                    app.terminal_focused = false;
                    // Drop in-flight leaked CSI so a partial mouse sequence
                    // outside the window does not swallow the next real key.
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

/// Handle a bracketed paste event.
///
/// Normal-mode rules (also apply while the model is streaming — drafts queue
/// on submit behind the active turn):
/// 1. If `content` is a filesystem path to an image → attach that file.
/// 2. If `content` is empty/whitespace → try system clipboard image (some
///    terminals deliver image paste as an empty bracketed paste).
/// 3. Otherwise insert `content` as text. Never steal a text paste just because
///    the clipboard also holds an image.
pub(crate) fn handle_paste(app: &mut TuiApp, content: &str) {
    use crate::clipboard::try_read_clipboard_image;
    use crate::notifications::show_notification;

    match app.mode {
        Mode::Normal => {
            if let Some(image) = crate::clipboard::try_read_image_from_path(content) {
                app.pending_images.push(image);
                let tag = format!("[Image {}]", app.pending_images.len());
                insert_input_text(app, &tag);
                show_notification(app, "Image", format!("Attached as {}", tag));
                return;
            }

            if content.trim().is_empty() {
                if let Some(image) = try_read_clipboard_image() {
                    app.pending_images.push(image);
                    let tag = format!("[Image {}]", app.pending_images.len());
                    insert_input_text(app, &tag);
                    show_notification(app, "Image", format!("Attached as {}", tag));
                }
                return;
            }

            insert_input_text(app, content);
        }
        Mode::ApiKeyEntry => insert_api_key_text(app, content),
        Mode::QueuedMessageEdit => crate::input::insert_queued_edit_text(app, content),
        _ => {}
    }
}
