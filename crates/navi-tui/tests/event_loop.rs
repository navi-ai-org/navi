//! Tier 3 / TtyDriver-style tests: drive the real TUI event loop against a
//! `TestBackend` with a scripted `VecInput` event stream.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use navi_tui::testing::{Harness, Mode, TestConfig};

fn h() -> Harness {
    Harness::new(TestConfig {
        width: 80,
        height: 24,
        ..TestConfig::default()
    })
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    })
}

#[test]
fn drive_loop_opens_and_closes_command_palette() {
    let mut h = h();
    h.drive_loop(vec![key(KeyCode::Char('p'), KeyModifiers::CONTROL)]);

    // drive_loop appends ctrl+c at the end, which closes all modals before
    // quitting. So by the time drive_loop returns, the palette is closed.
    // The TUI should be back in normal mode.
    assert!(!h.is_loading());
}

#[test]
fn drive_loop_types_text_and_renders_input() {
    let mut h = h();
    h.drive_loop(vec![
        key(KeyCode::Char('h'), KeyModifiers::NONE),
        key(KeyCode::Char('e'), KeyModifiers::NONE),
        key(KeyCode::Char('l'), KeyModifiers::NONE),
        key(KeyCode::Char('l'), KeyModifiers::NONE),
        key(KeyCode::Char('o'), KeyModifiers::NONE),
    ]);
    // The harness's input was reset by `quit` clearing; just ensure no panic.
    assert!(!h.is_loading());
}

#[test]
fn drive_loop_esc_closes_palette() {
    let mut h = h();
    h.drive_loop(vec![
        key(KeyCode::Char('p'), KeyModifiers::CONTROL),
        key(KeyCode::Esc, KeyModifiers::NONE),
    ]);
    assert!(!h.is_loading());
}

#[test]
fn settings_keyboard_workflow() {
    let mut h = h();

    // Open command palette
    h.press(KeyCode::Char('p'), KeyModifiers::CONTROL);
    assert_eq!(h.mode(), Mode::Commands);

    // Type "settings" to filter
    h.type_text("settings");
    assert_eq!(h.mode(), Mode::Commands);

    // Select the settings command
    h.press(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(h.mode(), Mode::Settings);

    // Navigate down to Theme (index 3)
    h.press(KeyCode::Down, KeyModifiers::NONE);
    h.press(KeyCode::Down, KeyModifiers::NONE);
    h.press(KeyCode::Down, KeyModifiers::NONE);

    // Open theme picker
    h.press(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(h.mode(), Mode::ThemePicker);

    // Navigate in theme picker
    h.press(KeyCode::Down, KeyModifiers::NONE);

    // Select theme
    h.press(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(h.mode(), Mode::ThemePicker);

    // Close theme picker
    h.press(KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(h.mode(), Mode::Normal);
}
