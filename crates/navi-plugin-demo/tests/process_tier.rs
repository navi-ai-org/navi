//! Process tier: a third-party plugin runs as a child process, serves a real
//! tool through the kernel, is swapped by generation, and is killed on withdraw.
//!
//! This is the end-to-end proof of the "plugins" half of the composition story:
//! the plugin binary is **not** compiled into NAVI. The compose supervisor
//! installs a manifest pointing at it, the Matrix process host spawns it with a
//! launch token, and calls reach it over `matrix.component`.

#![cfg(target_os = "linux")]

use std::path::Path;
use std::time::{Duration, Instant};

use navi_compose::{Compose, ComposeOptions, ComponentKind, ComponentManifest, Execution};
use navi_core::{LoadedConfig, NaviConfig};
use serde_json::{Value, json};

const PLUGIN_ID: &str = "navi.plugin.roll_dice";

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
        description: "Third-party dice-rolling tool plugin (out of process)".to_string(),
        capabilities: vec![
            "navi.plugin.info@1".to_string(),
            "navi.tool.roll_dice.definition@1".to_string(),
            "navi.tool.roll_dice@1".to_string(),
        ],
        requires: Vec::new(),
        subscriptions: Vec::new(),
        // `{sock}`/`{id}` are expanded by the process host at spawn time.
        execution: Execution::process_with_args(
            env!("CARGO_BIN_EXE_navi-plugin-demo"),
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
        kind: ComponentKind::Tool,
    }
}

/// Polls a capability until the child has connected and answered.
fn invoke_until_ready(compose: &Compose, capability: &str, input: &Value) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = json!({"code": "not-started"});
    while Instant::now() < deadline {
        let (value, ok) = compose.kernel().invoke(capability, input);
        if ok {
            return value;
        }
        last = value;
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("plugin never became reachable; last response: {last}");
}

#[test]
fn third_party_plugin_runs_out_of_process_and_swaps_by_generation() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let log_path = tempdir.path().join("plugin.log");
    let compose = Compose::new(ComposeOptions {
        home: tempdir.path().join("compose"),
        project_dir: tempdir.path().join("project"),
        loaded_config: test_loaded_config(tempdir.path()),
        dry_run: false,
        terminal_effects: false,
        process_components: true,
    })
    .expect("compose");
    assert!(
        compose.process_host().is_some(),
        "the process host must be attached for third-party components"
    );

    // Install the definition, then activate once: the host spawns the plugin
    // binary for generation 1.
    compose
        .install_definition(plugin_manifest(&log_path))
        .expect("install plugin");
    let outcome = compose.activate(PLUGIN_ID).expect("activate");
    assert_eq!(outcome.state, "Active");
    assert_eq!(outcome.generation, Some(1));

    // The plugin answers from *its own* process.
    let info = invoke_until_ready(&compose, "navi.plugin.info@1", &json!({}));
    let plugin_pid = info["pid"].as_u64().expect("pid");
    assert_ne!(
        plugin_pid,
        std::process::id() as u64,
        "the plugin must run out of process: {info}"
    );
    assert_eq!(info["generation"], 1);
    assert_eq!(info["id"], PLUGIN_ID);

    // The tool itself: deterministic, and reachable through the kernel.
    let first = invoke_until_ready(
        &compose,
        "navi.tool.roll_dice@1",
        &json!({"sides": 6, "count": 3, "seed": 42}),
    );
    assert_eq!(first["rolls"].as_array().map(Vec::len), Some(3));
    assert_eq!(first["plugin_pid"].as_u64(), Some(plugin_pid));
    let again = invoke_until_ready(
        &compose,
        "navi.tool.roll_dice@1",
        &json!({"sides": 6, "count": 3, "seed": 42}),
    );
    assert_eq!(first["rolls"], again["rolls"], "same seed ⇒ same rolls");
    let different = invoke_until_ready(
        &compose,
        "navi.tool.roll_dice@1",
        &json!({"sides": 6, "count": 3, "seed": 7}),
    );
    assert_ne!(first["rolls"], different["rolls"], "different seed ⇒ different rolls");
    for roll in first["rolls"].as_array().expect("rolls") {
        let value = roll.as_u64().expect("roll value");
        assert!((1..=6).contains(&value), "roll out of range: {value}");
    }

    // The tool definition the agent would see comes from the plugin.
    let definition = invoke_until_ready(&compose, "navi.tool.roll_dice.definition@1", &json!({}));
    assert_eq!(definition["name"], "roll_dice");
    assert_eq!(definition["capability"], "navi.tool.roll_dice@1");
    let spec = navi_compose::PluginToolSpec::from_response(PLUGIN_ID, &definition).expect("spec");
    assert_eq!(spec.name, "roll_dice");

    // The supervisor discovers the installed tool plugin and can build a
    // `navi_core::Tool` proxy for it (this is what the CLI registers).
    let plugins = compose.tool_plugins();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].0, PLUGIN_ID);
    assert_eq!(plugins[0].1, "navi.tool.roll_dice@1");
    assert_eq!(plugins[0].2, "navi.tool.roll_dice.definition@1");
    let proxy = navi_compose::PluginToolProxy::new(compose.kernel().clone(), spec);
    let tool_result = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(async {
            use navi_core::tool::Tool;
            let invocation = navi_core::ToolInvocation {
                id: "call-1".to_string(),
                tool_name: "roll_dice".to_string(),
                input: json!({"sides": 20, "count": 1, "seed": 3}),
            };
            proxy.invoke(invocation).await.expect("plugin tool call")
        });
    assert!(tool_result.ok, "plugin tool must succeed: {tool_result:?}");
    assert_eq!(
        tool_result.output["plugin_pid"].as_u64(),
        Some(plugin_pid),
        "the agent-visible tool must run in the plugin process"
    );

    // Runtime swap: a new generation means a new process.
    let reloaded = compose.reload(PLUGIN_ID).expect("reload");
    assert_eq!(reloaded.previous_generation, Some(1));
    assert_eq!(reloaded.generation, Some(2));
    let info_after = invoke_until_ready(&compose, "navi.plugin.info@1", &json!({}));
    assert_eq!(info_after["generation"], 2);
    assert_ne!(
        info_after["pid"].as_u64(),
        Some(plugin_pid),
        "the successor generation must be a fresh process: {info_after}"
    );

    // Withdraw: the child process is killed and the capability stops answering.
    compose.withdraw(PLUGIN_ID).expect("withdraw");
    let host = compose.process_host().expect("process host");
    assert!(
        host.wait_no_child(PLUGIN_ID, Duration::from_secs(5)),
        "the plugin process must be reaped on withdraw"
    );
    let (value, ok) = compose.kernel().invoke("navi.plugin.info@1", &json!({}));
    assert!(!ok, "a withdrawn plugin must not answer: {value}");

    // The plugin's own log shows two process lifetimes and the served calls.
    let log = std::fs::read_to_string(&log_path).expect("plugin log");
    let starts = log.lines().filter(|line| line.starts_with("start ")).count();
    assert_eq!(starts, 2, "one process per generation:\n{log}");
    assert!(
        log.contains("cap=navi.tool.roll_dice@1 generation=1"),
        "the tool call must reach the first generation:\n{log}"
    );
    assert!(
        log.contains("cap=navi.plugin.info@1 generation=2"),
        "the successor generation must serve calls too:\n{log}"
    );
}

#[test]
fn in_process_components_are_never_spawned_as_processes() {
    // The in-process sentinel (`inproc:<id>`) must not reach the process host,
    // otherwise NAVI would try to execute `inproc:navi.engine` as a binary.
    let tempdir = tempfile::tempdir().expect("tempdir");
    let compose = Compose::new(ComposeOptions {
        home: tempdir.path().join("compose"),
        project_dir: tempdir.path().join("project"),
        loaded_config: test_loaded_config(tempdir.path()),
        dry_run: false,
        terminal_effects: false,
        process_components: true,
    })
    .expect("compose");
    compose
        .install_definitions(&navi_compose::default_manifests())
        .expect("install");
    let engine = compose.activate("navi.engine").expect("engine");
    assert_eq!(engine.state, "Active");
    let host = compose.process_host().expect("process host");
    assert!(
        !host.child_running("navi.engine"),
        "in-process components must never spawn a child"
    );
    assert!(!host.child_running("navi.compaction.default"));
}
