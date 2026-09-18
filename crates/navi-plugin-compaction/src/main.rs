//! Example third-party NAVI engine part — a **compaction strategy running out
//! of process**.
//!
//! This binary is not compiled into NAVI: the compose supervisor activates it as
//! a `kind = compaction` component, and the host installs a
//! [`navi_compose::ProcessCompaction`] proxy into the engine's compaction slot.
//! The engine then asks *this process* what to compact.
//!
//! The plugin decides, the host applies. On every call it receives the
//! conversation plus the gap threshold, and answers with indices to drop or to
//! clear — so the host keeps every field it owns (timestamps, thinking blocks,
//! multimodal parts) exactly as it was:
//!
//! ```json
//! // host → plugin
//! {"messages": [{"role":"tool","content":"…","tool_name":"read_file"}],
//!  "timestamps": [1758000000000], "gap_threshold_minutes": 60}
//! // plugin → host
//! {"drop": [0], "cleared": [2]}
//! ```
//!
//! Policy implemented here ("window"):
//!
//! * keep the newest `WINDOW` messages untouched;
//! * drop tool results older than the gap threshold (their content is what fills
//!   the context);
//! * clear (keep the message, empty the content) assistant/tool noise older than
//!   the threshold;
//! * never touch system messages or the newest messages.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use matrix_component::{CallCtx, CallOutcome, Component, Handler};
use serde_json::{Value, json};

/// How many of the newest messages are never touched.
const WINDOW: usize = 4;

struct WindowCompaction {
    id: String,
    log: Option<PathBuf>,
    calls: Mutex<u64>,
}

impl WindowCompaction {
    fn note(&self, line: &str) {
        if let Some(path) = &self.log
            && let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
        {
            let _ = writeln!(file, "{line}");
        }
    }

    /// Decides what to compact. Returns `(drop, cleared)`.
    fn plan(&self, messages: &[Value], timestamps: &[Value], gap_minutes: u64) -> (Vec<usize>, Vec<usize>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        let gap_ms = gap_minutes.saturating_mul(60_000);
        let keep_from = messages.len().saturating_sub(WINDOW);
        let mut drop = Vec::new();
        let mut cleared = Vec::new();
        for (index, message) in messages.iter().enumerate() {
            if index >= keep_from {
                break; // the newest window is never touched
            }
            let role = message.get("role").and_then(Value::as_str).unwrap_or("");
            if role == "system" {
                continue; // identity and instructions stay
            }
            let content_len = message
                .get("content")
                .and_then(Value::as_str)
                .map(str::len)
                .unwrap_or(0);
            let created_at = timestamps
                .get(index)
                .and_then(Value::as_u64)
                .unwrap_or(now);
            let stale = now.saturating_sub(created_at) >= gap_ms;
            if !stale || content_len == 0 {
                continue;
            }
            if role == "tool" {
                // Stale tool output is the bulk of the context: drop it whole.
                drop.push(index);
            } else {
                // Assistant narration: keep the turn, empty the prose.
                cleared.push(index);
            }
        }
        (drop, cleared)
    }
}

impl Handler for WindowCompaction {
    fn on_call(
        &self,
        ctx: &CallCtx,
        _ticket: &str,
        cap: &str,
        input: &Value,
        _cancel: &AtomicBool,
    ) -> CallOutcome {
        let n = {
            let mut calls = self.calls.lock().expect("calls");
            *calls += 1;
            *calls
        };
        self.note(&format!(
            "call #{n} cap={cap} generation={} session={}",
            ctx.generation, ctx.session_id
        ));
        match cap {
            "navi.compaction@1" => {
                let messages = input
                    .get("messages")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let timestamps = input
                    .get("timestamps")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let gap = input
                    .get("gap_threshold_minutes")
                    .and_then(Value::as_u64)
                    .unwrap_or(60);
                let (drop, cleared) = self.plan(&messages, &timestamps, gap);
                self.note(&format!(
                    "plan messages={} drop={:?} cleared={:?}",
                    messages.len(),
                    drop,
                    cleared
                ));
                CallOutcome::Ok(json!({
                    "drop": drop,
                    "cleared": cleared,
                    "policy": "window",
                    "window": WINDOW,
                    "plugin_pid": std::process::id(),
                }))
            }
            "navi.plugin.info@1" => CallOutcome::Ok(json!({
                "id": self.id,
                "pid": std::process::id(),
                "generation": ctx.generation,
                "kind": "compaction",
                "calls": n,
            })),
            other => CallOutcome::Err {
                code: "no-such-capability".into(),
                message: format!("plugin {} does not provide {other}", self.id),
            },
        }
    }

    fn on_event(&self, topic: &str, payload: &Value) {
        self.note(&format!("event {topic} {payload}"));
    }
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(sock) = arg_value(&args, "--matrix-sock") else {
        eprintln!("usage: navi-plugin-compaction --matrix-sock <path> --id <logical> [--log <path>]");
        std::process::exit(2);
    };
    let Some(id) = arg_value(&args, "--id") else {
        eprintln!("usage: navi-plugin-compaction --matrix-sock <path> --id <logical> [--log <path>]");
        std::process::exit(2);
    };
    let log = arg_value(&args, "--log").map(PathBuf::from);
    let plugin = WindowCompaction {
        id: id.clone(),
        log: log.clone(),
        calls: Mutex::new(0),
    };
    plugin.note(&format!(
        "start id={id} pid={} sock={sock}",
        std::process::id()
    ));

    let component = match Component::connect(std::path::Path::new(&sock), &id) {
        Ok(component) => component,
        Err(err) => {
            plugin.note(&format!("connect failed: {err}"));
            std::process::exit(1);
        }
    };
    plugin.note(&format!("registered id={id} pid={}", std::process::id()));
    match component.serve(plugin) {
        Ok(()) => std::process::exit(0),
        Err(err) => {
            eprintln!("navi-plugin-compaction: serve failed: {err}");
            std::process::exit(1);
        }
    }
}
