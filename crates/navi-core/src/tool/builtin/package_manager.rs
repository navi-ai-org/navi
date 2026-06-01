use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

const PKG_OUTPUT_LIMIT_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq)]
struct PackageEntry {
    name: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    section: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct PackageCheckOutput {
    status: &'static str,
    manager: String,
    installed: Vec<PackageEntry>,
    not_found: Vec<String>,
}

pub(crate) struct PackageManagerTool {
    project_root: PathBuf,
}

impl PackageManagerTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }
}

#[async_trait]
impl Tool for PackageManagerTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "package_manager",
            "Manage project dependencies. Auto-detects package manager from lockfiles (npm, bun, cargo, go). Actions: install (install all deps), add (add packages), remove (remove packages), update (update packages), check (verify installed).",
            ToolKind::Write,
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["install", "add", "remove", "update", "check"],
                        "description": "Operation to perform."
                    },
                    "packages": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Package names for add/remove/update. Not needed for install."
                    },
                    "dev": {
                        "type": "boolean",
                        "description": "Install as dev dependency. Defaults to false."
                    },
                    "manager": {
                        "type": "string",
                        "enum": ["auto", "npm", "bun", "cargo", "go"],
                        "description": "Package manager to use. Defaults to auto-detect."
                    }
                },
                "required": ["action"],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let action = helpers::required_string(&invocation.input, "action")?.to_string();
        let packages: Vec<String> = invocation
            .input
            .get("packages")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let dev = helpers::optional_bool(&invocation.input, "dev").unwrap_or(false);
        let manager = helpers::optional_string(&invocation.input, "manager")
            .unwrap_or_else(|| "auto".to_string());

        let detected = if manager == "auto" {
            detect_package_manager(&self.project_root).await?
        } else {
            manager.clone()
        };

        match action.as_str() {
            "install" => cmd_install(&self.project_root, &invocation.id, &detected).await,
            "add" => {
                cmd_add(
                    &self.project_root,
                    &invocation.id,
                    &detected,
                    &packages,
                    dev,
                )
                .await
            }
            "remove" => cmd_remove(&self.project_root, &invocation.id, &detected, &packages).await,
            "update" => cmd_update(&self.project_root, &invocation.id, &detected, &packages).await,
            "check" => cmd_check(&self.project_root, &invocation.id, &detected, &packages).await,
            _ => Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: helpers::tool_error(
                    "unknown_package_action",
                    format!("unknown package_manager action: {action}"),
                    true,
                    Some("Use install, add, remove, update, or check."),
                    None,
                ),
            }),
        }
    }
}

async fn run_pkg(project_root: &Path, args: &[&str]) -> Result<(bool, String, String)> {
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-lc").arg(args.join(" ")).current_dir(project_root);
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to run package manager")?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((output.status.success(), stdout, stderr))
}

async fn detect_package_manager(project_root: &Path) -> Result<String> {
    if project_root.join("bun.lockb").exists() {
        return Ok("bun".to_string());
    }
    if project_root.join("package-lock.json").exists() || project_root.join("package.json").exists()
    {
        return Ok("npm".to_string());
    }
    if project_root.join("Cargo.lock").exists() || project_root.join("Cargo.toml").exists() {
        return Ok("cargo".to_string());
    }
    if project_root.join("go.sum").exists() || project_root.join("go.mod").exists() {
        return Ok("go".to_string());
    }
    anyhow::bail!("no package manager detected in current directory");
}

async fn cmd_install(
    project_root: &Path,
    invocation_id: &str,
    manager: &str,
) -> Result<ToolResult> {
    let cmd = match manager {
        "npm" => "npm install",
        "bun" => "bun install",
        "cargo" => "cargo fetch",
        "go" => "go mod download",
        _ => anyhow::bail!("unsupported package manager: {manager}"),
    };

    let (ok, stdout, stderr) = run_pkg(project_root, &[cmd]).await?;

    Ok(ToolResult {
        invocation_id: invocation_id.to_string(),
        ok,
        output: if ok {
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "status": "success",
                "manager": manager,
                "output": helpers::truncate_string(stdout, PKG_OUTPUT_LIMIT_BYTES),
            })
        } else {
            helpers::tool_error(
                "package_install_failed",
                format!("{manager} install failed"),
                true,
                None,
                Some(helpers::truncate_string(stderr, PKG_OUTPUT_LIMIT_BYTES)),
            )
        },
    })
}

async fn cmd_add(
    project_root: &Path,
    invocation_id: &str,
    manager: &str,
    packages: &[String],
    dev: bool,
) -> Result<ToolResult> {
    if packages.is_empty() {
        return Ok(ToolResult {
            invocation_id: invocation_id.to_string(),
            ok: false,
            output: helpers::tool_error(
                "missing_packages",
                "no packages specified for add",
                true,
                Some("Pass packages, for example: [\"serde\"]."),
                None,
            ),
        });
    }

    let pkgs = packages.join(" ");
    let cmd = match manager {
        "npm" => {
            if dev {
                format!("npm install --save-dev {pkgs}")
            } else {
                format!("npm install {pkgs}")
            }
        }
        "bun" => {
            if dev {
                format!("bun add --dev {pkgs}")
            } else {
                format!("bun add {pkgs}")
            }
        }
        "cargo" => {
            let mut args = format!("cargo add {pkgs}");
            if dev {
                args.push_str(" --dev");
            }
            args
        }
        "go" => format!("go get {pkgs}"),
        _ => anyhow::bail!("unsupported package manager: {manager}"),
    };

    let (ok, stdout, stderr) = run_pkg(project_root, &[&cmd]).await?;

    let installed: Vec<Value> = packages
        .iter()
        .map(|p| json!({ "name": p, "version": "latest" }))
        .collect();

    Ok(ToolResult {
        invocation_id: invocation_id.to_string(),
        ok,
        output: if ok {
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "status": "success",
                "manager": manager,
                "installed": installed,
                "lockfile_changed": true,
                "output": helpers::truncate_string(stdout, PKG_OUTPUT_LIMIT_BYTES),
            })
        } else {
            helpers::tool_error(
                "package_add_failed",
                format!("{manager} add failed"),
                true,
                None,
                Some(helpers::truncate_string(stderr, PKG_OUTPUT_LIMIT_BYTES)),
            )
        },
    })
}

async fn cmd_remove(
    project_root: &Path,
    invocation_id: &str,
    manager: &str,
    packages: &[String],
) -> Result<ToolResult> {
    if packages.is_empty() {
        return Ok(ToolResult {
            invocation_id: invocation_id.to_string(),
            ok: false,
            output: helpers::tool_error(
                "missing_packages",
                "no packages specified for remove",
                true,
                Some("Pass packages, for example: [\"serde\"]."),
                None,
            ),
        });
    }

    let pkgs = packages.join(" ");
    let cmd = match manager {
        "npm" => format!("npm uninstall {pkgs}"),
        "bun" => format!("bun remove {pkgs}"),
        "cargo" => format!("cargo rm {pkgs}"),
        "go" => format!("go mod edit -droprequire {pkgs} && go mod tidy"),
        _ => anyhow::bail!("unsupported package manager: {manager}"),
    };

    let (ok, stdout, stderr) = run_pkg(project_root, &[&cmd]).await?;

    Ok(ToolResult {
        invocation_id: invocation_id.to_string(),
        ok,
        output: if ok {
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "status": "success",
                "manager": manager,
                "removed": packages,
                "lockfile_changed": true,
                "output": helpers::truncate_string(stdout, PKG_OUTPUT_LIMIT_BYTES),
            })
        } else {
            helpers::tool_error(
                "package_remove_failed",
                format!("{manager} remove failed"),
                true,
                None,
                Some(helpers::truncate_string(stderr, PKG_OUTPUT_LIMIT_BYTES)),
            )
        },
    })
}

async fn cmd_update(
    project_root: &Path,
    invocation_id: &str,
    manager: &str,
    packages: &[String],
) -> Result<ToolResult> {
    let cmd = if packages.is_empty() {
        match manager {
            "npm" => "npm update".to_string(),
            "bun" => "bun update".to_string(),
            "cargo" => "cargo update".to_string(),
            "go" => "go get -u ./...".to_string(),
            _ => anyhow::bail!("unsupported package manager: {manager}"),
        }
    } else {
        let pkgs = packages.join(" ");
        match manager {
            "npm" => format!("npm update {pkgs}"),
            "bun" => format!("bun update {pkgs}"),
            "cargo" => format!("cargo update {pkgs}"),
            "go" => format!("go get -u {pkgs}"),
            _ => anyhow::bail!("unsupported package manager: {manager}"),
        }
    };

    let (ok, stdout, stderr) = run_pkg(project_root, &[&cmd]).await?;

    Ok(ToolResult {
        invocation_id: invocation_id.to_string(),
        ok,
        output: if ok {
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "status": "success",
                "manager": manager,
                "lockfile_changed": true,
                "output": helpers::truncate_string(stdout, PKG_OUTPUT_LIMIT_BYTES),
            })
        } else {
            helpers::tool_error(
                "package_update_failed",
                format!("{manager} update failed"),
                true,
                None,
                Some(helpers::truncate_string(stderr, PKG_OUTPUT_LIMIT_BYTES)),
            )
        },
    })
}

async fn cmd_check(
    project_root: &Path,
    invocation_id: &str,
    manager: &str,
    packages: &[String],
) -> Result<ToolResult> {
    match manager {
        "npm" | "bun" => check_npm(project_root, invocation_id, manager, packages).await,
        "cargo" => check_cargo(project_root, invocation_id, packages).await,
        "go" => check_go(project_root, invocation_id, packages).await,
        _ => Ok(ToolResult {
            invocation_id: invocation_id.to_string(),
            ok: false,
            output: helpers::tool_error(
                "unsupported_package_manager",
                format!("check not supported for {manager}"),
                true,
                None,
                None,
            ),
        }),
    }
}

async fn check_npm(
    project_root: &Path,
    invocation_id: &str,
    manager: &str,
    packages: &[String],
) -> Result<ToolResult> {
    // Small manifest files (<10KB); blocking the runtime for microseconds is
    // cheaper than the spawn_blocking thread-pool overhead.
    let manifest = std::fs::read_to_string(project_root.join("package.json")).unwrap_or_default();
    let mut installed: Vec<PackageEntry> = Vec::new();
    let mut not_found: Vec<String> = Vec::new();

    for pkg in packages {
        if let Some(section) = find_npm_package(&manifest, pkg) {
            installed.push(PackageEntry {
                name: pkg.clone(),
                status: "found",
                section: Some(section),
            });
        } else {
            not_found.push(pkg.clone());
        }
    }

    // If no specific packages, check if node_modules exists
    if packages.is_empty() {
        let has_modules = project_root.join("node_modules").exists();
        return Ok(helpers::ok(
            invocation_id.to_string(),
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "status": if has_modules { "installed" } else { "not_installed" },
                "manager": manager,
            }),
        ));
    }

    Ok(helpers::ok(
        invocation_id.to_string(),
        helpers::versioned(PackageCheckOutput {
            status: if not_found.is_empty() {
                "success"
            } else {
                "not_found"
            },
            manager: manager.to_string(),
            installed,
            not_found,
        }),
    ))
}

async fn check_cargo(
    project_root: &Path,
    invocation_id: &str,
    packages: &[String],
) -> Result<ToolResult> {
    let manifest = std::fs::read_to_string(project_root.join("Cargo.toml")).unwrap_or_default();
    let mut installed: Vec<PackageEntry> = Vec::new();
    let mut not_found: Vec<String> = Vec::new();

    for pkg in packages {
        if let Some(section) = find_cargo_package(&manifest, pkg) {
            installed.push(PackageEntry {
                name: pkg.clone(),
                status: "found",
                section: Some(section),
            });
        } else {
            not_found.push(pkg.clone());
        }
    }

    if packages.is_empty() {
        let has_lock = project_root.join("Cargo.lock").exists();
        return Ok(helpers::ok(
            invocation_id.to_string(),
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "status": if has_lock { "installed" } else { "not_installed" },
                "manager": "cargo",
            }),
        ));
    }

    Ok(helpers::ok(
        invocation_id.to_string(),
        helpers::versioned(PackageCheckOutput {
            status: if not_found.is_empty() {
                "success"
            } else {
                "not_found"
            },
            manager: "cargo".to_string(),
            installed,
            not_found,
        }),
    ))
}

async fn check_go(
    project_root: &Path,
    invocation_id: &str,
    packages: &[String],
) -> Result<ToolResult> {
    let manifest = std::fs::read_to_string(project_root.join("go.mod")).unwrap_or_default();
    let mut installed: Vec<PackageEntry> = Vec::new();
    let mut not_found: Vec<String> = Vec::new();

    for pkg in packages {
        if find_go_package(&manifest, pkg) {
            installed.push(PackageEntry {
                name: pkg.clone(),
                status: "found",
                section: Some("require".to_string()),
            });
        } else {
            not_found.push(pkg.clone());
        }
    }

    if packages.is_empty() {
        let has_sum = project_root.join("go.sum").exists();
        return Ok(helpers::ok(
            invocation_id.to_string(),
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "status": if has_sum { "installed" } else { "not_installed" },
                "manager": "go",
            }),
        ));
    }

    Ok(helpers::ok(
        invocation_id.to_string(),
        helpers::versioned(PackageCheckOutput {
            status: if not_found.is_empty() {
                "success"
            } else {
                "not_found"
            },
            manager: "go".to_string(),
            installed,
            not_found,
        }),
    ))
}

fn find_npm_package(manifest: &str, package: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(manifest).ok()?;
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if parsed
            .get(section)
            .and_then(Value::as_object)
            .is_some_and(|deps| deps.contains_key(package))
        {
            return Some(section.to_string());
        }
    }
    None
}

fn find_cargo_package(manifest: &str, package: &str) -> Option<String> {
    let parsed: toml::Value = toml::from_str(manifest).ok()?;
    for section in [
        "dependencies",
        "dev-dependencies",
        "build-dependencies",
        "workspace.dependencies",
    ] {
        let table = section
            .split('.')
            .try_fold(&parsed, |value, key| value.get(key));
        if table
            .and_then(toml::Value::as_table)
            .is_some_and(|deps| deps.contains_key(package))
        {
            return Some(section.to_string());
        }
    }
    None
}

fn find_go_package(manifest: &str, package: &str) -> bool {
    let mut in_require_block = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with("require (") {
            in_require_block = true;
            continue;
        }
        if in_require_block && line == ")" {
            in_require_block = false;
            continue;
        }
        let require_line = if let Some(rest) = line.strip_prefix("require ") {
            rest.trim()
        } else if in_require_block {
            line
        } else {
            continue;
        };
        if require_line.split_whitespace().next() == Some(package) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── find_npm_package ─────────────────────────────────────────────────

    #[test]
    fn find_npm_package_in_dependencies() {
        let manifest = r#"{"dependencies": {"express": "4"}}"#;
        assert_eq!(
            find_npm_package(manifest, "express"),
            Some("dependencies".to_string())
        );
    }

    #[test]
    fn find_npm_package_in_dev_dependencies() {
        let manifest = r#"{"devDependencies": {"vitest": "1"}}"#;
        assert_eq!(
            find_npm_package(manifest, "vitest"),
            Some("devDependencies".to_string())
        );
    }

    #[test]
    fn find_npm_package_in_peer_dependencies() {
        let manifest = r#"{"peerDependencies": {"react": "18"}}"#;
        assert_eq!(
            find_npm_package(manifest, "react"),
            Some("peerDependencies".to_string())
        );
    }

    #[test]
    fn find_npm_package_in_optional_dependencies() {
        let manifest = r#"{"optionalDependencies": {"fsevents": "2"}}"#;
        assert_eq!(
            find_npm_package(manifest, "fsevents"),
            Some("optionalDependencies".to_string())
        );
    }

    #[test]
    fn find_npm_package_returns_none_for_missing() {
        let manifest = r#"{"dependencies": {"express": "4"}}"#;
        assert_eq!(find_npm_package(manifest, "missing"), None);
    }

    #[test]
    fn find_npm_package_returns_none_for_invalid_json() {
        assert_eq!(find_npm_package("not json", "pkg"), None);
    }

    // ── find_cargo_package ───────────────────────────────────────────────

    #[test]
    fn find_cargo_package_in_dependencies() {
        let manifest = "[dependencies]\nserde = \"1\"\n";
        assert_eq!(
            find_cargo_package(manifest, "serde"),
            Some("dependencies".to_string())
        );
    }

    #[test]
    fn find_cargo_package_in_dev_dependencies() {
        let manifest = "[dev-dependencies]\npretty_assertions = \"1\"\n";
        assert_eq!(
            find_cargo_package(manifest, "pretty_assertions"),
            Some("dev-dependencies".to_string())
        );
    }

    #[test]
    fn find_cargo_package_in_build_dependencies() {
        let manifest = "[build-dependencies]\ncc = \"1\"\n";
        assert_eq!(
            find_cargo_package(manifest, "cc"),
            Some("build-dependencies".to_string())
        );
    }

    #[test]
    fn find_cargo_package_in_workspace_dependencies() {
        let manifest = "[workspace.dependencies]\nserde = \"1\"\n";
        assert_eq!(
            find_cargo_package(manifest, "serde"),
            Some("workspace.dependencies".to_string())
        );
    }

    #[test]
    fn find_cargo_package_returns_none_for_missing() {
        let manifest = "[dependencies]\nserde = \"1\"\n";
        assert_eq!(find_cargo_package(manifest, "missing"), None);
    }

    #[test]
    fn find_cargo_package_returns_none_for_invalid_toml() {
        assert_eq!(find_cargo_package("not toml [[[", "pkg"), None);
    }

    // ── find_go_package ──────────────────────────────────────────────────

    #[test]
    fn find_go_package_in_require_block() {
        let manifest = "module example\n\nrequire (\n\tgithub.com/acme/pkg v1.2.3\n)\n";
        assert!(find_go_package(manifest, "github.com/acme/pkg"));
    }

    #[test]
    fn find_go_package_inline_require() {
        let manifest = "module example\n\nrequire github.com/acme/pkg v1.2.3\n";
        assert!(find_go_package(manifest, "github.com/acme/pkg"));
    }

    #[test]
    fn find_go_package_returns_false_for_missing() {
        let manifest = "module example\n\nrequire (\n\tgithub.com/acme/pkg v1.2.3\n)\n";
        assert!(!find_go_package(manifest, "github.com/missing/pkg"));
    }

    #[test]
    fn find_go_package_returns_false_for_empty_manifest() {
        assert!(!find_go_package("", "pkg"));
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

        let result = check_cargo(
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

        let result = check_cargo(tempdir.path(), "test", &[]).await.unwrap();
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

        let result = check_npm(tempdir.path(), "test", "npm", &["express".to_string()])
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

        let result = check_npm(tempdir.path(), "test", "npm", &["serde".to_string()])
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

        let result = check_cargo(tempdir.path(), "test", &["serde".to_string()])
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

        let result = check_go(tempdir.path(), "test", &["github.com/acme/pkg".to_string()])
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["status"], "success");
        assert_eq!(result.output["installed"][0]["section"], "require");
    }

    #[tokio::test]
    async fn check_npm_empty_packages_checks_node_modules() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::create_dir(tempdir.path().join("node_modules")).unwrap();
        let _guard = ChangeDirGuard::new(tempdir.path());

        let result = check_npm(tempdir.path(), "test", "npm", &[]).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "installed");
    }

    // ── invoke dispatch ──────────────────────────────────────────────────

    fn pm_executor(root: &Path) -> crate::tool::ToolExecutor {
        let policy = crate::SecurityPolicy::new(
            root.to_path_buf(),
            root.join(".navi-data"),
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "install", "manager": "cargo" }),
            })
            .await;
        // Should not bail with "unsupported package manager"
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "add", "manager": "cargo", "packages": ["serde"] }),
            })
            .await;
        assert!(
            result.output["manager"] == "cargo"
                || result.output["error_code"] == "package_add_failed"
        );
    }

    #[tokio::test]
    async fn invoke_remove_dispatches_to_cargo() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("Cargo.toml"), "[package]\nname=\"t\"\n").unwrap();
        let _guard = ChangeDirGuard::new(tempdir.path());
        let executor = pm_executor(tempdir.path());
        let result = executor
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "remove", "manager": "cargo", "packages": ["serde"] }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "update", "manager": "cargo" }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "check", "manager": "cargo", "packages": ["serde"] }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "install", "manager": "npm" }),
            })
            .await;
        // Verify the match arm was taken (manager is npm, not "unsupported")
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "install", "manager": "go" }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "add", "manager": "npm", "packages": ["express"] }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "add", "manager": "go", "packages": ["github.com/acme/pkg"] }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "remove", "manager": "npm", "packages": ["express"] }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "remove", "manager": "go", "packages": ["github.com/acme/pkg"] }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "update", "manager": "npm" }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "update", "manager": "go" }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "install", "manager": "auto" }),
            })
            .await;
        let manager = result.output["manager"]
            .as_str()
            .or_else(|| result.output["message"].as_str());
        assert!(manager.is_some());
        assert!(!manager.unwrap().contains("unsupported"));
    }

    // ── bun dispatch (delete match arm bun) ───────────────────────────────

    #[tokio::test]
    async fn invoke_install_dispatches_to_bun() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("package.json"), "{}").unwrap();
        let _guard = ChangeDirGuard::new(tempdir.path());
        let executor = pm_executor(tempdir.path());
        let result = executor
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "install", "manager": "bun" }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "add", "manager": "bun", "packages": ["express"] }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "remove", "manager": "bun", "packages": ["express"] }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "update", "manager": "bun" }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "update", "manager": "bun", "packages": ["express"] }),
            })
            .await;
        let manager = result.output["manager"]
            .as_str()
            .or_else(|| result.output["message"].as_str());
        assert!(manager.is_some());
        assert!(!manager.unwrap().contains("unsupported"));
    }

    // ── update with packages (non-empty branch for lines 315-318) ─────────

    #[tokio::test]
    async fn invoke_update_dispatches_to_npm_with_packages() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("package.json"), "{}").unwrap();
        let _guard = ChangeDirGuard::new(tempdir.path());
        let executor = pm_executor(tempdir.path());
        let result = executor
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "update", "manager": "npm", "packages": ["express"] }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "update", "manager": "cargo", "packages": ["serde"] }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "update", "manager": "go", "packages": ["github.com/acme/pkg"] }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "check", "manager": "npm", "packages": ["express"] }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "check", "manager": "bun", "packages": ["express"] }),
            })
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
            .invoke(crate::tool::ToolInvocation {
                id: "pm".to_string(),
                tool_name: "package_manager".to_string(),
                input: json!({ "action": "check", "manager": "go", "packages": ["github.com/acme/pkg"] }),
            })
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
}
