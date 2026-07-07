use super::*;
use serde_json::json;
use std::path::Path;

// ── find_npm_package ─────────────────────────────────────────────────

#[test]
fn find_npm_package_in_dependencies() {
    let manifest = r#"{"dependencies": {"express": "4"}}"#;
    assert_eq!(
        finders::find_npm_package(manifest, "express"),
        Some("dependencies".to_string())
    );
}

#[test]
fn find_npm_package_in_dev_dependencies() {
    let manifest = r#"{"devDependencies": {"vitest": "1"}}"#;
    assert_eq!(
        finders::find_npm_package(manifest, "vitest"),
        Some("devDependencies".to_string())
    );
}

#[test]
fn find_npm_package_in_peer_dependencies() {
    let manifest = r#"{"peerDependencies": {"react": "18"}}"#;
    assert_eq!(
        finders::find_npm_package(manifest, "react"),
        Some("peerDependencies".to_string())
    );
}

#[test]
fn find_npm_package_in_optional_dependencies() {
    let manifest = r#"{"optionalDependencies": {"fsevents": "2"}}"#;
    assert_eq!(
        finders::find_npm_package(manifest, "fsevents"),
        Some("optionalDependencies".to_string())
    );
}

#[test]
fn find_npm_package_returns_none_for_missing() {
    let manifest = r#"{"dependencies": {"express": "4"}}"#;
    assert_eq!(finders::find_npm_package(manifest, "missing"), None);
}

#[test]
fn find_npm_package_returns_none_for_invalid_json() {
    assert_eq!(finders::find_npm_package("not json", "pkg"), None);
}

// ── find_cargo_package ───────────────────────────────────────────────

#[test]
fn find_cargo_package_in_dependencies() {
    let manifest = "[dependencies]\nserde = \"1\"\n";
    assert_eq!(
        finders::find_cargo_package(manifest, "serde"),
        Some("dependencies".to_string())
    );
}

#[test]
fn find_cargo_package_in_dev_dependencies() {
    let manifest = "[dev-dependencies]\npretty_assertions = \"1\"\n";
    assert_eq!(
        finders::find_cargo_package(manifest, "pretty_assertions"),
        Some("dev-dependencies".to_string())
    );
}

#[test]
fn find_cargo_package_in_build_dependencies() {
    let manifest = "[build-dependencies]\ncc = \"1\"\n";
    assert_eq!(
        finders::find_cargo_package(manifest, "cc"),
        Some("build-dependencies".to_string())
    );
}

#[test]
fn find_cargo_package_in_workspace_dependencies() {
    let manifest = "[workspace.dependencies]\nserde = \"1\"\n";
    assert_eq!(
        finders::find_cargo_package(manifest, "serde"),
        Some("workspace.dependencies".to_string())
    );
}

#[test]
fn find_cargo_package_returns_none_for_missing() {
    let manifest = "[dependencies]\nserde = \"1\"\n";
    assert_eq!(finders::find_cargo_package(manifest, "missing"), None);
}

#[test]
fn find_cargo_package_returns_none_for_invalid_toml() {
    assert_eq!(finders::find_cargo_package("not toml [[[", "pkg"), None);
}

// ── find_dart_package ────────────────────────────────────────────────

#[test]
fn find_dart_package_in_dependencies() {
    let manifest = "dependencies:\n  http: ^1.0.0\n";
    assert!(finders::find_dart_package(manifest, "http"));
}

#[test]
fn find_dart_package_in_dev_dependencies() {
    let manifest = "dev_dependencies:\n  test: ^1.24.0\n";
    assert!(finders::find_dart_package(manifest, "test"));
}

#[test]
fn find_dart_package_returns_false_for_missing() {
    let manifest = "dependencies:\n  http: ^1.0.0\n";
    assert!(!finders::find_dart_package(manifest, "missing"));
}

#[test]
fn find_dart_package_returns_false_for_invalid_yaml() {
    assert!(!finders::find_dart_package("not yaml [[[", "pkg"));
}

#[test]
fn find_go_package_in_require_block() {
    let manifest = "module example\n\nrequire (\n\tgithub.com/acme/pkg v1.2.3\n)\n";
    assert!(finders::find_go_package(manifest, "github.com/acme/pkg"));
}

#[test]
fn find_go_package_inline_require() {
    let manifest = "module example\n\nrequire github.com/acme/pkg v1.2.3\n";
    assert!(finders::find_go_package(manifest, "github.com/acme/pkg"));
}

#[test]
fn find_go_package_returns_false_for_missing() {
    let manifest = "module example\n\nrequire (\n\tgithub.com/acme/pkg v1.2.3\n)\n";
    assert!(!finders::find_go_package(
        manifest,
        "github.com/missing/pkg"
    ));
}

#[test]
fn find_go_package_returns_false_for_empty_manifest() {
    assert!(!finders::find_go_package("", "pkg"));
}

// ── detect_package_manager ─────────────────────────────────────────────

#[tokio::test]
async fn detect_bun_from_lockfile() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("bun.lockb"), "").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    assert_eq!(detect_package_manager(tempdir.path()).await.unwrap(), "bun");
}
#[tokio::test]
async fn detect_dart_from_pubspec() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("pubspec.yaml"), "name: test").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    assert_eq!(
        detect_package_manager(tempdir.path()).await.unwrap(),
        "dart"
    );
}

#[tokio::test]
async fn detect_npm_from_package_json() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("package.json"), "{}").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    assert_eq!(detect_package_manager(tempdir.path()).await.unwrap(), "npm");
}

#[tokio::test]
async fn detect_npm_from_lockfile() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("package-lock.json"), "{}").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    assert_eq!(detect_package_manager(tempdir.path()).await.unwrap(), "npm");
}

#[tokio::test]
async fn detect_cargo_from_toml() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("Cargo.toml"), "[package]").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    assert_eq!(
        detect_package_manager(tempdir.path()).await.unwrap(),
        "cargo"
    );
}

#[tokio::test]
async fn detect_cargo_from_lockfile() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("Cargo.lock"), "").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    assert_eq!(
        detect_package_manager(tempdir.path()).await.unwrap(),
        "cargo"
    );
}

#[tokio::test]
async fn detect_go_from_mod() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("go.mod"), "module test").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    assert_eq!(detect_package_manager(tempdir.path()).await.unwrap(), "go");
}

#[tokio::test]
async fn detect_go_from_sum() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("go.sum"), "").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    assert_eq!(detect_package_manager(tempdir.path()).await.unwrap(), "go");
}

#[tokio::test]
async fn detect_fails_without_manifests() {
    let tempdir = tempfile::tempdir().unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    assert!(detect_package_manager(tempdir.path()).await.is_err());
}

#[tokio::test]
async fn detect_prefers_bun_over_npm() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("bun.lockb"), "").unwrap();
    std::fs::write(tempdir.path().join("package.json"), "{}").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    assert_eq!(detect_package_manager(tempdir.path()).await.unwrap(), "bun");
}

// ── check_cargo inline ─────────────────────────────────────────────────

#[tokio::test]
async fn check_cargo_finds_packages_in_manifest() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(
        tempdir.path().join("Cargo.toml"),
        "[dependencies]\nserde = \"1\"\ntokio = \"1\"\n",
    )
    .unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());

    let result = check::check_cargo(
        tempdir.path(),
        "test",
        &["serde".to_string(), "missing".to_string()],
    )
    .await
    .unwrap();

    assert!(result.ok);
    let installed = result.output["installed"].as_array().unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0]["name"], "serde");
    assert_eq!(installed[0]["section"], "dependencies");
    let not_found = result.output["not_found"].as_array().unwrap();
    assert_eq!(not_found.len(), 1);
    assert_eq!(not_found[0], "missing");
}

#[tokio::test]
async fn check_cargo_empty_packages_checks_lock() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("Cargo.lock"), "").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());

    let result = check::check_cargo(tempdir.path(), "test", &[])
        .await
        .unwrap();
    assert!(result.ok);
    assert_eq!(result.output["status"], "installed");
}

// ── check_npm inline ───────────────────────────────────────────────────

#[tokio::test]
async fn check_npm_finds_packages_in_manifest() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(
        tempdir.path().join("package.json"),
        r#"{"dependencies": {"express": "4", "lodash": "4"}}"#,
    )
    .unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());

    let result = check::check_npm(tempdir.path(), "test", "npm", &["express".to_string()])
        .await
        .unwrap();

    assert!(result.ok);
    let installed = result.output["installed"].as_array().unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0]["name"], "express");
    assert_eq!(installed[0]["section"], "dependencies");
}

#[tokio::test]
async fn check_npm_avoids_substring_false_positive() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(
        tempdir.path().join("package.json"),
        r#"{"dependencies": {"serde_json": "1"}}"#,
    )
    .unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());

    let result = check::check_npm(tempdir.path(), "test", "npm", &["serde".to_string()])
        .await
        .unwrap();

    assert!(result.ok);
    assert_eq!(result.output["status"], "not_found");
    assert_eq!(result.output["not_found"][0], "serde");
}

#[tokio::test]
async fn check_cargo_avoids_substring_false_positive() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(
        tempdir.path().join("Cargo.toml"),
        "[dependencies]\nserde_json = \"1\"\n",
    )
    .unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());

    let result = check::check_cargo(tempdir.path(), "test", &["serde".to_string()])
        .await
        .unwrap();

    assert!(result.ok);
    assert_eq!(result.output["status"], "not_found");
    assert_eq!(result.output["not_found"][0], "serde");
}

#[tokio::test]
async fn check_go_parses_require_block() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(
        tempdir.path().join("go.mod"),
        "module example\n\nrequire (\n\tgithub.com/acme/pkg v1.2.3\n)\n",
    )
    .unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());

    let result = check::check_go(tempdir.path(), "test", &["github.com/acme/pkg".to_string()])
        .await
        .unwrap();

    assert!(result.ok);
    assert_eq!(result.output["status"], "success");
    assert_eq!(result.output["installed"][0]["section"], "require");
}
#[tokio::test]
async fn check_dart_finds_packages_in_manifest() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(
        tempdir.path().join("pubspec.yaml"),
        "dependencies:\n  http: ^1.0.0\n  path: ^1.8.0\n",
    )
    .unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());

    let result = check::check_dart(
        tempdir.path(),
        "test",
        &["http".to_string(), "missing".to_string()],
    )
    .await
    .unwrap();

    assert!(result.ok);
    let installed = result.output["installed"].as_array().unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0]["name"], "http");
    assert_eq!(installed[0]["section"], "dependencies");
    let not_found = result.output["not_found"].as_array().unwrap();
    assert_eq!(not_found.len(), 1);
    assert_eq!(not_found[0], "missing");
}

#[tokio::test]
async fn check_dart_finds_packages_in_dev_dependencies() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(
        tempdir.path().join("pubspec.yaml"),
        "dev_dependencies:\n  test: ^1.24.0\n",
    )
    .unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());

    let result = check::check_dart(tempdir.path(), "test", &["test".to_string()])
        .await
        .unwrap();

    assert!(result.ok);
    assert_eq!(result.output["installed"][0]["name"], "test");
}

#[tokio::test]
async fn check_dart_empty_packages_checks_lock() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("pubspec.lock"), "").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());

    let result = check::check_dart(tempdir.path(), "test", &[])
        .await
        .unwrap();
    assert!(result.ok);
    assert_eq!(result.output["status"], "installed");
}

#[tokio::test]
async fn check_npm_empty_packages_checks_node_modules() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::create_dir(tempdir.path().join("node_modules")).unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());

    let result = check::check_npm(tempdir.path(), "test", "npm", &[])
        .await
        .unwrap();
    assert!(result.ok);
    assert_eq!(result.output["status"], "installed");
}

// ── invoke dispatch ──────────────────────────────────────────────────

fn pm_executor(root: &Path) -> crate::tool::ToolExecutor {
    let policy = crate::SecurityPolicy::new(
        root.to_path_buf(),
        root.parent()
            .unwrap_or(root)
            .join("navi-test-data-package-manager"),
        crate::SecurityConfig::default(),
    )
    .expect("policy");
    crate::tool::ToolExecutor::new(policy)
}

#[tokio::test]
async fn invoke_install_dispatches_to_cargo() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("Cargo.toml"), "[package]\nname=\"t\"\n").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "install", "manager": "cargo" }),
            },
            None,
        )
        .await;
    assert!(
        result.output["manager"] == "cargo"
            || result.output["error_code"] == "package_install_failed"
    );
}

#[tokio::test]
async fn invoke_add_dispatches_to_cargo() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("Cargo.toml"), "[package]\nname=\"t\"\n").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "add", "manager": "cargo", "packages": ["serde"] }),
            },
            None,
        )
        .await;
    assert!(
        result.output["manager"] == "cargo" || result.output["error_code"] == "package_add_failed"
    );
}

#[tokio::test]
async fn invoke_remove_dispatches_to_cargo() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("Cargo.toml"), "[package]\nname=\"t\"\n").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "remove", "manager": "cargo", "packages": ["serde"] }),
            },
            None,
        )
        .await;
    assert!(
        result.output["manager"] == "cargo"
            || result.output["error_code"] == "package_remove_failed"
    );
}

#[tokio::test]
async fn invoke_update_dispatches_to_cargo() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("Cargo.toml"), "[package]\nname=\"t\"\n").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "update", "manager": "cargo" }),
            },
            None,
        )
        .await;
    assert!(
        result.output["manager"] == "cargo"
            || result.output["error_code"] == "package_update_failed"
    );
}

#[tokio::test]
async fn invoke_check_dispatches_to_cargo() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(
        tempdir.path().join("Cargo.toml"),
        "[dependencies]\nserde = \"1\"\n",
    )
    .unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "check", "manager": "cargo", "packages": ["serde"] }),
            },
            None,
        )
        .await;
    assert!(result.ok);
    assert_eq!(result.output["manager"], "cargo");
}

#[tokio::test]
async fn invoke_install_dispatches_to_npm() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("package.json"), "{}").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "install", "manager": "npm" }),
            },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

#[tokio::test]
async fn invoke_install_dispatches_to_go() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("go.mod"), "module test").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "install", "manager": "go" }),
            },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

#[tokio::test]
async fn invoke_add_dispatches_to_npm() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("package.json"), "{}").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "add", "manager": "npm", "packages": ["express"] }),
            },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

#[tokio::test]
async fn invoke_add_dispatches_to_go() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("go.mod"), "module test").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
            id: "pm".to_string(),
            tool_name: "package_manager".to_string(),
            input: json!({ "action": "add", "manager": "go", "packages": ["github.com/acme/pkg"] }),
        },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

#[tokio::test]
async fn invoke_remove_dispatches_to_npm() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("package.json"), "{}").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "remove", "manager": "npm", "packages": ["express"] }),
            },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

#[tokio::test]
async fn invoke_remove_dispatches_to_go() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("go.mod"), "module test").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
            id: "pm".to_string(),
            tool_name: "package_manager".to_string(),
            input: json!({ "action": "remove", "manager": "go", "packages": ["github.com/acme/pkg"] }),
        },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

#[tokio::test]
async fn invoke_update_dispatches_to_npm() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("package.json"), "{}").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "update", "manager": "npm" }),
            },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

#[tokio::test]
async fn invoke_update_dispatches_to_go() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("go.mod"), "module test").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "update", "manager": "go" }),
            },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

// ── run_pkg direct test ───────────────────────────────────────────────

#[tokio::test]
async fn run_pkg_echo_success() {
    let tempdir = tempfile::tempdir().unwrap();
    let (ok, stdout, _) = run_pkg(tempdir.path(), &["echo hello"]).await.unwrap();
    assert!(ok);
    assert_eq!(stdout.trim(), "hello");
}

// ── Constant assertion ────────────────────────────────────────────────

#[test]
fn pkg_output_limit_constant() {
    assert_eq!(PKG_OUTPUT_LIMIT_BYTES, 32768);
}

// ── "auto" manager detection ──────────────────────────────────────────

#[tokio::test]
async fn invoke_auto_detects_cargo() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("Cargo.toml"), "[package]\nname=\"t\"\n").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "install", "manager": "auto" }),
            },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

// ── bun dispatch ──────────────────────────────────────────────────────

#[tokio::test]
async fn invoke_install_dispatches_to_bun() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("package.json"), "{}").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "install", "manager": "bun" }),
            },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

#[tokio::test]
async fn invoke_add_dispatches_to_bun() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("package.json"), "{}").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "add", "manager": "bun", "packages": ["express"] }),
            },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

#[tokio::test]
async fn invoke_remove_dispatches_to_bun() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("package.json"), "{}").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "remove", "manager": "bun", "packages": ["express"] }),
            },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

#[tokio::test]
async fn invoke_update_dispatches_to_bun_empty() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("package.json"), "{}").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "update", "manager": "bun" }),
            },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

#[tokio::test]
async fn invoke_update_dispatches_to_bun_with_packages() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("package.json"), "{}").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "update", "manager": "bun", "packages": ["express"] }),
            },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

// ── update with packages ──────────────────────────────────────────────

#[tokio::test]
async fn invoke_update_dispatches_to_npm_with_packages() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("package.json"), "{}").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "update", "manager": "npm", "packages": ["express"] }),
            },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

#[tokio::test]
async fn invoke_update_dispatches_to_cargo_with_packages() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("Cargo.toml"), "[package]\nname=\"t\"\n").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "update", "manager": "cargo", "packages": ["serde"] }),
            },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

#[tokio::test]
async fn invoke_update_dispatches_to_go_with_packages() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("go.mod"), "module test").unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
            id: "pm".to_string(),
            tool_name: "package_manager".to_string(),
            input: json!({ "action": "update", "manager": "go", "packages": ["github.com/acme/pkg"] }),
        },
            None,
        )
        .await;
    let manager = result.output["manager"]
        .as_str()
        .or_else(|| result.output["message"].as_str());
    assert!(manager.is_some());
    assert!(!manager.unwrap().contains("unsupported"));
}

// ── check dispatch for npm/bun ────────────────────────────────────────

#[tokio::test]
async fn invoke_check_dispatches_to_npm() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("package.json"), "{}").unwrap();
    std::fs::create_dir(tempdir.path().join("node_modules")).unwrap();
    std::fs::create_dir_all(tempdir.path().join("node_modules/express")).unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "check", "manager": "npm", "packages": ["express"] }),
            },
            None,
        )
        .await;
    assert!(result.ok);
    assert_eq!(result.output["manager"], "npm");
}

#[tokio::test]
async fn invoke_check_dispatches_to_bun() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(tempdir.path().join("package.json"), "{}").unwrap();
    std::fs::create_dir(tempdir.path().join("node_modules")).unwrap();
    std::fs::create_dir_all(tempdir.path().join("node_modules/express")).unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "check", "manager": "bun", "packages": ["express"] }),
            },
            None,
        )
        .await;
    assert!(result.ok);
    assert_eq!(result.output["manager"], "bun");
}

#[tokio::test]
async fn invoke_check_dispatches_to_go() {
    let tempdir = tempfile::tempdir().unwrap();
    std::fs::write(
        tempdir.path().join("go.mod"),
        "module example\n\nrequire (\n\tgithub.com/acme/pkg v1.2.3\n)\n",
    )
    .unwrap();
    let _guard = ChangeDirGuard::new(tempdir.path());
    let executor = pm_executor(tempdir.path());
    let result = executor
        .invoke_approved_with_event_tx(
            crate::tool::ToolInvocation {
            id: "pm".to_string(),
            tool_name: "package_manager".to_string(),
            input: json!({ "action": "check", "manager": "go", "packages": ["github.com/acme/pkg"] }),
        },
            None,
        )
        .await;
    assert!(result.ok);
    assert_eq!(result.output["manager"], "go");
}

// Helper: temporarily change working directory (thread-safe via mutex)
use std::sync::Mutex;
static CWD_MUTEX: Mutex<()> = Mutex::new(());

struct ChangeDirGuard {
    original: std::path::PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl ChangeDirGuard {
    fn new(dir: &Path) -> Self {
        let _lock = CWD_MUTEX.lock().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir).unwrap();
        Self { original, _lock }
    }
}

impl Drop for ChangeDirGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.original).unwrap();
    }
}
