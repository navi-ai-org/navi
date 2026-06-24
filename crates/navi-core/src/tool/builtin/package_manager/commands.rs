use anyhow::Result;
use serde_json::json;
use std::path::Path;

use super::helpers;
use super::{PKG_OUTPUT_LIMIT_BYTES, run_pkg};
use crate::tool::ToolResult;

pub(super) async fn cmd_install(
    project_root: &Path,
    invocation_id: &str,
    manager: &str,
) -> Result<ToolResult> {
    let cmd = match manager {
        "dart" => "dart pub get",
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

pub(super) async fn cmd_add(
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
        "dart" => {
            if dev {
                format!("dart pub add --dev {pkgs}")
            } else {
                format!("dart pub add {pkgs}")
            }
        }
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

    let installed: Vec<serde_json::Value> = packages
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

pub(super) async fn cmd_remove(
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
        "dart" => format!("dart pub remove {pkgs}"),
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

pub(super) async fn cmd_update(
    project_root: &Path,
    invocation_id: &str,
    manager: &str,
    packages: &[String],
) -> Result<ToolResult> {
    let cmd = if packages.is_empty() {
        match manager {
            "dart" => "dart pub upgrade".to_string(),
            "npm" => "npm update".to_string(),
            "bun" => "bun update".to_string(),
            "cargo" => "cargo update".to_string(),
            "go" => "go get -u ./...".to_string(),
            _ => anyhow::bail!("unsupported package manager: {manager}"),
        }
    } else {
        let pkgs = packages.join(" ");
        match manager {
            "dart" => format!("dart pub upgrade {pkgs}"),
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
