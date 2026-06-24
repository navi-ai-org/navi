mod check;
mod commands;
mod finders;
#[cfg(test)]
mod tests;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::json;
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
            "Manage project dependencies. Auto-detects package manager from lockfiles (npm, bun, cargo, go, dart). Actions: install (install all deps), add (add packages), remove (remove packages), update (update packages), check (verify installed).",
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
                        "enum": ["auto", "npm", "bun", "cargo", "go", "dart"],
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
            "install" => commands::cmd_install(&self.project_root, &invocation.id, &detected).await,
            "add" => {
                commands::cmd_add(
                    &self.project_root,
                    &invocation.id,
                    &detected,
                    &packages,
                    dev,
                )
                .await
            }
            "remove" => {
                commands::cmd_remove(&self.project_root, &invocation.id, &detected, &packages).await
            }
            "update" => {
                commands::cmd_update(&self.project_root, &invocation.id, &detected, &packages).await
            }
            "check" => {
                check::cmd_check(&self.project_root, &invocation.id, &detected, &packages).await
            }
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
    if project_root.join("pubspec.yaml").exists() {
        return Ok("dart".to_string());
    }
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
