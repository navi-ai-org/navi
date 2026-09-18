//! Example third-party NAVI **system-prompt builder**, running out of process.
//!
//! The engine asks this process to render the base instructions (and optional
//! developer blocks) for every turn. The host sends a *projection* of
//! `SystemPromptInput` — the data, not the live objects (prompt cache, config):
//!
//! ```json
//! // host → plugin
//! {"op":"build","project_dir":"/p","tools":["read_file"],"memory":null,
//!  "harness_card":null,"system_prompt":null,
//!  "context_packets":[…],"available_skills":[…],"active_skills":[…]}
//! // plugin → host
//! {"instructions":"…","developer_messages":[{"role":"developer","content":"…"}]}
//! ```
//!
//! An empty or unreadable answer makes the engine fall back to its own builder,
//! so a broken prompt component degrades instead of breaking the session.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use matrix_component::{CallCtx, CallOutcome, Component, Handler};
use serde_json::{Value, json};

struct Prompt {
    id: String,
    log: Option<PathBuf>,
    builds: Mutex<u64>,
}

impl Prompt {
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

    fn names(value: Option<&Value>, field: &str) -> Vec<String> {
        value
            .and_then(|value| value.as_array())
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        entry
                            .get(field)
                            .or_else(|| entry.get("name"))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Handler for Prompt {
    fn on_call(
        &self,
        ctx: &CallCtx,
        _ticket: &str,
        cap: &str,
        input: &Value,
        _cancel: &AtomicBool,
    ) -> CallOutcome {
        match cap {
            "navi.prompt@1" => {
                let op = input.get("op").and_then(Value::as_str).unwrap_or("");
                if op != "build" {
                    return CallOutcome::Err {
                        code: "invalid-message".into(),
                        message: format!("unknown operation `{op}`"),
                    };
                }
                let n = {
                    let mut builds = self.builds.lock().expect("builds");
                    *builds += 1;
                    *builds
                };
                let project = input
                    .get("project_dir")
                    .and_then(Value::as_str)
                    .unwrap_or(".");
                let tools = Self::names(input.get("tools"), "name");
                let skills = Self::names(input.get("active_skills"), "name");
                let override_prompt = input.get("system_prompt").and_then(Value::as_str);
                let memory = input.get("memory").and_then(Value::as_str);
                self.note(&format!(
                    "build #{n} project={project} tools={} skills={} generation={}",
                    tools.len(),
                    skills.len(),
                    ctx.generation
                ));

                // The base identity. A host-supplied override wins, so the
                // component composes with the session instead of ignoring it.
                let mut instructions = match override_prompt {
                    Some(override_prompt) => format!(
                        "{override_prompt}\n\n[rendered by {} in pid {}]",
                        self.id,
                        std::process::id()
                    ),
                    None => format!(
                        "You are NAVI, driven by the third-party prompt component {} \
                         (pid {}). You work in {project} with {} tools. Be direct, verify \
                         before claiming success, and keep the user informed.",
                        self.id,
                        std::process::id(),
                        tools.len()
                    ),
                };
                if let Some(memory) = memory
                    && !memory.trim().is_empty()
                {
                    instructions.push_str("\n\nRemembered context:\n");
                    instructions.push_str(memory);
                }

                let mut developer_messages = Vec::new();
                if !skills.is_empty() {
                    developer_messages.push(json!({
                        "role": "developer",
                        "content": format!(
                            "Active skills this session (from the prompt component): {}",
                            skills.join(", ")
                        ),
                    }));
                }
                developer_messages.push(json!({
                    "role": "developer",
                    "content": format!(
                        "Tool catalog size: {}. Project root: {project}.",
                        tools.len()
                    ),
                }));

                CallOutcome::Ok(json!({
                    "instructions": instructions,
                    "developer_messages": developer_messages,
                    "plugin_pid": std::process::id(),
                }))
            }
            "navi.plugin.info@1" => CallOutcome::Ok(json!({
                "id": self.id,
                "pid": std::process::id(),
                "generation": ctx.generation,
                "kind": "prompt",
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
        eprintln!("usage: navi-plugin-prompt --matrix-sock <path> --id <logical> [--log <path>]");
        std::process::exit(2);
    };
    let Some(id) = arg_value(&args, "--id") else {
        eprintln!("usage: navi-plugin-prompt --matrix-sock <path> --id <logical> [--log <path>]");
        std::process::exit(2);
    };
    let log = arg_value(&args, "--log").map(PathBuf::from);
    let plugin = Prompt {
        id: id.clone(),
        log: log.clone(),
        builds: Mutex::new(0),
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
            eprintln!("navi-plugin-prompt: serve failed: {err}");
            std::process::exit(1);
        }
    }
}
