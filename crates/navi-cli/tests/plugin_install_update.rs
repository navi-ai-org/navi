//! Integration tests for the `navi plugin install` and `navi plugin update` CLI commands.
//!
//! These tests exercise the binary end-to-end through a mock plugin directory.

use navi_plugin_manifest::{
    PluginManifest, PluginMeta, RuntimeKind, ToolDef, ToolRisk, sign_plugin_manifest_for_tests,
};
use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn write_test_plugin(dir: &std::path::Path, id: &str, version: &str, publisher: &str, wasm: &[u8]) {
    fs::create_dir_all(dir).unwrap();
    let mut manifest = PluginManifest {
        plugin: PluginMeta {
            id: id.to_string(),
            name: id.to_string(),
            version: version.to_string(),
            publisher: publisher.to_string(),
            runtime: RuntimeKind::WasmComponent,
            entry: "plugin.wasm".to_string(),
            wasm_hash: String::new(),
            signature: String::new(),
            public_key: None,
            minimum_navi: "0.1.0".to_string(),
        },
        capabilities: vec![],
        tools: vec![ToolDef {
            id: "tool".to_string(),
            summary: "test tool".to_string(),
            risk: ToolRisk::ReadOnly,
            input_schema: None,
            capabilities: vec![],
        }],
    };
    sign_plugin_manifest_for_tests(&mut manifest, wasm);
    fs::write(dir.join("plugin.toml"), toml::to_string(&manifest).unwrap()).unwrap();
    fs::write(dir.join("plugin.wasm"), wasm).unwrap();
}

/// Build a `navi` command with isolated env so it doesn't pollute user state.
fn navi_cmd_for(data_dir: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_navi"));
    cmd.env_remove("OPENAI_API_KEY");
    cmd.env_remove("ANTHROPIC_API_KEY");
    cmd.env("XDG_DATA_HOME", data_dir);
    cmd.env("XDG_CONFIG_HOME", data_dir);
    cmd.env("NAVI_NO_REGISTRY_UPDATE", "1");
    cmd
}

/// NAVI's resolved data dir on Linux under XDG_DATA_HOME.
fn navi_data_dir(xdg_root: &std::path::Path) -> std::path::PathBuf {
    xdg_root.join("navi")
}

#[test]
fn plugin_install_yes_flag_installs_to_data_dir() {
    let project = TempDir::new().unwrap();
    let xdg = TempDir::new().unwrap();
    let src = TempDir::new().unwrap();
    write_test_plugin(
        src.path(),
        "test-plugin",
        "1.0.0",
        "gh:test",
        b"fake-wasm-1",
    );

    let output = navi_cmd_for(xdg.path())
        .current_dir(project.path())
        .args(["plugin", "install", src.path().to_str().unwrap(), "--yes"])
        .output()
        .expect("run navi plugin install");

    assert!(
        output.status.success(),
        "install failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let plugins_dir = navi_data_dir(xdg.path()).join("plugins");
    assert!(plugins_dir.join("test-plugin").join("plugin.toml").exists());
    assert!(plugins_dir.join("test-plugin").join("plugin.wasm").exists());
    assert!(plugins_dir.join("navi-plugins.lock").exists());
    assert!(
        !plugins_dir
            .join("test-plugin")
            .join("navi-plugins.lock")
            .exists(),
        "lockfile must be aggregate at plugins root, not per-plugin"
    );
}

#[test]
fn plugin_update_changes_version() {
    let project = TempDir::new().unwrap();
    let xdg = TempDir::new().unwrap();
    let src_v1 = TempDir::new().unwrap();
    let src_v2 = TempDir::new().unwrap();
    write_test_plugin(src_v1.path(), "upd", "1.0.0", "gh:test", b"fake-wasm-1");
    write_test_plugin(src_v2.path(), "upd", "1.1.0", "gh:test", b"fake-wasm-2");

    let install = navi_cmd_for(xdg.path())
        .current_dir(project.path())
        .args([
            "plugin",
            "install",
            src_v1.path().to_str().unwrap(),
            "--yes",
        ])
        .output()
        .expect("install v1");
    assert!(
        install.status.success(),
        "install v1 failed: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let mut update = navi_cmd_for(xdg.path())
        .current_dir(project.path())
        .args(["plugin", "update", src_v2.path().to_str().unwrap()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn update");
    use std::io::Write;
    update
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"y\n")
        .expect("write stdin");
    let out = update.wait_with_output().expect("wait update");
    assert!(
        out.status.success(),
        "update failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let lockfile_path = navi_data_dir(xdg.path())
        .join("plugins")
        .join("navi-plugins.lock");
    let lockfile_contents = fs::read_to_string(&lockfile_path).unwrap();
    assert!(
        lockfile_contents.contains("1.1.0"),
        "lockfile should reflect new version: {lockfile_contents}"
    );
}

#[test]
fn plugin_update_blocked_on_publisher_change() {
    let project = TempDir::new().unwrap();
    let xdg = TempDir::new().unwrap();
    let src_v1 = TempDir::new().unwrap();
    let src_v2 = TempDir::new().unwrap();
    write_test_plugin(
        src_v1.path(),
        "pub-change",
        "1.0.0",
        "gh:original",
        b"fake-wasm-1",
    );
    write_test_plugin(
        src_v2.path(),
        "pub-change",
        "1.0.1",
        "gh:attacker",
        b"fake-wasm-2",
    );

    let install = navi_cmd_for(xdg.path())
        .current_dir(project.path())
        .args([
            "plugin",
            "install",
            src_v1.path().to_str().unwrap(),
            "--yes",
        ])
        .output()
        .expect("install v1");
    assert!(
        install.status.success(),
        "install v1 failed: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let update = navi_cmd_for(xdg.path())
        .current_dir(project.path())
        .args(["plugin", "update", src_v2.path().to_str().unwrap()])
        .output()
        .expect("update");
    assert!(!update.status.success(), "update should be blocked");
    let stderr = String::from_utf8_lossy(&update.stderr);
    assert!(
        stderr.contains("publisher change") || stderr.contains("blocked"),
        "expected publisher-blocked error, got: {stderr}"
    );
}

#[test]
fn plugin_update_force_overrides_publisher_block() {
    let project = TempDir::new().unwrap();
    let xdg = TempDir::new().unwrap();
    let src_v1 = TempDir::new().unwrap();
    let src_v2 = TempDir::new().unwrap();
    write_test_plugin(
        src_v1.path(),
        "forced",
        "1.0.0",
        "gh:original",
        b"fake-wasm-1",
    );
    write_test_plugin(src_v2.path(), "forced", "1.0.1", "gh:new", b"fake-wasm-2");

    let install = navi_cmd_for(xdg.path())
        .current_dir(project.path())
        .args([
            "plugin",
            "install",
            src_v1.path().to_str().unwrap(),
            "--yes",
        ])
        .output()
        .expect("install v1");
    assert!(
        install.status.success(),
        "install v1 failed: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let update = navi_cmd_for(xdg.path())
        .current_dir(project.path())
        .args([
            "plugin",
            "update",
            src_v2.path().to_str().unwrap(),
            "--force",
        ])
        .output()
        .expect("update --force");
    assert!(
        update.status.success(),
        "force update should succeed: {}",
        String::from_utf8_lossy(&update.stderr)
    );
}
