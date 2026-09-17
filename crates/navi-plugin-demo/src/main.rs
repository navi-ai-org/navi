//! Example third-party NAVI component — a **tool plugin running out of process**.
//!
//! This binary is not compiled into NAVI. The compose supervisor installs a
//! manifest pointing at it; the Matrix process host spawns it with a launch
//! token and a Unix socket, and the plugin serves capabilities over
//! `matrix.component` v0.1:
//!
//! * `navi.tool.roll_dice.definition@1` — the tool definition NAVI exposes to
//!   the model (one definition capability per tool, so several plugins can
//!   coexist without ambiguity);
//! * `navi.tool.roll_dice@1` — a deterministic dice roller (the actual tool);
//! * `navi.plugin.info@1` — id/pid/generation/session, so a caller can prove the
//!   work happened in another process and in which generation.
//!
//! Everything the plugin prints goes to `--log <path>` (the host spawns it with
//! stdio attached to /dev/null on purpose).

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use matrix_component::{CallCtx, CallOutcome, Component, Handler};
use serde_json::{Value, json};

const TOOL_NAME: &str = "roll_dice";
const TOOL_CAPABILITY: &str = "navi.tool.roll_dice@1";

struct RollDice {
    id: String,
    log: Option<PathBuf>,
    calls: Mutex<u64>,
}

impl RollDice {
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
}

/// Deterministic 64-bit LCG: same input ⇒ same rolls (tests rely on it).
fn roll(seed: u64, sides: u64, count: u64) -> Vec<u64> {
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    (0..count)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) % sides.max(1) + 1
        })
        .collect()
}

impl Handler for RollDice {
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
            "navi.plugin.info@1" => CallOutcome::Ok(json!({
                "id": self.id,
                "pid": std::process::id(),
                "generation": ctx.generation,
                "instance_id": ctx.instance_id,
                "session_id": ctx.session_id,
                "calls": n,
            })),
            "navi.tool.roll_dice.definition@1" => CallOutcome::Ok(json!({
                "name": TOOL_NAME,
                "description": "Roll dice (deterministic given a seed). Runs in a third-party plugin process.",
                "capability": TOOL_CAPABILITY,
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "sides": {"type": "integer", "description": "Faces per die (default 6)"},
                        "count": {"type": "integer", "description": "Number of dice (default 1, max 20)"},
                        "seed": {"type": "integer", "description": "Deterministic seed (default 1)"}
                    }
                }
            })),
            "navi.tool.roll_dice@1" => {
                let sides = input.get("sides").and_then(Value::as_u64).unwrap_or(6);
                let count = input
                    .get("count")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .min(20);
                let seed = input.get("seed").and_then(Value::as_u64).unwrap_or(1);
                if !(2..=1000).contains(&sides) {
                    return CallOutcome::Err {
                        code: "invalid-message".into(),
                        message: "sides must be between 2 and 1000".into(),
                    };
                }
                let rolls = roll(seed, sides, count);
                let total: u64 = rolls.iter().sum();
                CallOutcome::Ok(json!({
                    "rolls": rolls,
                    "total": total,
                    "sides": sides,
                    "seed": seed,
                    "plugin_pid": std::process::id(),
                    "plugin_generation": ctx.generation,
                }))
            }
            other => CallOutcome::Err {
                code: "no-such-capability".into(),
                message: format!("plugin {} does not provide {other}", self.id),
            },
        }
    }

    fn on_event(&self, topic: &str, payload: &Value) {
        self.note(&format!("event {topic} {}", payload));
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
        eprintln!("usage: navi-plugin-demo --matrix-sock <path> --id <logical> [--log <path>]");
        std::process::exit(2);
    };
    let Some(id) = arg_value(&args, "--id") else {
        eprintln!("usage: navi-plugin-demo --matrix-sock <path> --id <logical> [--log <path>]");
        std::process::exit(2);
    };
    let log = arg_value(&args, "--log").map(PathBuf::from);
    let plugin = RollDice {
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
            eprintln!("navi-plugin-demo: serve failed: {err}");
            std::process::exit(1);
        }
    }
}
