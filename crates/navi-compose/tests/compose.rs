//! Integration tests for the compose supervisor.
//!
//! These prove the spatiotemporal properties end to end:
//!
//! * dependencies decide activation (`Waiting` with a visible reason);
//! * a reload swaps the live generation (frontend quiesced, successor bound);
//! * stale generations are rejected at the effect boundary (I03);
//! * admission is revoked when a generation leaves `Active` (I02);
//! * the terminal is an exclusive resource released on swap (I01/I05/I07).

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use navi_compose::{
    Compose, ComposeOptions, ControlMsg, Execution, RequirementSpec, UiExit, UiFrontend, UiHost,
    default_manifests, ui_manifest,
};
use navi_core::{LoadedConfig, NaviConfig};
use navi_sdk::EngineDriver;
use navi_tui::host::TerminalLease;
use navi_tui::testing::MockEngine;
use serde_json::json;

/// What a scripted frontend observed during one run.
#[derive(Debug, Clone)]
struct RunRecord {
    generation: u64,
    session_id: Option<String>,
    command_results: Vec<String>,
    exit: UiExit,
}

/// Deterministic frontend: submits scripted commands through the kernel, then
/// waits for the supervisor to quiesce it (or quits immediately when
/// `quit_immediately` is set).
struct ScriptedFrontend {
    id: String,
    requires_tty: bool,
    commands: Vec<String>,
    quit_immediately: bool,
    runs: Arc<Mutex<Vec<RunRecord>>>,
}

impl UiFrontend for ScriptedFrontend {
    fn id(&self) -> &str {
        &self.id
    }

    fn requires_tty(&self) -> bool {
        self.requires_tty
    }

    fn run(&self, host: UiHost) -> UiExit {
        let mut command_results = Vec::new();
        for command in &self.commands {
            command_results.push(match host.commands.submit(command) {
                Ok(()) => "ok".to_string(),
                Err(err) => err,
            });
        }
        let exit = if self.quit_immediately {
            UiExit::Quit
        } else {
            loop {
                if let Some(message) = host.control.poll() {
                    match message {
                        ControlMsg::Quiesce { .. } => break UiExit::Quiesced,
                        ControlMsg::Shutdown => break UiExit::Quit,
                    }
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        self.runs.lock().unwrap().push(RunRecord {
            generation: host.generation,
            session_id: host.session_id.clone(),
            command_results,
            exit,
        });
        exit
    }
}

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

fn requirement(interface: &str, provider: &str) -> RequirementSpec {
    RequirementSpec::Detailed {
        interface: interface.to_string(),
        major: Some(1),
        provider: Some(provider.to_string()),
    }
}

/// Builds a compose host with the engine bound to a mock driver and the given
/// scripted frontends registered.
fn compose_with_frontends(
    frontends: Vec<(Arc<ScriptedFrontend>, bool)>,
) -> (Compose, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let compose = Compose::new(ComposeOptions {
        home: tempdir.path().join("compose"),
        project_dir: tempdir.path().join("project"),
        loaded_config: test_loaded_config(tempdir.path()),
        dry_run: false,
        terminal_effects: false,
        process_components: false,
    })
    .expect("compose");
    let driver: Arc<dyn EngineDriver> = Arc::new(MockEngine::with_config(test_loaded_config(
        tempdir.path(),
    )));
    compose.set_engine(driver);
    for (frontend, _) in &frontends {
        compose.register_ui(frontend.clone());
    }
    (compose, tempdir)
}

fn scripted(id: &str, requires_tty: bool, commands: Vec<String>) -> Arc<ScriptedFrontend> {
    Arc::new(ScriptedFrontend {
        id: id.to_string(),
        requires_tty,
        commands,
        quit_immediately: false,
        runs: Arc::new(Mutex::new(Vec::new())),
    })
}

/// A frontend that quits as soon as it starts (user quit / shutdown request).
fn scripted_quitter(id: &str) -> Arc<ScriptedFrontend> {
    Arc::new(ScriptedFrontend {
        id: id.to_string(),
        requires_tty: false,
        commands: Vec::new(),
        quit_immediately: true,
        runs: Arc::new(Mutex::new(Vec::new())),
    })
}

fn manifest_for(frontend: &ScriptedFrontend) -> navi_compose::ComponentManifest {
    let command_capability = navi_compose::command_capability(&frontend.id);
    ui_manifest(
        &frontend.id,
        "scripted test frontend",
        &command_capability,
        vec![requirement("navi.engine.driver", "navi.engine")],
    )
}

#[test]
fn missing_provider_leaves_component_waiting_with_reason() {
    let (compose, _dir) = compose_with_frontends(Vec::new());
    let frontend = scripted("navi.ui.test", false, Vec::new());
    compose
        .install_definition(manifest_for(&frontend))
        .expect("install");

    let outcome = compose.activate("navi.ui.test").expect("activate");
    assert_eq!(
        outcome.state, "Waiting",
        "a frontend without its engine provider must not activate"
    );
    let reason = outcome.waiting_reason.expect("waiting reason");
    assert!(
        reason.contains("navi.engine") || reason.contains("dependency") || reason.contains("provider"),
        "reason should name the missing dependency, got: {reason}"
    );
    assert!(!compose.has_running_frontend());
}

#[test]
fn activation_publishes_dependencies_and_starts_the_frontend() {
    let frontend = scripted("navi.ui.test", false, Vec::new());
    let (compose, _dir) = compose_with_frontends(vec![(frontend.clone(), true)]);
    compose
        .install_definitions(&[navi_compose::engine_manifest(), manifest_for(&frontend)])
        .expect("install");

    let engine = compose.activate("navi.engine").expect("engine");
    assert_eq!(engine.state, "Active");

    let ui = compose.activate("navi.ui.test").expect("ui");
    assert_eq!(ui.state, "Active");
    assert_eq!(ui.generation, Some(1));
    assert!(compose.has_running_frontend());
    assert_eq!(compose.running_frontend().as_deref(), Some("navi.ui.test"));
}

#[test]
fn commands_are_admitted_while_active_and_denied_after_withdraw() {
    let frontend = scripted("navi.ui.test", false, Vec::new());
    let (compose, _dir) = compose_with_frontends(vec![(frontend.clone(), true)]);
    compose
        .install_definitions(&[navi_compose::engine_manifest(), manifest_for(&frontend)])
        .expect("install");
    compose.activate("navi.engine").expect("engine");
    compose.activate("navi.ui.test").expect("ui");

    let capability = navi_compose::command_capability("navi.ui.test");
    let (value, ok) = compose
        .kernel()
        .invoke(&capability, &json!({"text": "hello engine"}));
    assert!(ok, "admitted while active: {value}");
    let queued = compose.take_commands();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].text, "hello engine");
    assert_eq!(queued[0].component, "navi.ui.test");

    compose.withdraw("navi.ui.test").expect("withdraw");
    let (value, ok) = compose
        .kernel()
        .invoke(&capability, &json!({"text": "after withdraw"}));
    assert!(!ok, "a withdrawn generation must not be admitted");
    let code = value
        .get("code")
        .and_then(|code| code.as_str())
        .unwrap_or_default();
    assert!(
        matches!(
            code,
            "context-not-active" | "plugin-not-active" | "plugin-not-loaded" | "no-such-capability"
        ),
        "expected an admission denial, got: {value}"
    );
    assert!(compose.take_commands().is_empty());
}

#[test]
fn runtime_reload_swaps_the_frontend_generation() {
    let frontend = scripted("navi.ui.test", false, Vec::new());
    let (compose, _dir) = compose_with_frontends(vec![(frontend.clone(), true)]);
    compose
        .install_definitions(&[navi_compose::engine_manifest(), manifest_for(&frontend)])
        .expect("install");
    compose.activate("navi.engine").expect("engine");
    compose.activate("navi.ui.test").expect("ui");
    let first_generation = compose.generation_of("navi.ui.test").expect("generation");
    let first_instance = compose
        .instance_ref_of("navi.ui.test")
        .expect("instance ref")
        .instance;

    // Wait until the frontend thread is actually running.
    std::thread::sleep(Duration::from_millis(50));

    let outcome = compose.reload("navi.ui.test").expect("reload");
    assert_eq!(outcome.previous_generation, Some(first_generation));
    assert_eq!(outcome.generation, Some(first_generation + 1));
    assert_eq!(outcome.state, "Active");
    let second_instance = compose
        .instance_ref_of("navi.ui.test")
        .expect("instance ref")
        .instance;
    assert_ne!(first_instance, second_instance, "new activation, new identity");

    // The previous generation was quiesced (not killed) and recorded its run.
    let runs = frontend.runs.lock().unwrap().clone();
    assert_eq!(runs.len(), 1, "exactly one generation stopped so far");
    assert_eq!(runs[0].generation, first_generation);
    assert_eq!(runs[0].exit, UiExit::Quiesced);

    // Withdrawing the successor stops it too.
    compose.withdraw("navi.ui.test").expect("withdraw");
    let runs = frontend.runs.lock().unwrap().clone();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[1].generation, first_generation + 1);
    assert_eq!(runs[1].exit, UiExit::Quiesced);
    assert!(!compose.has_running_frontend());
}

#[test]
fn stale_generation_is_rejected_at_the_effect_boundary() {
    let (compose, _dir) = compose_with_frontends(Vec::new());
    compose
        .install_definitions(&[navi_compose::engine_manifest()])
        .expect("install");
    compose.activate("navi.engine").expect("engine");

    let old_ref = compose.instance_ref_of("navi.engine").expect("instance ref");
    let (value, ok) = compose
        .kernel()
        .invoke_for_ref(&old_ref, "navi.engine.driver@1", &json!({}));
    assert!(ok, "the live reference must be admitted: {value}");

    let outcome = compose.reload("navi.engine").expect("reload");
    assert!(outcome.generation.expect("generation") > old_ref.generation);

    let (value, ok) = compose
        .kernel()
        .invoke_for_ref(&old_ref, "navi.engine.driver@1", &json!({}));
    assert!(!ok, "a stale reference must never be revalidated");
    assert_eq!(value["code"], "stale-generation");
}

#[test]
fn terminal_lease_is_exclusive_and_released_on_swap() {
    let frontend = scripted("navi.ui.tty", true, Vec::new());
    let (compose, _dir) = compose_with_frontends(vec![(frontend.clone(), true)]);
    compose
        .install_definitions(&[navi_compose::engine_manifest(), manifest_for(&frontend)])
        .expect("install");
    compose.activate("navi.engine").expect("engine");
    compose.activate("navi.ui.tty").expect("ui");
    std::thread::sleep(Duration::from_millis(50));

    assert!(
        TerminalLease::acquire_detached("intruder").is_err(),
        "the terminal must have exactly one owner while a TTY frontend is active"
    );

    compose.reload("navi.ui.tty").expect("reload");
    assert!(
        TerminalLease::acquire_detached("intruder").is_err(),
        "the successor generation must hold the terminal after the swap"
    );

    compose.withdraw("navi.ui.tty").expect("withdraw");
    let lease = TerminalLease::acquire_detached("intruder")
        .expect("the terminal must be free after the generation is withdrawn");
    assert_eq!(lease.owner(), "intruder");
    lease.release().expect("release");
    assert!(!lease.is_held());
}

#[test]
fn failed_frontend_start_rolls_back_the_activation() {
    // Hold the terminal so the TTY frontend cannot acquire it: the activation
    // must fail closed and clean up instead of leaving a half-active component.
    let blocker = TerminalLease::acquire_detached("test-blocker").expect("blocker lease");
    let frontend = scripted("navi.ui.tty", true, Vec::new());
    let (compose, _dir) = compose_with_frontends(vec![(frontend.clone(), true)]);
    compose
        .install_definitions(&[navi_compose::engine_manifest(), manifest_for(&frontend)])
        .expect("install");
    compose.activate("navi.engine").expect("engine");
    let outcome = compose.activate("navi.ui.tty").expect("activate");

    assert!(
        outcome.state != "Active",
        "activation must fail closed when the terminal is taken: {outcome:?}"
    );
    assert!(!compose.has_running_frontend());
    assert!(
        compose
            .last_error()
            .unwrap_or_default()
            .contains("terminal lease"),
        "the failure reason must be visible: {:?}",
        compose.last_error()
    );
    // No resources left behind by the failed generation.
    assert!(compose.kernel().pending_calls().is_empty());
    blocker.release().expect("release blocker");
}

#[test]
fn default_manifests_install_without_activating() {
    let (compose, _dir) = compose_with_frontends(Vec::new());
    compose
        .install_definitions(&default_manifests())
        .expect("install");

    // Definitions are on disk; nothing is active until the supervisor activates it.
    let statuses = compose.status();
    let tui = statuses
        .iter()
        .find(|status| status.id == "navi.ui.tui")
        .expect("tui status");
    assert_eq!(tui.state, "NotLoaded");
    assert!(!tui.ui_running);

    let engine = compose.activate("navi.engine").expect("engine");
    assert_eq!(engine.state, "Active");

    let compaction = compose.activate("navi.compaction.default").expect("compaction");
    assert_eq!(compaction.state, "Active");
    let (value, ok) = compose.kernel().invoke("navi.compaction@1", &json!({}));
    assert!(ok, "compaction handler reachable: {value}");
    assert_eq!(value["strategy"], "default");

    // A non-UI component swap is observable through its generation.
    let first = compaction.generation.expect("generation");
    let swapped = compose.reload("navi.compaction.default").expect("reload");
    assert_eq!(swapped.generation, Some(first + 1));
    let (value, ok) = compose.kernel().invoke("navi.compaction@1", &json!({}));
    assert!(ok);
    assert_eq!(value["generation"], first + 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_line_frontend_starts_and_quiesces_through_compose() {
    // The *real* `navi.ui.plain` frontend (not a test double) activated as a
    // component: it starts on a supervisor thread, then releases on withdraw.
    let (compose, _dir) = compose_with_frontends(Vec::new());
    compose
        .install_definitions(&default_manifests())
        .expect("install");
    compose.activate("navi.engine").expect("engine");

    let outcome = compose.activate("navi.ui.plain").expect("plain ui");
    assert_eq!(outcome.state, "Active");
    std::thread::sleep(Duration::from_millis(80));
    assert!(compose.has_running_frontend());
    assert_eq!(compose.running_frontend().as_deref(), Some("navi.ui.plain"));

    let statuses = compose.status();
    let plain = statuses
        .iter()
        .find(|status| status.id == "navi.ui.plain")
        .expect("plain status");
    assert!(plain.ui_running);
    assert!(!plain.tty, "the line frontend does not own the terminal");

    compose.withdraw("navi.ui.plain").expect("withdraw");
    assert!(!compose.has_running_frontend());
}

#[test]
fn reload_request_file_is_consumed_by_the_supervisor() {
    let (compose, _dir) = compose_with_frontends(Vec::new());
    compose
        .install_definitions(&default_manifests())
        .expect("install");
    compose.activate("navi.engine").expect("engine");

    compose.request_reload("navi.compaction.default").expect("request");
    let requests = compose.take_reload_requests();
    assert_eq!(requests, vec!["navi.compaction.default".to_string()]);
    assert!(
        compose.take_reload_requests().is_empty(),
        "requests are consumed once"
    );
}

#[test]
fn session_binding_is_shared_by_every_generation() {
    let frontend = scripted("navi.ui.test", false, vec!["hello".to_string()]);
    let (compose, _dir) = compose_with_frontends(vec![(frontend.clone(), true)]);
    compose
        .install_definitions(&[navi_compose::engine_manifest(), manifest_for(&frontend)])
        .expect("install");
    compose.activate("navi.engine").expect("engine");
    compose.set_session("session-swap-1");
    compose.activate("navi.ui.test").expect("ui");
    std::thread::sleep(Duration::from_millis(50));
    compose.reload("navi.ui.test").expect("reload");
    // Let the successor generation actually run before withdrawing it, so both
    // generations get to submit their command while active.
    std::thread::sleep(Duration::from_millis(50));
    compose.withdraw("navi.ui.test").expect("withdraw");

    // Admission is revoked once the generation is gone: a late command from the
    // retired frontend never reaches the engine.
    let capability = navi_compose::command_capability("navi.ui.test");
    let (value, ok) = compose.kernel().invoke(&capability, &json!({"text": "late"}));
    assert!(!ok, "late command must be rejected: {value}");
    let queued = compose.take_commands();
    assert_eq!(
        queued.len(),
        2,
        "only the two admitted commands are queued, not the late one"
    );
    assert!(queued.iter().all(|command| command.text == "hello"));

    let runs = frontend.runs.lock().unwrap().clone();
    assert_eq!(runs.len(), 2, "one run per generation");
    for run in &runs {
        assert_eq!(
            run.session_id.as_deref(),
            Some("session-swap-1"),
            "every generation attaches to the same live session (gen {})",
            run.generation
        );
        assert_eq!(
            run.command_results,
            vec!["ok".to_string()],
            "commands are admitted through the kernel in every generation (gen {})",
            run.generation
        );
        assert_eq!(run.exit, UiExit::Quiesced);
    }
    assert_ne!(runs[0].generation, runs[1].generation);
}

#[test]
fn uninstalling_a_provider_cascades_consumers_to_waiting() {
    let frontend = scripted("navi.ui.test", false, Vec::new());
    let (compose, _dir) = compose_with_frontends(vec![(frontend.clone(), true)]);
    compose
        .install_definitions(&[navi_compose::engine_manifest(), manifest_for(&frontend)])
        .expect("install");
    compose.activate("navi.engine").expect("engine");
    assert_eq!(compose.activate("navi.ui.test").expect("ui").state, "Active");
    std::thread::sleep(Duration::from_millis(50));
    assert!(compose.has_running_frontend());

    // Remove the provider entirely: the consumer keeps its definition, is
    // withdrawn, and waits with a visible reason.
    compose.uninstall("navi.engine").expect("uninstall");
    assert!(
        compose.manifests().iter().all(|m| m.id != "navi.engine"),
        "the installed artifact is gone"
    );
    assert!(
        compose.activate("navi.engine").is_err(),
        "an uninstalled component cannot be activated again"
    );
    assert_eq!(
        compose.state_of("navi.ui.test").as_deref(),
        Some("Waiting"),
        "the consumer must be withdrawn into Waiting"
    );
    assert!(!compose.has_running_frontend());
    let reason = compose
        .kernel()
        .waiting_reason_of("navi.ui.test")
        .expect("waiting reason");
    assert!(!reason.is_empty());
}

#[test]
fn activating_another_frontend_withdraws_the_previous_one() {
    let first = scripted("navi.ui.test", false, Vec::new());
    let second = scripted("navi.ui.test2", false, Vec::new());
    let (compose, _dir) = compose_with_frontends(vec![(first.clone(), true), (second.clone(), true)]);
    compose
        .install_definitions(&[
            navi_compose::engine_manifest(),
            manifest_for(&first),
            manifest_for(&second),
        ])
        .expect("install");
    compose.activate("navi.engine").expect("engine");
    compose.activate("navi.ui.test").expect("first ui");
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(compose.running_frontend().as_deref(), Some("navi.ui.test"));

    // The frontend role is exclusive: activating another frontend swaps them.
    let outcome = compose.activate("navi.ui.test2").expect("second ui");
    assert_eq!(outcome.state, "Active");
    assert_eq!(compose.running_frontend().as_deref(), Some("navi.ui.test2"));
    assert_eq!(
        compose.state_of("navi.ui.test").as_deref(),
        Some("Disposed"),
        "the previous frontend must be withdrawn"
    );
    let runs = first.runs.lock().unwrap().clone();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].exit, UiExit::Quiesced);
    assert_eq!(compose.status().iter().filter(|s| s.ui_running).count(), 1);
}

#[test]
fn a_frontend_that_quits_on_its_own_is_reaped() {
    let frontend = scripted_quitter("navi.ui.test");
    let (compose, _dir) = compose_with_frontends(vec![(frontend.clone(), true)]);
    compose
        .install_definitions(&[navi_compose::engine_manifest(), manifest_for(&frontend)])
        .expect("install");
    compose.activate("navi.engine").expect("engine");
    compose.activate("navi.ui.test").expect("ui");
    std::thread::sleep(Duration::from_millis(50));

    // The supervisor must notice the dead generation and drop it, otherwise it
    // would never exit once the last frontend quits.
    let reaped = compose.reap_exited_frontends();
    assert_eq!(reaped, vec!["navi.ui.test".to_string()]);
    assert!(!compose.has_running_frontend());
    assert!(compose.reap_exited_frontends().is_empty(), "reaping is idempotent");
}

#[test]
fn execution_kind_marks_host_execution() {
    let manifest = navi_compose::engine_manifest();
    assert!(matches!(manifest.execution, Execution::Detailed { .. }));
    assert!(manifest.execution.is_inproc());
    let value = manifest.to_kernel_json();
    assert_eq!(value["execution"]["entrypoint"], "inproc:navi.engine");
}
