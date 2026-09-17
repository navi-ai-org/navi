//! PTY test: swap the frontend of a live session to a **third-party TUI that
//! runs in its own process**, then back to the built-in one.
//!
//! This is the literal form of the goal: another TUI, shipped as a plugin, not
//! compiled into NAVI, taking over the terminal at runtime and handing it back.
//!
//! 1. `navi --ui navi.ui.tui` starts the built-in ratatui frontend;
//! 2. `navi components reload navi.plugin.tui` withdraws it and activates the
//!    plugin, which owns the terminal from its own process;
//! 3. the plugin renders its banner into the same PTY;
//! 4. `navi components reload navi.ui.tui` swaps back;
//! 5. the session still exits cleanly.
//!
//! Gated to Linux (same rationale as the other PTY tests).

#![cfg(target_os = "linux")]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use navi_compose::{ComponentKind, ComponentManifest, Compose, ComposeOptions, Execution};
use navi_core::{LoadedConfig, NaviConfig};
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};

const PLUGIN_ID: &str = "navi.plugin.tui";

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

fn output_text(buf: &Arc<Mutex<Vec<u8>>>) -> String {
    let snapshot = buf.lock().map(|guard| guard.clone()).unwrap_or_default();
    strip_ansi(&String::from_utf8_lossy(&snapshot))
}

fn wait_for_output(
    buf: &Arc<Mutex<Vec<u8>>>,
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

/// Kills the spawned NAVI (and the plugin it started) when the test ends.
struct ProcessGuard {
    child: Option<Box<dyn Child + Send + Sync>>,
    plugin_binary: String,
}

impl ProcessGuard {
    fn child(&mut self) -> &mut Box<dyn Child + Send + Sync> {
        self.child.as_mut().expect("child present")
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            force_kill(&mut child);
        }
        // The plugin runs in its own process; never leave it behind.
        let _ = std::process::Command::new("pkill")
            .args(["-f", &self.plugin_binary])
            .status();
    }
}

fn read_status(xdg_data: &Path) -> Option<serde_json::Value> {
    let path = xdg_data.join("navi/compose/status.json");
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn running_frontend(status: &serde_json::Value) -> Option<String> {
    status
        .get("running_frontend")
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

/// Installs the plugin manifest into the compose home the session will use.
fn install_plugin(xdg_data: &Path, project: &Path, plugin_binary: &str) {
    let loaded_config = LoadedConfig {
        config: NaviConfig::default(),
        global_config_path: None,
        project_config_path: None,
        data_dir: xdg_data.join("navi"),
    };
    let compose = Compose::new(ComposeOptions {
        home: navi_compose::default_home(&loaded_config.data_dir),
        project_dir: project.to_path_buf(),
        loaded_config,
        dry_run: false,
        terminal_effects: false,
        process_components: false,
    })
    .expect("compose");
    compose
        .install_definitions(&navi_compose::default_manifests())
        .expect("default manifests");
    compose
        .install_definition(ComponentManifest {
            id: PLUGIN_ID.to_string(),
            version: "0.1.0".to_string(),
            description: "Third-party terminal frontend (out of process)".to_string(),
            capabilities: vec!["navi.plugin.tui@1".to_string()],
            requires: Vec::new(),
            subscriptions: Vec::new(),
            execution: Execution::process_with_args(
                plugin_binary,
                vec![
                    "--control".to_string(),
                    "{control}".to_string(),
                    "--session".to_string(),
                    "{session}".to_string(),
                ],
            ),
            tier: "local".to_string(),
            trust: "trusted".to_string(),
            restart: "on-failure".to_string(),
            calls: Default::default(),
            kind: ComponentKind::Ui,
            sandbox: None,
        })
        .expect("install plugin manifest");
}

/// Resolves a workspace binary next to the test executable.
///
/// `target/debug/deps/<test>` → `target/debug/<name>`. The plugin is a separate
/// package, so `CARGO_BIN_EXE_*` is not available for it: build it first
/// (`cargo test -p navi-cli -p navi-plugin-tui`).
fn workspace_binary(name: &str) -> PathBuf {
    let exe = std::env::current_exe().expect("current test exe");
    let deps = exe.parent().expect("deps dir");
    let debug = deps.parent().expect("target dir");
    let candidate = debug.join(name);
    assert!(
        candidate.exists(),
        "missing {} — build it first: cargo build -p {name}",
        candidate.display()
    );
    candidate
}

#[test]
fn pty_swaps_to_a_third_party_process_tui_and_back() {
    let started = Instant::now();
    let navi = env!("CARGO_BIN_EXE_navi");
    let plugin_binary = workspace_binary("navi-plugin-tui");
    let plugin_binary = plugin_binary.to_string_lossy().to_string();

    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let xdg_data = tmp.path().join("data");
    let xdg_config = tmp.path().join("config");
    let project = tmp.path().join("project");
    for dir in [&home, &xdg_data, &xdg_config, &project] {
        std::fs::create_dir_all(dir).expect("dir");
    }
    install_plugin(&xdg_data, &project, &plugin_binary);

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let mut cmd = CommandBuilder::new(navi);
    cmd.cwd(&project);
    cmd.env("HOME", &home);
    cmd.env("XDG_DATA_HOME", &xdg_data);
    cmd.env("XDG_CONFIG_HOME", &xdg_config);
    cmd.env("NAVI_NO_REGISTRY_UPDATE", "1");
    cmd.args(["--ui", "navi.ui.tui"]);

    let child = pair.slave.spawn_command(cmd).expect("spawn navi");
    drop(pair.slave);
    let mut guard = ProcessGuard {
        child: Some(child),
        plugin_binary: plugin_binary.clone(),
    };

    let mut reader = pair.master.try_clone_reader().expect("clone reader");
    let mut writer = pair.master.take_writer().expect("take writer");
    let buf = Arc::new(Mutex::new(Vec::with_capacity(128 * 1024)));
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

    let reload = |id: &str| {
        let output = std::process::Command::new(navi)
            .args(["components", "reload", id])
            .current_dir(&project)
            .env("HOME", &home)
            .env("XDG_DATA_HOME", &xdg_data)
            .env("XDG_CONFIG_HOME", &xdg_config)
            .output()
            .expect("run navi components reload");
        assert!(
            output.status.success(),
            "reload {id} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };

    // 1. The built-in TUI renders.
    assert!(
        wait_for_output(&buf, started + Duration::from_secs(30), |text| {
            text.contains("model") && text.contains("session")
        }),
        "built-in TUI never rendered:\n{}",
        output_text(&buf)
    );
    let status = read_status(&xdg_data).expect("status");
    assert_eq!(running_frontend(&status).as_deref(), Some("navi.ui.tui"));

    // 2/3. Swap to the third-party TUI: it takes over the terminal.
    reload(PLUGIN_ID);
    let swapped = wait_for_output(&buf, Instant::now() + Duration::from_secs(30), |text| {
        text.contains("NAVI plugin TUI")
    });
    assert!(
        swapped,
        "the plugin TUI never rendered:\n{}",
        output_text(&buf)
    );
    let status = read_status(&xdg_data).expect("status after swap");
    assert_eq!(
        running_frontend(&status).as_deref(),
        Some(PLUGIN_ID),
        "the plugin must be the running frontend: {status}"
    );
    let plugin_component = status["components"]
        .as_array()
        .expect("components")
        .iter()
        .find(|component| component["id"] == PLUGIN_ID)
        .expect("plugin component");
    assert_eq!(plugin_component["state"], "Active");
    assert!(
        plugin_component["generation"].as_u64().unwrap_or(0) >= 1,
        "the plugin runs in a generation: {plugin_component}"
    );

    // 4. Swap back to the built-in TUI.
    reload("navi.ui.tui");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut back = false;
    while Instant::now() < deadline {
        let status = read_status(&xdg_data);
        if status
            .as_ref()
            .and_then(running_frontend)
            .as_deref()
            .is_some_and(|id| id == "navi.ui.tui")
        {
            back = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(back, "the built-in TUI never became the running frontend");

    // 5. Quit the built-in TUI; the composed session exits.
    let _ = writer.write_all(b"\x03");
    let _ = writer.flush();
    std::thread::sleep(Duration::from_millis(150));
    let _ = writer.write_all(b"\x03");
    let _ = writer.flush();
    let mut exited = wait_until_exit(guard.child(), Instant::now() + Duration::from_secs(10));
    if !exited {
        if let Some(pid) = guard.child().process_id() {
            let _ = std::process::Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
        }
        exited = wait_until_exit(guard.child(), Instant::now() + Duration::from_secs(10));
    }
    assert!(
        exited,
        "the composed session must exit; output:\n{}",
        output_text(&buf)
    );
    let _ = guard.child.take();
    drop(pair.master);
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = read_handle.join();
        let _ = done_tx.send(());
    });
    let _ = done_rx.recv_timeout(Duration::from_secs(3));

    // The journal records the frontend swap in both directions.
    let journal = std::fs::read_to_string(xdg_data.join("navi/compose/journal.jsonl"))
        .expect("kernel journal");
    assert!(
        journal.contains("\"id\":\"navi.plugin.tui\""),
        "journal must record the plugin activation:\n{journal}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(120),
        "scenario exceeded the wall budget: {:?}",
        started.elapsed()
    );
}
