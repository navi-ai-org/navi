//! Example third-party NAVI **security policy and session hooks**, running out
//! of process.
//!
//! This binary is not compiled into NAVI. The compose supervisor activates it as
//! a process component; the host installs a proxy into the engine's security
//! slot (and its hooks slot), and the engine asks *this process* before every
//! tool invocation.
//!
//! Contracts:
//!
//! ```json
//! // navi.security@1 — host → plugin
//! {"op":"validate_tool","tool":{"name":"web_fetch",…},"input":{"url":"…"}}
//! // plugin → host
//! {"decision":"deny","reason":"…"} | {"decision":"allow"}
//!                                  | {"decision":"approval","risk":"command"}
//!                                  | {"decision":"abstain"}
//!
//! // navi.hooks@1 — host → plugin (fire and forget)
//! {"op":"event","event":"tool_call","tool":"read_file"}
//! ```
//!
//! `abstain` means "the base policy decides": a security plugin adds
//! restrictions without re-implementing the host's rules. A plugin that stops
//! answering is ignored (the engine keeps its default policy), so a broken
//! security component never silently allows everything.
//!
//! Policy implemented here: deny by tool **name or prefix** (the set comes from
//! `--deny`, default `run,web_,browser`), abstain for everything else. Every
//! decision is written to `--log`, which is how a test proves the answer came
//! from this process.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use matrix_component::{CallCtx, CallOutcome, Component, Handler};
use serde_json::{Value, json};

struct Policy {
    id: String,
    denied: Vec<String>,
    log: Option<PathBuf>,
    decisions: Mutex<u64>,
}

impl Policy {
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

    /// Whether the tool matches the deny list (exact name or prefix).
    fn denied(&self, tool: &str) -> bool {
        self.denied
            .iter()
            .any(|entry| tool == entry || tool.starts_with(entry.as_str()))
    }
}

impl Handler for Policy {
    fn on_call(
        &self,
        ctx: &CallCtx,
        _ticket: &str,
        cap: &str,
        input: &Value,
        _cancel: &AtomicBool,
    ) -> CallOutcome {
        match cap {
            "navi.security@1" => {
                let op = input.get("op").and_then(Value::as_str).unwrap_or("");
                if op != "validate_tool" {
                    return CallOutcome::Err {
                        code: "invalid-message".into(),
                        message: format!("unknown operation `{op}`"),
                    };
                }
                let tool = input
                    .get("tool")
                    .and_then(|tool| tool.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let n = {
                    let mut decisions = self.decisions.lock().expect("decisions");
                    *decisions += 1;
                    *decisions
                };
                let answer = if self.denied(tool) {
                    self.note(&format!(
                        "decision #{n} tool={tool} deny generation={}",
                        ctx.generation
                    ));
                    json!({
                        "decision": "deny",
                        "reason": format!(
                            "denied by {} (policy in another process, pid {})",
                            self.id,
                            std::process::id()
                        ),
                        "plugin_pid": std::process::id(),
                    })
                } else {
                    self.note(&format!("decision #{n} tool={tool} abstain"));
                    json!({"decision": "abstain", "plugin_pid": std::process::id()})
                };
                CallOutcome::Ok(answer)
            }
            // Session hooks: audit lines in this process's log.
            "navi.hooks@1" => {
                let event = input.get("event").and_then(Value::as_str).unwrap_or("");
                self.note(&format!("hook {event} {input}"));
                CallOutcome::Ok(json!({"ok": true, "plugin_pid": std::process::id()}))
            }
            "navi.plugin.info@1" => CallOutcome::Ok(json!({
                "id": self.id,
                "pid": std::process::id(),
                "generation": ctx.generation,
                "kind": "security",
                "denied": self.denied,
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
        eprintln!("usage: navi-plugin-security --matrix-sock <path> --id <logical> [--log <path>] [--deny <a,b>]");
        std::process::exit(2);
    };
    let Some(id) = arg_value(&args, "--id") else {
        eprintln!("usage: navi-plugin-security --matrix-sock <path> --id <logical> [--log <path>] [--deny <a,b>]");
        std::process::exit(2);
    };
    let log = arg_value(&args, "--log").map(PathBuf::from);
    let denied: Vec<String> = arg_value(&args, "--deny")
        .unwrap_or_else(|| "run,web_,browser".to_string())
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect();
    let plugin = Policy {
        id: id.clone(),
        denied,
        log: log.clone(),
        decisions: Mutex::new(0),
    };
    plugin.note(&format!(
        "start id={id} pid={} sock={sock} deny={:?}",
        std::process::id(),
        plugin.denied
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
            eprintln!("navi-plugin-security: serve failed: {err}");
            std::process::exit(1);
        }
    }
}
