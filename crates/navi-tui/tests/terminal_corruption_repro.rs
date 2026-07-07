//! Tests for terminal corruption bug fixes: when the user switches away from
//! the NAVI TUI window and returns, the input field would fill with garbage
//! escape-sequence characters and keyboard shortcuts would stop working.
//!
//! ## Evidence from user's terminal
//!
//! After returning to NAVI and exiting, the user's shell showed:
//!
//! ```text
//! ^[]11;rgb:1a1a/1b1b/2626^[\^[[19;3R^[[?62;52;c^[[99;133u^[[99;133uclear^M...
//! ```
//!
//! Decoded:
//! - `ESC ] 11 ; rgb:1a1a/1b1b/2626 ESC \` — OSC 11 background-color response
//! - `ESC [ 19;3 R` — CSI cursor-position report (DSR)
//! - `ESC [ ?62;52 c` — CSI device-attributes response (DA)
//! - `ESC [ 99;133 u` — Kitty keyboard protocol key event (key 'c', modifiers)
//!
//! ## Fixes applied
//!
//! 1. `LeakedTerminalSequenceFilter` now handles OSC (`\x1b]`), SS3 (`\x1bO`),
//!    and DCS (`\x1bP`) in addition to CSI (`\x1b[`).
//! 2. `insert_input_char` now rejects control characters (except `\n` and `\t`).
//! 3. `strip_terminal_control_sequences` now handles OSC and DCS sequences.
//! 4. Kitty keyboard protocol disable uses `\x1B[>0u` (set flags to 0) in
//!    addition to `\x1B[<u` (pop one level).
//! 5. `Event::Resize` now sets `needs_draw = true`.

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyModifiers};

use navi_tui::testing::{Harness, LeakedTerminalSequenceFilter, MockEngine, TestConfig};

// ─── Test 1: OSC sequences are now caught by the filter ───────────────────────

#[test]
fn osc_color_response_is_caught_by_filter() {
    let mut filter = LeakedTerminalSequenceFilter::default();

    // OSC 11 response: \x1b]11;rgb:1a1a/1b1b/2626\x1b\
    // (exactly what appeared in the user's terminal)
    let osc: Vec<char> = "\u{1b}]11;rgb:1a1a/1b1b/2626\u{1b}\\".chars().collect();
    let mut leaked = String::new();
    for ch in osc {
        let code = KeyCode::Char(ch);
        if !filter.should_drop(&code) {
            leaked.push(ch);
        }
    }

    assert!(
        leaked.is_empty(),
        "FIXED: OSC 11 color response is now fully filtered. \
         No characters should leak, but got: '{leaked}'"
    );
}

// ─── Test 2: OSC terminated with BEL is caught ────────────────────────────────

#[test]
fn osc_terminated_with_bel_is_caught() {
    let mut filter = LeakedTerminalSequenceFilter::default();

    // OSC with BEL terminator: \x1b]0;title\x07
    let osc: Vec<char> = "\u{1b}]0;title\u{07}".chars().collect();
    let mut leaked = String::new();
    for ch in osc {
        let code = KeyCode::Char(ch);
        if !filter.should_drop(&code) {
            leaked.push(ch);
        }
    }

    assert!(
        leaked.is_empty(),
        "OSC with BEL terminator should be fully filtered, got: '{leaked}'"
    );
}

// ─── Test 3: SS3 sequences are now caught by the filter ───────────────────────

#[test]
fn ss3_sequence_is_caught_by_filter() {
    let mut filter = LeakedTerminalSequenceFilter::default();

    // SS3: \x1bOA (Up arrow in some terminal modes)
    let ss3: Vec<char> = "\u{1b}OA".chars().collect();
    let mut leaked = String::new();
    for ch in ss3 {
        let code = KeyCode::Char(ch);
        if !filter.should_drop(&code) {
            leaked.push(ch);
        }
    }

    assert!(
        leaked.is_empty(),
        "FIXED: SS3 sequence is now fully filtered. \
         No characters should leak, but got: '{leaked}'"
    );
}

// ─── Test 4: CSI sequences still work (regression test) ───────────────────────

#[test]
fn csi_kitty_key_event_is_caught_by_filter() {
    let mut filter = LeakedTerminalSequenceFilter::default();

    // Kitty keyboard protocol: \x1b[99;133u (key 'c' with modifiers)
    let csi: Vec<char> = "\u{1b}[99;133u".chars().collect();
    let mut leaked = String::new();
    for ch in csi {
        let code = KeyCode::Char(ch);
        if !filter.should_drop(&code) {
            leaked.push(ch);
        }
    }

    assert!(
        leaked.is_empty(),
        "CSI sequences should still be fully filtered, got: '{leaked}'"
    );
}

// ─── Test 5: Normal characters still pass through (regression test) ──────────

#[test]
fn normal_characters_pass_through_filter() {
    let mut filter = LeakedTerminalSequenceFilter::default();

    let normal = "hello world";
    let mut leaked = String::new();
    for ch in normal.chars() {
        let code = KeyCode::Char(ch);
        if !filter.should_drop(&code) {
            leaked.push(ch);
        }
    }

    assert_eq!(
        leaked, normal,
        "Normal characters should pass through the filter unchanged"
    );
}

// ─── Test 6: insert_input_char rejects control characters ─────────────────────

#[test]
fn insert_input_char_rejects_control_chars() {
    let mut harness = Harness::with_engine(TestConfig::default(), Arc::new(MockEngine::new()));

    // Try to insert control characters that would come from leaked sequences
    let control_chars = ['\u{1b}', '\u{07}', '\u{00}', '\u{01}'];
    for ch in control_chars {
        harness.press(KeyCode::Char(ch), KeyModifiers::NONE);
    }

    assert!(
        harness.input().is_empty(),
        "FIXED: Control characters are now rejected by insert_input_char. \
         Input should be empty, but got: '{}'",
        harness.input()
    );
}

// ─── Test 7: Normal characters still insert correctly (regression) ───────────

#[test]
fn normal_characters_still_insert() {
    let mut harness = Harness::with_engine(TestConfig::default(), Arc::new(MockEngine::new()));

    harness.press(KeyCode::Char('h'), KeyModifiers::NONE);
    harness.press(KeyCode::Char('i'), KeyModifiers::NONE);

    assert_eq!(
        harness.input(),
        "hi",
        "Normal characters should still be inserted into the input field"
    );
}

// ─── Test 8: Paste of OSC sequence is sanitized ───────────────────────────────

#[test]
fn paste_of_osc_sequence_is_sanitized() {
    let mut harness = Harness::with_engine(TestConfig::default(), Arc::new(MockEngine::new()));

    // Simulate a paste containing an OSC sequence.
    harness.drive_loop(vec![Event::Paste(
        "\u{1b}]11;rgb:1a1a/1b1b/2626\u{1b}\\".to_string(),
    )]);

    let input = harness.input();
    assert!(
        input.is_empty(),
        "FIXED: Paste of OSC sequence is now fully sanitized. \
         Input should be empty, but got: '{input}'"
    );
}

// ─── Test 9: DCS sequences are caught by the filter ───────────────────────────

#[test]
fn dcs_sequence_is_caught_by_filter() {
    let mut filter = LeakedTerminalSequenceFilter::default();

    // DCS: \x1bP1$qr\x1b\
    let dcs: Vec<char> = "\u{1b}P1$qr\u{1b}\\".chars().collect();
    let mut leaked = String::new();
    for ch in dcs {
        let code = KeyCode::Char(ch);
        if !filter.should_drop(&code) {
            leaked.push(ch);
        }
    }

    assert!(
        leaked.is_empty(),
        "DCS sequences should be fully filtered, got: '{leaked}'"
    );
}

// ─── Test 10: Full scenario — leaked OSC does not corrupt input ───────────────

#[test]
fn full_scenario_osc_does_not_corrupt_input() {
    let mut harness = Harness::with_engine(TestConfig::default(), Arc::new(MockEngine::new()));

    // Simulate: terminal sends OSC 11 response that leaks as key chars.
    // We must use drive_loop (not press) so the LeakedTerminalSequenceFilter
    // runs inside run_loop.
    use crossterm::event::{KeyEvent, KeyEventKind};
    let osc_chars: Vec<char> = "\u{1b}]11;rgb:1a1a/1b1b/2626\u{1b}\\".chars().collect();
    let mut events: Vec<Event> = osc_chars
        .iter()
        .map(|&ch| {
            Event::Key(KeyEvent::new_with_kind(
                KeyCode::Char(ch),
                KeyModifiers::NONE,
                KeyEventKind::Press,
            ))
        })
        .collect();
    // Type "clear" after the leaked sequence
    for ch in "clear".chars() {
        events.push(Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char(ch),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )));
    }
    harness.drive_loop(events);

    assert_eq!(
        harness.input(),
        "clear",
        "FIXED: Full scenario — OSC response is filtered by \
         LeakedTerminalSequenceFilter and input is not corrupted. \
         User can type normally after leaked sequences."
    );
}
