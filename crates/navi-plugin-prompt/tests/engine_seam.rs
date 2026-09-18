//! The **system prompt rendered by a third-party process**.
//!
//! The engine's `PromptBuilder` proxy sends a projection of `SystemPromptInput`
//! to the plugin and uses the instructions/developer blocks it answers with.
//!
//! Gated to Linux (process tier).

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use navi_compose::{
    ComponentKind, ComponentManifest, Compose, ComposeOptions, ComposePrompt, Execution,
};
use navi_core::{
    DefaultPromptBuilder, LoadedConfig, NaviConfig, PromptBuilder, PromptCache, SystemPromptInput,
};
use serde_json::{Value, json};

const PLUGIN_ID: &str = "navi.plugin.prompt";

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
        description: "Third-party system-prompt builder (out of process)".to_string(),
        capabilities: vec!["navi.prompt@1".to_string(), "navi.plugin.info@1".to_string()],
        requires: Vec::new(),
        subscriptions: Vec::new(),
        execution: Execution::process_with_args(
            env!("CARGO_BIN_EXE_navi-plugin-prompt"),
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
        kind: ComponentKind::Prompt,
        sandbox: None,
    }
}

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

fn prompt_input(project: &Path, override_prompt: Option<&str>) -> SystemPromptInput {
    SystemPromptInput {
        config: NaviConfig::default(),
        project_dir: project.to_path_buf(),
        memory_injection: Some("the user prefers tabs".to_string()),
        tools: Vec::new(),
        include_tool_prompt_manifest: false,
        context_packets: Vec::new(),
        available_skills: Vec::new(),
        active_skills: Vec::new(),
        skill_pools: Vec::new(),
        harness_card: None,
        system_prompt: override_prompt.map(str::to_string),
    }
}

#[test]
fn a_plugin_process_can_render_the_system_prompt() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let log_path = tempdir.path().join("prompt.log");
    let home = tempdir.path().join("compose");
    install(&home, &plugin_manifest(&log_path));

    let compose = compose_for(&tempdir);
    let outcome = compose.activate(PLUGIN_ID).expect("activate");
    assert_eq!(outcome.state, "Active");

    let info = invoke_until_ready(&compose);
    assert_eq!(info["kind"], "prompt");
    assert_ne!(
        info["pid"].as_u64(),
        Some(std::process::id() as u64),
        "the prompt must be rendered in another process: {info}"
    );
    assert_eq!(
        compose
            .slots()
            .prompt
            .current()
            .map(|(component, _, _)| component)
            .as_deref(),
        Some(PLUGIN_ID)
    );

    // The engine's proxy now asks the plugin to render the prompt.
    let prompt: Arc<dyn PromptBuilder> = Arc::new(ComposePrompt::new(
        compose.slots(),
        Arc::new(DefaultPromptBuilder),
    ));
    let project = tempdir.path().join("project");
    let rendered = prompt.build(
        prompt_input(&project, None),
        Arc::new(PromptCache::new()),
    );
    assert!(
        rendered.instructions.contains(PLUGIN_ID),
        "the plugin's identity is in the instructions: {}",
        rendered.instructions
    );
    assert!(
        rendered
            .instructions
            .contains(&project.display().to_string()),
        "the plugin saw the project root: {}",
        rendered.instructions
    );
    assert!(
        rendered.instructions.contains("the user prefers tabs"),
        "the plugin used the injected memory: {}",
        rendered.instructions
    );
    assert!(
        !rendered.developer_messages.is_empty(),
        "the plugin emitted developer blocks: {:?}",
        rendered.developer_messages
    );

    // A host-supplied override wins over the component's own identity.
    let overridden = prompt.build(
        prompt_input(&project, Some("You are a pirate.")),
        Arc::new(PromptCache::new()),
    );
    assert!(
        overridden.instructions.starts_with("You are a pirate."),
        "the session override is honoured: {}",
        overridden.instructions
    );

    // The plugin's log proves the render happened in its process.
    let log = std::fs::read_to_string(&log_path).expect("plugin log");
    assert!(
        log.contains("build #1") && log.contains(&format!("project={}", project.display())),
        "the plugin rendered the prompt:\n{log}"
    );

    // Withdrawing pops the slot: the engine's own builder renders again.
    compose.withdraw(PLUGIN_ID).expect("withdraw");
    assert!(compose.slots().prompt.current().is_none());
    let fallback = prompt.build(
        prompt_input(&project, None),
        Arc::new(PromptCache::new()),
    );
    assert!(
        !fallback.instructions.contains(PLUGIN_ID),
        "without the component the default builder renders: {}",
        fallback.instructions
    );
}
