//! Frontends as components.
//!
//! A frontend implements [`UiFrontend`] and is registered under its component
//! id. The compose supervisor starts it on `Activated`, quiesces it on
//! `Withdrawn`, and hands it a [`UiHost`] with the engine driver, the terminal
//! lease, the control channel and the kernel-mediated command sink.

use std::io::BufRead;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use navi_tui::host::{UiExit, UiHost};
use navi_tui::TuiApp;

use crate::manifest::{ComponentKind, ComponentManifest, Execution, RequirementSpec};

/// Versioned interface every frontend implements.
pub const UI_INTERFACE: &str = "navi.ui.frontend";

/// A runnable frontend.
pub trait UiFrontend: Send + Sync {
    /// Component id (`navi.ui.tui`).
    fn id(&self) -> &str;

    /// Whether this frontend needs the exclusive terminal lease.
    fn requires_tty(&self) -> bool {
        true
    }

    /// Runs until the user quits, the supervisor quiesces the generation, or an
    /// error stops it. Blocking: the supervisor runs it on a dedicated thread.
    fn run(&self, host: UiHost) -> UiExit;
}

/// The ratatui frontend (`navi.ui.tui`), the default NAVI TUI.
pub struct TuiFrontend;

impl UiFrontend for TuiFrontend {
    fn id(&self) -> &str {
        "navi.ui.tui"
    }

    fn run(&self, host: UiHost) -> UiExit {
        let app = match TuiApp::new_with_engine(
            host.loaded_config.clone(),
            host.project_dir.clone(),
            host.task.clone(),
            host.active_skills.clone(),
            host.driver.clone(),
        ) {
            Ok(app) => app,
            Err(err) => {
                eprintln!("[navi.ui.tui] failed to start: {err}");
                return UiExit::Error;
            }
        };
        match navi_tui::run_with_host(app, host) {
            Ok(exit) => exit,
            Err(err) => {
                eprintln!("[navi.ui.tui] run failed: {err}");
                UiExit::Error
            }
        }
    }
}

/// A line-based frontend (`navi.ui.plain`).
///
/// No ratatui, no raw mode, no terminal lease: it exists to prove that a
/// frontend is *any* component satisfying the contract, and it is the
/// deterministic target for swap tests (stdin can be a pipe).
pub struct PlainFrontend;

impl UiFrontend for PlainFrontend {
    fn id(&self) -> &str {
        "navi.ui.plain"
    }

    fn requires_tty(&self) -> bool {
        false
    }

    fn run(&self, host: UiHost) -> UiExit {
        println!(
            "[navi.ui.plain] generation={} session={} (type a message, `exit` to quit)",
            host.generation,
            host.session_id.as_deref().unwrap_or("<new>"),
        );

        // Live event printer: the frontend subscribes to the engine's event
        // stream through the *driver* (data plane), while commands are admitted
        // through the kernel (control plane). Only when a runtime is available:
        // headless hosts may run frontends on plain threads.
        if let Some(session_id) = host.session_id.clone()
            && let Ok(mut events) = host.driver.subscribe_events(&session_id)
            && tokio::runtime::Handle::try_current().is_ok()
        {
            tokio::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(event) => println!("[event] {}", summarize_event(&event)),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            println!("[event] lagged, skipped {skipped}");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }

        // stdin is blocking: read it on a helper thread so the loop can poll
        // the control channel (quiesce) every 50ms.
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                match line {
                    Ok(line) => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        loop {
            if let Some(message) = host.control.poll() {
                match message {
                    navi_tui::host::ControlMsg::Quiesce { .. } => return UiExit::Quiesced,
                    navi_tui::host::ControlMsg::Shutdown => return UiExit::Quit,
                }
            }
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => {
                    let line = line.trim().to_string();
                    if line.is_empty() {
                        continue;
                    }
                    if line == "exit" || line == "quit" {
                        return UiExit::Quit;
                    }
                    match host.commands.submit(&line) {
                        Ok(()) => println!("[accepted] {line}"),
                        Err(err) => println!("[denied] {err}"),
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return UiExit::Quit,
            }
        }
    }
}

fn summarize_event(event: &navi_sdk::RuntimeEvent) -> String {
    use navi_sdk::RuntimeEventKind as K;
    match &event.kind {
        K::SessionStarted { session_id } => format!("session-started {session_id}"),
        K::TurnStarted { turn_id } => format!("turn-started {turn_id}"),
        K::AssistantDelta { text } => format!("delta {}", text.trim_end()),
        K::ToolRequested(invocation) => format!("tool-requested {}", invocation.tool_name),
        K::ToolCompleted(result) => format!("tool-completed ok={}", result.ok),
        K::TurnCompleted { .. } => "turn-completed".to_string(),
        other => format!("{other:?}"),
    }
}

/// The engine component (`navi.engine`).
///
/// Provides the driver/events/command interfaces every frontend binds to.
pub fn engine_manifest() -> ComponentManifest {
    ComponentManifest {
        id: "navi.engine".to_string(),
        version: "0.8.0".to_string(),
        description: "NAVI agent engine (sessions, turns, tools, providers)".to_string(),
        capabilities: vec![
            "navi.engine.driver@1".to_string(),
            "navi.engine.events@1".to_string(),
            "navi.engine.command@1".to_string(),
        ],
        requires: Vec::new(),
        subscriptions: Vec::new(),
        execution: Execution::inproc("navi.engine"),
        tier: "core".into(),
        trust: "trusted".into(),
        restart: "permanent".into(),
        calls: Default::default(),
        kind: ComponentKind::Engine,
    }
}

/// Manifest for a frontend component.
pub fn ui_manifest(
    id: &str,
    description: &str,
    command_capability: &str,
    requires: Vec<RequirementSpec>,
) -> ComponentManifest {
    let mut manifest = ComponentManifest::ui(id, description, requires);
    manifest.capabilities.push(command_capability.to_string());
    manifest
}

/// The built-in component set shipped by this prototype.
pub fn default_manifests() -> Vec<ComponentManifest> {
    let engine_requires = vec![
        RequirementSpec::Detailed {
            interface: "navi.engine.driver".into(),
            major: Some(1),
            provider: Some("navi.engine".into()),
        },
        RequirementSpec::Detailed {
            interface: "navi.engine.events".into(),
            major: Some(1),
            provider: Some("navi.engine".into()),
        },
    ];
    let mut compaction = ComponentManifest {
        id: "navi.compaction.default".to_string(),
        version: "0.8.0".to_string(),
        description: "Default NAVI compaction strategy (swappable at runtime)".to_string(),
        capabilities: vec!["navi.compaction@1".to_string()],
        requires: Vec::new(),
        subscriptions: Vec::new(),
        execution: Execution::inproc("navi.compaction.default"),
        tier: "core".into(),
        trust: "trusted".into(),
        restart: "on-failure".into(),
        calls: Default::default(),
        kind: ComponentKind::Compaction,
    };
    compaction.calls.insert(
        "navi.compaction@1".to_string(),
        crate::manifest::CallPolicySpec {
            on_withdraw: Some("drain".to_string()),
            drain_ms: Some(250),
        },
    );

    vec![
        engine_manifest(),
        ui_manifest(
            "navi.ui.tui",
            "ratatui terminal frontend",
            "navi.ui.command.tui@1",
            engine_requires.clone(),
        ),
        ui_manifest(
            "navi.ui.plain",
            "line-based frontend (no TTY lease)",
            "navi.ui.command.plain@1",
            engine_requires,
        ),
        compaction,
        terse_compaction(),
        branded_prompt(),
    ]
}

/// A second provider of `navi.compaction@1`: activating it swaps the engine's
/// compaction strategy at runtime (most recently activated provider wins).
pub fn terse_compaction() -> ComponentManifest {
    let mut manifest = ComponentManifest {
        id: "navi.compaction.terse".to_string(),
        version: "0.1.0".to_string(),
        description: "Aggressive compaction: clears every old tool result".to_string(),
        capabilities: vec!["navi.compaction@1".to_string()],
        requires: Vec::new(),
        subscriptions: Vec::new(),
        execution: Execution::inproc("navi.compaction.terse"),
        tier: "core".into(),
        trust: "trusted".into(),
        restart: "on-failure".into(),
        calls: Default::default(),
        kind: ComponentKind::Compaction,
    };
    manifest.calls.insert(
        "navi.compaction@1".to_string(),
        crate::manifest::CallPolicySpec {
            on_withdraw: Some("drain".to_string()),
            drain_ms: Some(250),
        },
    );
    manifest
}

/// A provider of `navi.prompt@1` that stamps a marker into the base
/// instructions, so a prompt swap is observable.
pub fn branded_prompt() -> ComponentManifest {
    ComponentManifest {
        id: "navi.prompt.branded".to_string(),
        version: "0.1.0".to_string(),
        description: "System-prompt builder that stamps a marker into the base instructions"
            .to_string(),
        capabilities: vec!["navi.prompt@1".to_string()],
        requires: Vec::new(),
        subscriptions: Vec::new(),
        execution: Execution::inproc("navi.prompt.branded"),
        tier: "core".into(),
        trust: "trusted".into(),
        restart: "on-failure".into(),
        calls: Default::default(),
        kind: ComponentKind::Prompt,
    }
}

/// Command capability for a frontend component id.
pub fn command_capability(component_id: &str) -> String {
    let suffix = component_id
        .strip_prefix("navi.ui.")
        .unwrap_or(component_id)
        .replace('.', "-");
    format!("navi.ui.command.{suffix}@1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_manifests_are_internally_consistent() {
        let manifests = default_manifests();
        let ids: Vec<&str> = manifests.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"navi.engine"));
        assert!(ids.contains(&"navi.ui.tui"));
        assert!(ids.contains(&"navi.ui.plain"));
        // Every declared requirement must be provided by some component.
        let provided: Vec<String> = manifests
            .iter()
            .flat_map(|m| m.capabilities.clone())
            .collect();
        for manifest in &manifests {
            for requirement in &manifest.requires {
                let iface = requirement.interface();
                let iface_no_major = iface.split('@').next().unwrap_or(&iface);
                assert!(
                    provided.iter().any(|cap| cap.starts_with(iface_no_major)),
                    "{} requires {iface} but nothing provides it",
                    manifest.id
                );
            }
        }
    }

    #[test]
    fn command_capability_is_derived_from_component_id() {
        assert_eq!(command_capability("navi.ui.tui"), "navi.ui.command.tui@1");
        assert_eq!(command_capability("navi.ui.plain"), "navi.ui.command.plain@1");
    }
}
