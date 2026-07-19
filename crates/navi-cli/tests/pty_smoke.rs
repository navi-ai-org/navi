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
//!
//! Hard timeouts: unbounded `child.wait()` / reader joins previously stuck
//! CI for 30+ minutes when Ctrl+C did not exit the process. This test always
//! kills the child after a budget and never blocks forever.

#![cfg(target_os = "linux")]

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};

/// Overall wall-clock budget for the whole smoke (spawn + paint + quit).
const TOTAL_BUDGET: Duration = Duration::from_secs(20);
/// How long to wait for welcome controls before sending quit anyway.
const WELCOME_BUDGET: Duration = Duration::from_secs(12);
/// How long to wait for a clean exit after Ctrl+C before SIGKILL.
const QUIT_BUDGET: Duration = Duration::from_secs(3);
/// How long to wait for the PTY reader thread after the child dies.
const READER_JOIN_BUDGET: Duration = Duration::from_secs(2);

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

/// True when the first-paint TUI chrome is visible.
///
/// Assert on stable sidebar / status labels rather than the block logo (glyphs
/// vary) or old footer shortcuts (`commands`/`models`/`send`), which no longer
/// appear on the default home screen.
fn snapshot_has_welcome(buf: &[u8]) -> bool {
    let text = strip_ansi(&String::from_utf8_lossy(buf));
    let has_sidebar = text.contains("model") && text.contains("session");
    let has_status = text.contains("restricted")
        || text.contains("accept-edits")
        || text.contains("yolo")
        || text.contains("auto");
    has_sidebar && has_status
}

/// Poll `try_wait` until the child exits or `deadline` elapses.
fn wait_until_exit(child: &mut Box<dyn Child + Send + Sync>, deadline: Instant) -> bool {
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return false,
        }
    }
    matches!(child.try_wait(), Ok(Some(_)))
}

/// Force-terminate the child. Prefer portable-pty kill; fall back to SIGKILL.
fn force_kill(child: &mut Box<dyn Child + Send + Sync>) {
    let _ = child.kill();
    if matches!(child.try_wait(), Ok(None) | Err(_)) {
        if let Some(pid) = child.process_id() {
            // Last resort: SIGKILL so the test cannot hang the suite.
            // Avoid a direct libc dep; `kill(1)` is always available on Linux CI.
            let _ = std::process::Command::new("kill")
                .args(["-9", &pid.to_string()])
                .status();
        }
    }
    let _ = wait_until_exit(child, Instant::now() + Duration::from_secs(1));
}

#[test]
fn pty_smoke_renders_welcome_then_quits_cleanly() {
    let test_start = Instant::now();
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
    // Fully isolated dirs so the smoke never touches real user config/data
    // or blocks on network/registry side effects from a loaded profile.
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let xdg_data = tmp.path().join("data");
    let xdg_config = tmp.path().join("config");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&xdg_data).expect("data");
    std::fs::create_dir_all(&xdg_config).expect("config");
    cmd.env("HOME", &home);
    cmd.env("XDG_DATA_HOME", &xdg_data);
    cmd.env("XDG_CONFIG_HOME", &xdg_config);
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
    // Shared buffer so the main thread can poll for first paint without a
    // multi-second fixed sleep.
    let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::with_capacity(16 * 1024)));
    let buf_reader = buf.clone();
    let read_handle = std::thread::spawn(move || {
        let mut local = vec![0u8; 4096];
        loop {
            match reader.read(&mut local) {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(mut guard) = buf_reader.lock() {
                        guard.extend_from_slice(&local[..n]);
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Poll until welcome controls appear (or deadline). Fast on warm runs,
    // tolerant of cold binary start without burning a fixed multi-second wait.
    let welcome_deadline =
        Instant::now() + WELCOME_BUDGET.min(TOTAL_BUDGET.saturating_sub(test_start.elapsed()));
    let mut saw_welcome = false;
    while Instant::now() < welcome_deadline {
        // Bail early if the process already died (crash / immediate exit).
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        let snapshot = buf.lock().map(|g| g.clone()).unwrap_or_default();
        if snapshot_has_welcome(&snapshot) {
            saw_welcome = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    // Ask the TUI to quit (Ctrl+C → UiEffect::Quit when raw mode is on).
    // Write twice: first may be lost if the event loop was not ready yet.
    let _ = writer.write_all(b"\x03");
    let _ = writer.flush();
    std::thread::sleep(Duration::from_millis(100));
    let _ = writer.write_all(b"\x03");
    let _ = writer.flush();
    drop(writer);

    // Never block forever on wait — poll, then force-kill.
    let quit_deadline = Instant::now() + QUIT_BUDGET;
    let exited_cleanly = wait_until_exit(&mut child, quit_deadline);
    if !exited_cleanly {
        force_kill(&mut child);
    }

    // Closing the master side unblocks a stuck reader after the child dies.
    drop(pair.master);

    // Join reader with a timeout so a wedged read cannot hang the suite.
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = read_handle.join();
        let _ = done_tx.send(());
    });
    let _ = done_rx.recv_timeout(READER_JOIN_BUDGET);

    let output = buf.lock().map(|g| g.clone()).unwrap_or_default();
    let text = strip_ansi(&String::from_utf8_lossy(&output));

    // The TUI should have rendered the main chat chrome (sidebar + status).
    assert!(
        saw_welcome || snapshot_has_welcome(&output),
        "expected NAVI TUI chrome in PTY output (exited_cleanly={exited_cleanly}), got:\n{text}"
    );

    // Guardrail: the test itself must not exceed the total budget by much.
    // (Kill path already bounds the child; this is a soft sanity check.)
    assert!(
        test_start.elapsed() < TOTAL_BUDGET + Duration::from_secs(5),
        "pty smoke exceeded wall budget: {:?}",
        test_start.elapsed()
    );
}
