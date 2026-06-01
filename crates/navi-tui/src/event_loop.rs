use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind, KeyboardEnhancementFlags};
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use ratatui::prelude::{CrosstermBackend, Terminal};

use crate::app::TuiApp;
use crate::chat::submit_message;
use crate::dispatch::handle_async_event;
use crate::keybindings::handle_key;
use crate::mouse::handle_mouse;
use crate::notifications::{expire_notification, visible_notification};
use crate::persistence::{save_current_session, save_preferences};
use crate::state::Mode;
use crate::view::render;

// ─── entry point (sync — no nested runtime) ────────────────────────────────────
// The caller (navi-cli `#[tokio::main]`) already owns a multi-thread tokio
// runtime, so `tokio::spawn` works from inside this synchronous event loop.
// We must NOT create a second runtime here.
pub fn run(app: TuiApp) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

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
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
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
            app.advance_tick();
            needs_draw = false;
        }

        if expire_notification(&mut app) {
            needs_draw = true;
        }

        // Check for async model stream events (non-blocking)
        while let Some(event) = app.try_recv_async_event() {
            needs_draw = true;
            handle_async_event(&mut app, event);
        }

        let timeout = if app.is_loading {
            Duration::from_millis(16)
        } else if app.messages.is_empty() || visible_notification(&app).is_some() {
            Duration::from_millis(80)
        } else {
            Duration::from_millis(250)
        };

        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key)
                    if key.kind == KeyEventKind::Press => {
                        needs_draw = true;
                        if handle_key(&mut app, key.code, key.modifiers) {
                            break;
                        }
                    }
                Event::Mouse(mouse_event) => {
                    needs_draw = true;
                    handle_mouse(&mut app, mouse_event);
                }
                _ => {}
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
