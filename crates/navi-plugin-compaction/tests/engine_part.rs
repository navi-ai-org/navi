//! An **engine part served by a third-party process**.
//!
//! The compaction strategy is not a Rust object compiled into NAVI: the compose
//! supervisor activates a `kind = compaction` component, the Matrix process host
//! spawns the plugin, and the host installs a proxy into the engine's compaction
//! slot. The engine then asks *that process* what to compact — and the answer is
//! applied on the host side, so nothing the engine owns is lost.
//!
//! Gated to Linux (process tier).

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use navi_compose::{
    ComponentKind, ComponentManifest, Compose, ComposeOptions, ComposeCompaction, Execution,
};
use navi_core::{
    CompactionStrategy, DefaultCompactionStrategy, LoadedConfig, ModelMessage, ModelRole,
    NaviConfig,
};
use serde_json::{Value, json};

const PLUGIN_ID: &str = "navi.plugin.compaction";

fn test_loaded_config(dir: &Path) -> LoadedConfig {
    let mut config = NaviConfig::default();
    config.updates.check_enabled = false;
    config.registry.update_enabled = false;
    LoadedConfig {
        config,
        global_config_path: None,
        project_config_path: None,
        data_dir: dir.join("data"),
    }
}

fn plugin_manifest(log_path: &Path) -> ComponentManifest {
    ComponentManifest {
        id: PLUGIN_ID.to_string(),
        version: "0.1.0".to_string(),
        description: "Third-party compaction strategy (out of process)".to_string(),
        capabilities: vec![
            "navi.compaction@1".to_string(),
            "navi.plugin.info@1".to_string(),
        ],
        requires: Vec::new(),
        subscriptions: Vec::new(),
        execution: Execution::process_with_args(
            env!("CARGO_BIN_EXE_navi-plugin-compaction"),
            vec![
                "--matrix-sock".to_string(),
                "{sock}".to_string(),
                "--id".to_string(),
                "{id}".to_string(),
                "--log".to_string(),
                log_path.to_string_lossy().to_string(),
            ],
        ),
        tier: "local".to_string(),
        trust: "trusted".to_string(),
        restart: "on-failure".to_string(),
        calls: Default::default(),
        kind: ComponentKind::Compaction,
        sandbox: None,
    }
}

/// Installs the manifest the way `navi components install` does, **before** the
/// session starts (the host policy and the kernel manifest are read from disk).
fn install(home: &Path, manifest: &ComponentManifest) {
    std::fs::create_dir_all(home.join("manifests")).expect("manifests dir");
    std::fs::write(
        home.join("manifests").join(format!("{PLUGIN_ID}.json")),
        manifest.to_kernel_json().to_string(),
    )
    .expect("kernel manifest");
    std::fs::write(
        home.join("manifests").join(format!("{PLUGIN_ID}.toml")),
        manifest.to_toml(),
    )
    .expect("navi manifest");
}

fn compose_for(tempdir: &tempfile::TempDir) -> Compose {
    Compose::new(ComposeOptions {
        home: tempdir.path().join("compose"),
        project_dir: tempdir.path().join("project"),
        loaded_config: test_loaded_config(tempdir.path()),
        dry_run: false,
        terminal_effects: false,
        process_components: true,
    })
    .expect("compose")
}

/// Polls a capability until the child has connected and answered.
fn invoke_until_ready(compose: &Compose, capability: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = json!({"code": "not-started"});
    while Instant::now() < deadline {
        let (value, ok) = compose.kernel().invoke(capability, &json!({}));
        if ok {
            return value;
        }
        last = value;
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("plugin never became reachable; last response: {last}");
}

fn message(role: ModelRole, content: &str, tool: Option<&str>, age_minutes: u64) -> ModelMessage {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    ModelMessage {
        role,
        content: content.to_string(),
        content_parts: Vec::new(),
        tool_call_id: tool.map(|name| format!("call-{name}")),
        tool_name: tool.map(str::to_string),
        tool_calls: Vec::new(),
        created_at: Some(now.saturating_sub(age_minutes.saturating_mul(60_000))),
        thinking_content: None,
    }
}

/// The conversation the plugin is asked to compact: an old tool result and an
/// old assistant message (outside the plugin's window) plus a recent tail.
fn conversation() -> Vec<ModelMessage> {
    vec![
        message(ModelRole::System, "you are NAVI", None, 600),
        message(ModelRole::Tool, "file contents", Some("read_file"), 120),
        message(ModelRole::Assistant, "I read the file", None, 120),
        message(ModelRole::Tool, "patched", Some("edit"), 1),
        message(ModelRole::User, "carry on", None, 1),
        message(ModelRole::Assistant, "working on it", None, 1),
        message(ModelRole::User, "thanks", None, 1),
    ]
}

/// Ages every message, so the *built-in* strategy's idle gate (which looks at
/// the newest assistant message) is satisfied too.
fn age_all(messages: &mut [ModelMessage], minutes: u64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    for message in messages {
        message.created_at = Some(now.saturating_sub(minutes.saturating_mul(60_000)));
    }
}

#[test]
fn a_plugin_process_can_be_the_engines_compaction_strategy() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let log_path = tempdir.path().join("compaction.log");
    let home = tempdir.path().join("compose");
    install(&home, &plugin_manifest(&log_path));

    let compose = compose_for(&tempdir);
    let outcome = compose.activate(PLUGIN_ID).expect("activate");
    assert_eq!(outcome.state, "Active");
    assert_eq!(outcome.generation, Some(1));

    // The child needs a moment to connect; the slot is installed on activation.
    let info = invoke_until_ready(&compose, "navi.plugin.info@1");
    assert_eq!(info["kind"], "compaction");
    assert_ne!(
        info["pid"].as_u64(),
        Some(std::process::id() as u64),
        "the strategy must be decided out of process: {info}"
    );

    let (provider, generation, strategy) = compose
        .slots()
        .compaction
        .current()
        .expect("the plugin must own the compaction slot");
    assert_eq!(provider, PLUGIN_ID);
    assert_eq!(generation, 1);

    // The engine calls the strategy; the plugin decides, the host applies.
    let mut messages = conversation();
    let before = messages.len();
    let changed = strategy.micro_compact(&mut messages, 60);

    assert_eq!(
        changed, 2,
        "one stale tool result dropped + one stale assistant message cleared"
    );
    assert_eq!(messages.len(), before - 1);
    assert!(
        messages.iter().all(|message| message.tool_name.as_deref() != Some("read_file")),
        "the stale tool result is gone: {messages:?}"
    );
    assert_eq!(messages[0].role, ModelRole::System);
    assert_eq!(messages[0].content, "you are NAVI", "system stays intact");
    let cleared = messages
        .iter()
        .find(|message| message.role == ModelRole::Assistant)
        .expect("assistant message");
    assert!(
        cleared.content.is_empty(),
        "the stale assistant prose was cleared, the turn kept: {messages:?}"
    );
    // Host-owned metadata survives the round trip.
    assert!(
        messages.iter().all(|message| message.created_at.is_some()),
        "timestamps are host-side and must not be lost"
    );

    // The decision really came from the other process.
    let log = std::fs::read_to_string(&log_path).expect("plugin log");
    assert!(
        log.contains("cap=navi.compaction@1"),
        "the plugin served the compaction call:\n{log}"
    );
    assert!(
        log.contains("plan messages=7 drop=[1] cleared=[2]"),
        "the plugin planned the compaction:\n{log}"
    );
}

#[test]
fn withdrawing_the_plugin_falls_back_to_the_built_in_strategy() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let log_path = tempdir.path().join("compaction.log");
    let home = tempdir.path().join("compose");
    install(&home, &plugin_manifest(&log_path));

    let compose = compose_for(&tempdir);
    compose.activate(PLUGIN_ID).expect("activate");
    invoke_until_ready(&compose, "navi.plugin.info@1");

    // The engine holds a proxy that resolves the slot per call.
    let proxy = ComposeCompaction::new(
        compose.slots(),
        Arc::new(DefaultCompactionStrategy) as Arc<dyn CompactionStrategy>,
    );

    let mut messages = conversation();
    let plugin_changed = proxy.micro_compact(&mut messages, 60);
    assert_eq!(plugin_changed, 2, "the plugin strategy is active");
    assert_eq!(
        compose.slots().compaction.current().map(|(id, _, _)| id),
        Some(PLUGIN_ID.to_string())
    );

    // Withdrawing the plugin pops the slot: the same proxy now uses the
    // built-in strategy (which clears read-only tool results in place, and only
    // when the conversation has been idle past the gap).
    compose.withdraw(PLUGIN_ID).expect("withdraw");
    assert!(
        compose.slots().compaction.current().is_none(),
        "the slot fell back to the proxy's fallback"
    );
    let mut messages = conversation();
    age_all(&mut messages, 120);
    let before = messages.len();
    let default_changed = proxy.micro_compact(&mut messages, 60);
    assert_eq!(
        messages.len(),
        before,
        "the built-in strategy clears content instead of dropping messages"
    );
    assert_eq!(
        default_changed, 1,
        "the built-in strategy cleared the stale read-only tool result"
    );
}
