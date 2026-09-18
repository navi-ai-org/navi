//! A **security policy and session hooks served by a third-party process**.
//!
//! The engine asks *that process* before every tool invocation and after every
//! session event; the host installs the proxies when the component activates and
//! pops them when it is withdrawn.
//!
//! Gated to Linux (process tier).

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use navi_compose::{
    ComponentKind, ComponentManifest, Compose, ComposeOptions, ComposeHooks, ComposeSecurity,
    Execution,
};
use navi_core::{
    DefaultToolSecurityPolicy, LoadedConfig, NaviConfig, NoopSessionHooks, SecurityDecision,
    SecurityPolicy, SessionHooks, ToolDefinition, ToolInvocation, ToolSecurityPolicy,
};
use serde_json::{Value, json};

const PLUGIN_ID: &str = "navi.plugin.security";

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
        description: "Third-party security policy and hooks (out of process)".to_string(),
        capabilities: vec![
            "navi.security@1".to_string(),
            "navi.hooks@1".to_string(),
            "navi.plugin.info@1".to_string(),
        ],
        requires: Vec::new(),
        subscriptions: Vec::new(),
        execution: Execution::process_with_args(
            env!("CARGO_BIN_EXE_navi-plugin-security"),
            vec![
                "--matrix-sock".to_string(),
                "{sock}".to_string(),
                "--id".to_string(),
                "{id}".to_string(),
                "--log".to_string(),
                log_path.to_string_lossy().to_string(),
                "--deny".to_string(),
                "web_fetch,run".to_string(),
            ],
        ),
        tier: "local".to_string(),
        trust: "trusted".to_string(),
        restart: "on-failure".to_string(),
        calls: Default::default(),
        kind: ComponentKind::Security,
        sandbox: None,
    }
}

/// Installs the manifest the way `navi components install` does, before the
/// session starts (the host policy and the kernel manifest come from disk).
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

fn invoke_until_ready(compose: &Compose) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = json!({"code": "not-started"});
    while Instant::now() < deadline {
        let (value, ok) = compose.kernel().invoke("navi.plugin.info@1", &json!({}));
        if ok {
            return value;
        }
        last = value;
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("plugin never became reachable; last response: {last}");
}

#[test]
fn a_plugin_process_can_be_the_engines_security_policy() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let log_path = tempdir.path().join("security.log");
    let home = tempdir.path().join("compose");
    install(&home, &plugin_manifest(&log_path));

    let compose = compose_for(&tempdir);
    let outcome = compose.activate(PLUGIN_ID).expect("activate");
    assert_eq!(outcome.state, "Active");

    let info = invoke_until_ready(&compose);
    assert_eq!(info["kind"], "security");
    assert_ne!(
        info["pid"].as_u64(),
        Some(std::process::id() as u64),
        "the policy must decide in its own process: {info}"
    );

    // The host installed the component into both slots it declares.
    assert_eq!(
        compose
            .slots()
            .security
            .current()
            .map(|(component, _, _)| component)
            .as_deref(),
        Some(PLUGIN_ID)
    );
    assert_eq!(
        compose
            .slots()
            .hooks
            .current()
            .map(|(component, _, _)| component)
            .as_deref(),
        Some(PLUGIN_ID)
    );

    // The engine's proxy now asks the plugin.
    let security: Arc<dyn ToolSecurityPolicy> = Arc::new(ComposeSecurity::new(
        compose.slots(),
        Arc::new(DefaultToolSecurityPolicy),
    ));
    let base = SecurityPolicy::new(
        tempdir.path().to_path_buf(),
        tempdir.path().join("data"),
        Default::default(),
    )
    .expect("base policy");
    let definition = ToolDefinition {
        name: "web_fetch".to_string(),
        ..Default::default()
    };
    let invocation = ToolInvocation {
        id: "call-1".to_string(),
        tool_name: "web_fetch".to_string(),
        input: json!({"url": "https://example.com"}),
    };
    match security.validate_tool(&base, &definition, &invocation) {
        SecurityDecision::Deny(reason) => assert!(
            reason.contains(PLUGIN_ID),
            "the denial names the plugin that made the decision: {reason}"
        ),
        other => panic!("the plugin must deny web_fetch, got {other:?}"),
    }

    // A tool the plugin does not deny falls back to the base policy.
    let allowed = ToolDefinition {
        name: "read_file".to_string(),
        ..Default::default()
    };
    assert_eq!(
        security.validate_tool(&base, &allowed, &invocation),
        base.validate_tool_invocation(&allowed, &invocation),
        "abstain means the base policy decides"
    );

    // Hooks: the same process records session events.
    let hooks: Arc<dyn SessionHooks> = Arc::new(ComposeHooks::new(
        compose.slots(),
        Arc::new(NoopSessionHooks),
    ));
    hooks.on_session_start("sess-1");
    hooks.on_tool_call(&invocation);

    // The plugin's own log proves the answers came from its process.
    let log = std::fs::read_to_string(&log_path).expect("plugin log");
    assert!(
        log.contains("tool=web_fetch deny"),
        "the plugin decided in its process:\n{log}"
    );
    assert!(
        log.contains("tool=read_file abstain"),
        "the plugin abstained for a tool it does not police:\n{log}"
    );
    assert!(
        log.contains("hook session_start"),
        "the plugin received the session event:\n{log}"
    );

    // Withdrawing pops both slots: the engine is back on its defaults.
    compose.withdraw(PLUGIN_ID).expect("withdraw");
    assert!(compose.slots().security.current().is_none());
    assert!(compose.slots().hooks.current().is_none());
    assert_eq!(
        security.validate_tool(&base, &definition, &invocation),
        base.validate_tool_invocation(&definition, &invocation),
        "without the component the base policy decides again"
    );
}
