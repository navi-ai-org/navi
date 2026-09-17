//! Process-tier frontends: a third-party TUI in its own process.
//!
//! The in-process frontends (`navi.ui.tui`, `navi.ui.plain`) run as threads of
//! the NAVI process. This module lets a **frontend run as a child process**
//! (`kind = ui`, `execution.entrypoint` = a real binary): the child owns the
//! terminal directly (it does raw mode / alternate screen itself), while the
//! host keeps the composition contract — the kernel still activates/withdraws
//! generations, and the host still enforces the exclusive terminal lease
//! (bookkeeping-only for this tier) and admits the child's commands through the
//! kernel.
//!
//! Control channel: a Unix socket the child connects back to. The host sends
//! engine events and control messages (`quiesce`) as JSON lines; the child sends
//! commands back the same way.
//!
//! ```text
//!   host (navi)                              child (plugin TUI)
//!   ├── spawns with inherited stdio ───────► terminal: raw mode + render
//!   ├── listens on ui/<id>-<gen>.sock ◄──── connects back
//!   ├── {"type":"event", …} ──────────────► state update
//!   ├── {"type":"quiesce"} ───────────────► restore terminal, exit 0
//!   └── {"type":"command","text":…} ◄────── user submitted a message
//! ```

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use navi_tui::host::{ControlMsg, UiExit, UiHost};

use crate::ui::UiFrontend;

/// A frontend that runs as a child process owning the terminal.
pub struct ProcessFrontend {
    id: String,
    entrypoint: PathBuf,
    args: Vec<String>,
    /// Where to create the control socket (`{data_dir}/compose/ui`).
    control_dir: PathBuf,
}

impl ProcessFrontend {
    /// Builds a process frontend for `id`.
    pub fn new(
        id: impl Into<String>,
        entrypoint: impl Into<PathBuf>,
        args: Vec<String>,
        control_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            id: id.into(),
            entrypoint: entrypoint.into(),
            args,
            control_dir: control_dir.into(),
        }
    }

    fn socket_path(&self, generation: u64) -> PathBuf {
        self.control_dir
            .join(format!("{}-{generation}.sock", self.id))
    }

    /// Expands `{control}`/`{session}` markers in the configured args.
    fn expand_args(&self, control: &str, session: Option<&str>) -> Vec<String> {
        self.args
            .iter()
            .map(|arg| {
                arg.replace("{control}", control)
                    .replace("{session}", session.unwrap_or(""))
                    .replace("{id}", &self.id)
            })
            .collect()
    }
}

impl UiFrontend for ProcessFrontend {
    fn id(&self) -> &str {
        &self.id
    }

    fn requires_tty(&self) -> bool {
        true
    }

    fn child_owns_terminal(&self) -> bool {
        // The child enters raw mode itself; the host keeps ownership
        // bookkeeping only (exclusive lease, released on withdraw).
        true
    }

    fn run(&self, host: UiHost) -> UiExit {
        if let Err(err) = std::fs::create_dir_all(&self.control_dir) {
            eprintln!("[{}] cannot create control dir: {err}", self.id);
            return UiExit::Error;
        }
        let socket_path = self.socket_path(host.generation);
        let _ = std::fs::remove_file(&socket_path);
        let listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(err) => {
                eprintln!("[{}] cannot bind control socket: {err}", self.id);
                return UiExit::Error;
            }
        };
        let session = host.session_id.clone();
        let args = self.expand_args(&socket_path.to_string_lossy(), session.as_deref());
        let mut child = match Command::new(&self.entrypoint)
            .args(&args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                eprintln!(
                    "[{}] cannot spawn {}: {err}",
                    self.id,
                    self.entrypoint.display()
                );
                return UiExit::Error;
            }
        };
        tracing::info!(
            component = %self.id,
            pid = child.id(),
            generation = host.generation,
            "process frontend spawned"
        );

        // The child connects back as soon as it is ready.
        let stream = match accept_with_timeout(&listener, &mut child, Duration::from_secs(10)) {
            Ok(stream) => stream,
            Err(err) => {
                eprintln!("[{}] no control connection: {err}", self.id);
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&socket_path);
                return UiExit::Error;
            }
        };
        let writer = match stream.try_clone() {
            Ok(writer) => writer,
            Err(err) => {
                eprintln!("[{}] cannot clone control stream: {err}", self.id);
                let _ = child.kill();
                let _ = child.wait();
                return UiExit::Error;
            }
        };

        // Child → host: commands (and a courtesy "quit").
        let (tx, rx) = mpsc::channel::<String>();
        let reader = std::thread::Builder::new()
            .name(format!("navi-ui-proc-{}", self.id))
            .spawn(move || {
                let reader = BufReader::new(stream);
                for line in reader.lines().map_while(Result::ok) {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            })
            .ok();

        // Host → child: engine events.
        let events_writer = writer.try_clone().ok();
        if let Some(session_id) = host.session_id.clone()
            && let Ok(mut events) = host.driver.subscribe_events(&session_id)
            && let Some(mut writer) = events_writer
            && tokio::runtime::Handle::try_current().is_ok()
        {
            tokio::spawn(async move {
                while let Ok(event) = events.recv().await {
                    let line = serde_json::json!({
                        "type": "event",
                        "event": format!("{event:?}"),
                    });
                    if writeln!(writer, "{line}").is_err() {
                        break;
                    }
                }
            });
        }

        let mut writer = writer;
        let exit = loop {
            // Control from the supervisor (swap/shutdown).
            if let Some(message) = host.control.poll() {
                match message {
                    ControlMsg::Quiesce { .. } => {
                        let _ = writeln!(writer, "{}", serde_json::json!({"type": "quiesce"}));
                        let _ = writer.flush();
                        let _ = wait_or_kill(&mut child, Duration::from_secs(3));
                        let _ = std::fs::remove_file(&socket_path);
                        return UiExit::Quiesced;
                    }
                    ControlMsg::Shutdown => {
                        let _ = writeln!(writer, "{}", serde_json::json!({"type": "shutdown"}));
                        let _ = writer.flush();
                        let _ = wait_or_kill(&mut child, Duration::from_secs(3));
                        let _ = std::fs::remove_file(&socket_path);
                        return UiExit::Quit;
                    }
                }
            }
            // Commands from the child: admitted through the kernel (I02/I03).
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => {
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                        continue;
                    };
                    match value.get("type").and_then(|kind| kind.as_str()) {
                        Some("command") => {
                            let text = value
                                .get("text")
                                .and_then(|text| text.as_str())
                                .unwrap_or("")
                                .to_string();
                            match host.commands.submit(&text) {
                                Ok(()) => {
                                    let _ = writeln!(
                                        writer,
                                        "{}",
                                        serde_json::json!({"type": "accepted", "text": text})
                                    );
                                }
                                Err(err) => {
                                    let _ = writeln!(
                                        writer,
                                        "{}",
                                        serde_json::json!({"type": "denied", "reason": err})
                                    );
                                }
                            }
                            let _ = writer.flush();
                        }
                        Some("quit") => break UiExit::Quit,
                        _ => {}
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // The child closed the control connection: it is gone (or
                    // about to be). Its status decides whether this is a clean
                    // exit or a crash the supervisor must restart.
                    break exit_from_child(&mut child);
                }
            }
            // Child exited on its own.
            match child.try_wait() {
                Ok(Some(status)) => {
                    tracing::info!(component = %self.id, ?status, "process frontend exited");
                    // A non-zero status is a crash: the supervisor must treat it
                    // as a failure (restart per policy), not as a user quit.
                    break if status.success() {
                        UiExit::Quit
                    } else {
                        UiExit::Error
                    };
                }
                Ok(None) => {}
                Err(_) => break UiExit::Error,
            }
        };
        let _ = wait_or_kill(&mut child, Duration::from_secs(2));
        let _ = std::fs::remove_file(&socket_path);
        if let Some(reader) = reader {
            let _ = reader.join();
        }
        exit
    }
}

/// Accepts the child's control connection, aborting if it exits first.
fn accept_with_timeout(
    listener: &UnixListener,
    child: &mut Child,
    timeout: Duration,
) -> Result<UnixStream, String> {
    listener
        .set_nonblocking(true)
        .map_err(|err| format!("nonblocking: {err}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                return Ok(stream);
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(format!("child exited before connecting: {status}"));
                }
                if Instant::now() >= deadline {
                    return Err("timed out waiting for the control connection".to_string());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(err) => return Err(format!("accept: {err}")),
        }
    }
}

/// Waits (briefly) for the child to be reaped and classifies its exit.
///
/// A clean exit (status 0) is a user quit; anything else — a signal, a panic,
/// a non-zero status — is a failure the supervisor must restart per policy.
fn exit_from_child(child: &mut Child) -> UiExit {
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    UiExit::Quit
                } else {
                    tracing::warn!(?status, "process frontend child failed");
                    UiExit::Error
                };
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => return UiExit::Quit,
            Err(_) => return UiExit::Error,
        }
    }
}

/// Waits for the child to exit, killing it (and its group) on timeout.
fn wait_or_kill(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    false
}
