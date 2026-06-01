use anyhow::Result;
use serde_json::json;
use std::path::Path;

use super::finders;
use super::{PackageCheckOutput, PackageEntry, helpers};
use crate::tool::ToolResult;

pub(super) async fn cmd_check(
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

pub(super) async fn check_npm(
    project_root: &Path,
    invocation_id: &str,
    manager: &str,
    packages: &[String],
) -> Result<ToolResult> {
    let manifest = std::fs::read_to_string(project_root.join("package.json")).unwrap_or_default();
    let mut installed: Vec<PackageEntry> = Vec::new();
    let mut not_found: Vec<String> = Vec::new();

    for pkg in packages {
        if let Some(section) = finders::find_npm_package(&manifest, pkg) {
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

pub(super) async fn check_cargo(
    project_root: &Path,
    invocation_id: &str,
    packages: &[String],
) -> Result<ToolResult> {
    let manifest = std::fs::read_to_string(project_root.join("Cargo.toml")).unwrap_or_default();
    let mut installed: Vec<PackageEntry> = Vec::new();
    let mut not_found: Vec<String> = Vec::new();

    for pkg in packages {
        if let Some(section) = finders::find_cargo_package(&manifest, pkg) {
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

pub(super) async fn check_go(
    project_root: &Path,
    invocation_id: &str,
    packages: &[String],
) -> Result<ToolResult> {
    let manifest = std::fs::read_to_string(project_root.join("go.mod")).unwrap_or_default();
    let mut installed: Vec<PackageEntry> = Vec::new();
    let mut not_found: Vec<String> = Vec::new();

    for pkg in packages {
        if finders::find_go_package(&manifest, pkg) {
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
