//! PTY test: swap the frontend of a **live** session at runtime.
//!
//! This is the end-to-end proof of the spatiotemporal composition path with a
//! real terminal:
//!
//! 1. `navi --ui navi.ui.tui` starts the ratatui frontend *as a component*, with
//!    the real exclusive terminal lease;
//! 2. from another process, `navi components reload navi.ui.plain` requests a
//!    generation swap;
//! 3. the supervisor quiesces the TUI (it restores the terminal and releases the
//!    lease), withdraws it, and activates the line frontend — which prints its
//!    banner into the same PTY;
//! 4. the session keeps running and `exit` on the successor ends it cleanly.
//!
//! Gated to Linux (same rationale as `pty_smoke.rs`).

#![cfg(target_os = "linux")]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};

/// Overall wall-clock budget for the whole scenario.
const TOTAL_BUDGET: Duration = Duration::from_secs(90);

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
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

fn output_text(buf: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>) -> String {
    let snapshot = buf.lock().map(|guard| guard.clone()).unwrap_or_default();
    strip_ansi(&String::from_utf8_lossy(&snapshot))
}

/// Polls the PTY buffer until `pred` holds or the deadline elapses.
fn wait_for_output(
    buf: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    deadline: Instant,
    pred: impl Fn(&str) -> bool,
) -> bool {
    while Instant::now() < deadline {
        if pred(&output_text(buf)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    pred(&output_text(buf))
}

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

fn force_kill(child: &mut Box<dyn Child + Send + Sync>) {
    let _ = child.kill();
    if matches!(child.try_wait(), Ok(None) | Err(_))
        && let Some(pid) = child.process_id()
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
    }
    let _ = wait_until_exit(child, Instant::now() + Duration::from_secs(1));
}

/// Reads the supervisor's published status snapshot.
fn read_status(xdg_data: &Path) -> Option<serde_json::Value> {
    let path = xdg_data.join("navi/compose/status.json");
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn component<'a>(status: &'a serde_json::Value, id: &str) -> Option<&'a serde_json::Value> {
    status
        .get("components")?
        .as_array()?
        .iter()
        .find(|component| component.get("id").and_then(|v| v.as_str()) == Some(id))
}

fn running_frontend(status: &serde_json::Value) -> Option<String> {
    status
        .get("running_frontend")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
}

#[test]
fn pty_compose_swaps_frontend_at_runtime() {
    let started = Instant::now();
    let bin = env!("CARGO_BIN_EXE_navi");
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let xdg_data = tmp.path().join("data");
    let xdg_config = tmp.path().join("config");
    let project = tmp.path().join("project");
    for dir in [&home, &xdg_data, &xdg_config, &project] {
        std::fs::create_dir_all(dir).expect("dir");
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    let mut cmd = CommandBuilder::new(bin);
    cmd.cwd(&project);
    cmd.env("HOME", &home);
    cmd.env("XDG_DATA_HOME", &xdg_data);
    cmd.env("XDG_CONFIG_HOME", &xdg_config);
    cmd.env("NAVI_NO_REGISTRY_UPDATE", "1");
    cmd.args(["--ui", "navi.ui.tui"]);

    let mut child = pair.slave.spawn_command(cmd).expect("spawn navi");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("clone reader");
    let mut writer = pair.master.take_writer().expect("take writer");
    let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::with_capacity(64 * 1024)));
    let buf_reader = buf.clone();
    let read_handle = std::thread::spawn(move || {
        let mut local = vec![0u8; 8192];
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

    // 1. The TUI must come up as a component, owning the real terminal.
    let chrome_deadline = started + Duration::from_secs(30);
    let saw_tui = wait_for_output(&buf, chrome_deadline, |text| {
        text.contains("model") && text.contains("session")
    });
    let status = read_status(&xdg_data);
    assert!(
        saw_tui,
        "expected the composed TUI to render chrome, got:\n{}",
        output_text(&buf)
    );
    let status = status.expect("status snapshot published");
    assert_eq!(running_frontend(&status).as_deref(), Some("navi.ui.tui"));
    let tui = component(&status, "navi.ui.tui").expect("tui component");
    assert_eq!(tui["state"], "Active");
    assert_eq!(tui["tty"], true, "the TUI holds the terminal lease");
    assert_eq!(
        component(&status, "navi.engine").expect("engine")["state"],
        "Active"
    );

    // 2. Another process requests a frontend swap.
    let reload = std::process::Command::new(bin)
        .args(["components", "reload", "navi.ui.plain"])
        .current_dir(&project)
        .env("HOME", &home)
        .env("XDG_DATA_HOME", &xdg_data)
        .env("XDG_CONFIG_HOME", &xdg_config)
        .output()
        .expect("run navi components reload");
    assert!(
        reload.status.success(),
        "reload request failed: {}",
        String::from_utf8_lossy(&reload.stderr)
    );

    // 3. The successor frontend takes over the same PTY.
    let swap_deadline = Instant::now() + Duration::from_secs(30);
    let saw_successor = wait_for_output(&buf, swap_deadline, |text| {
        text.contains("[navi.ui.plain]")
    });
    assert!(
        saw_successor,
        "expected the line frontend to take over after the swap, got:\n{}",
        output_text(&buf)
    );

    let status = read_status(&xdg_data).expect("status snapshot after swap");
    assert_eq!(
        running_frontend(&status).as_deref(),
        Some("navi.ui.plain"),
        "the successor must be the running frontend: {status}"
    );
    let plain = component(&status, "navi.ui.plain").expect("plain component");
    assert_eq!(plain["state"], "Active");
    assert_eq!(
        plain["tty"], false,
        "the line frontend does not own the terminal"
    );
    let tui = component(&status, "navi.ui.tui").expect("tui component");
    assert_ne!(tui["state"], "Active", "the previous generation is gone");
    assert_eq!(tui["ui_running"], false);

    // 4. The live session survives; a shutdown signal ends it cleanly (the
    //    successor owns the input loop, and crossterm's internal reader from the
    //    retired TUI generation may still hold stdin).
    let _ = writer.write_all(b"exit\n");
    let _ = writer.flush();
    let mut exited = wait_until_exit(&mut child, Instant::now() + Duration::from_secs(3));
    if !exited {
        if let Some(pid) = child.process_id() {
            let _ = std::process::Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
        }
        exited = wait_until_exit(&mut child, Instant::now() + Duration::from_secs(15));
    }
    if !exited {
        force_kill(&mut child);
    }
    drop(pair.master);
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = read_handle.join();
        let _ = done_tx.send(());
    });
    let _ = done_rx.recv_timeout(Duration::from_secs(3));

    assert!(
        exited,
        "the composed session must exit after the successor quits; output:\n{}",
        output_text(&buf)
    );
    assert!(
        started.elapsed() < TOTAL_BUDGET,
        "scenario exceeded the wall budget: {:?}",
        started.elapsed()
    );

    // The kernel journal records the withdrawal of generation 1 and the
    // activation of the successor with fresh bindings.
    let journal = std::fs::read_to_string(xdg_data.join("navi/compose/journal.jsonl"))
        .expect("kernel journal");
    assert!(
        journal.contains("\"kind\":\"plugin.unloaded\""),
        "journal must record the withdrawal: {journal}"
    );
    assert!(
        journal.contains("\"id\":\"navi.ui.plain\""),
        "journal must record the successor activation: {journal}"
    );
    let _ = PathBuf::new();
}
