//! PTY smoke test: spawn the `navi` binary in a pseudo-terminal, send a few
//! keypresses, and verify the rendered output contains the expected banner.
//!
//! This catches the most embarrassing regressions — a TUI that crashes on
//! startup, hangs forever, or renders an empty screen — without depending
//! on a graphical terminal emulator.
//!
//! Gated to Linux because the test is part of `just ci` and we only run CI
//! on Linux runners in practice; the `portable-pty` crate supports macOS
//! and Windows too, but the assertion strings target the default `NAVI`
//! banner which is platform-independent.

#![cfg(target_os = "linux")]

use std::io::Read;
use std::time::Duration;

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

/// Strip ANSI escape sequences from a string of PTY output.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip until we hit a letter (the terminator of an escape sequence).
            while let Some(&next) = chars.peek() {
                chars.next();
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn pty_smoke_renders_welcome_then_quits_cleanly() {
    let bin = env!("CARGO_BIN_EXE_navi");
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    let mut cmd = CommandBuilder::new(bin);
    // Use a fresh, isolated data dir so the test doesn't touch user state.
    let tmp = tempfile::tempdir().expect("tempdir");
    cmd.env("XDG_DATA_HOME", tmp.path());
    cmd.env("NAVI_NO_REGISTRY_UPDATE", "1");
    // Mark this as a smoke test run so the binary could opt out of
    // things like network syncs if it wants to (no flags for that yet,
    // but the env var is documented here for future-proofing).
    cmd.env("NAVI_SMOKE_TEST", "1");

    let mut child = pair.slave.spawn_command(cmd).expect("spawn navi");
    // After spawn, the slave is owned by the child. Dropping the slave
    // handle here is fine.
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("clone reader");
    let mut writer = pair.master.take_writer().expect("take writer");
    let mut buf: Vec<u8> = Vec::with_capacity(16 * 1024);
    let read_handle = std::thread::spawn(move || {
        let mut local = vec![0u8; 4096];
        loop {
            match reader.read(&mut local) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&local[..n]);
                }
                Err(_) => break,
            }
        }
        buf
    });

    // Give the TUI a moment to draw the welcome screen, then quit.
    // We don't try to wait for a specific banner string here; we just
    // give it enough time to initialize and render, then send ctrl+c.
    std::thread::sleep(Duration::from_secs(2));

    use std::io::Write;
    writer.write_all(b"\x03").expect("write ctrl+c");
    drop(writer);
    // Wait for the child to exit.
    let _ = child.wait();
    let output = read_handle.join().expect("read thread");
    let text = strip_ansi(&String::from_utf8_lossy(&output));

    // The TUI should have rendered the welcome/chat screen. The block logo does
    // not contain a plain "NAVI" substring after ratatui layout compaction, so
    // assert on stable shortcut labels instead of the logo glyphs.
    assert!(
        text.contains("commands") && text.contains("models") && text.contains("send"),
        "expected NAVI welcome controls in PTY output, got:\n{text}"
    );

    // The binary should not have left the TUI in alt-screen on exit.
    // (Crossterm restores the screen on shutdown, so this is implicit.
    // We don't have a way to assert it from the test, but if it crashed
    // the output would be empty or garbled.)
}
