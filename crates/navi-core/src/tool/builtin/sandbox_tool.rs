use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::PathBuf;

use super::helpers;
use crate::sandbox::{ChangeSet, SandboxManager, WorkspaceSnapshot};
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

/// In-memory snapshot store for the current session.
///
/// Tools are stateless by convention, but `sandbox` needs to hold on to the
/// most recent snapshot across calls. A static is acceptable here because
/// there is at most one sandbox session per process.
static LAST_SNAPSHOT: std::sync::Mutex<Option<WorkspaceSnapshot>> = std::sync::Mutex::new(None);

pub(crate) struct SandboxTool {
    project_root: PathBuf,
}

impl SandboxTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }
}

#[async_trait]
impl Tool for SandboxTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "sandbox",
            "Create snapshots of file state, detect changes, and roll back the \
             workspace to a previous snapshot. Use this to safely experiment \
             with file modifications knowing you can undo them.\n\n\
             Actions:\n\
             - `snapshot` — capture current file state\n\
             - `rollback` — restore the workspace to the last snapshot\n\
             - `status` — compare current state against the last snapshot\n\
             - `reset` — clear the in-memory snapshot without modifying files",
            ToolKind::Command,
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["snapshot", "rollback", "status", "reset"],
                        "description": "Operation to perform."
                    },
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Files or directories to include in the snapshot. Required for `snapshot`. Paths may be absolute or project-relative."
                    }
                },
                "required": ["action"],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let action = helpers::required_string(&invocation.input, "action")?.to_string();

        match action.as_str() {
            "snapshot" => self.handle_snapshot(&invocation),
            "rollback" => self.handle_rollback(&invocation),
            "status" => self.handle_status(&invocation),
            "reset" => self.handle_reset(&invocation),
            other => Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: helpers::tool_error(
                    "unknown_action",
                    format!(
                        "unknown sandbox action: `{other}`. Use one of: snapshot, rollback, status, reset."
                    ),
                    true,
                    Some(
                        "Use `snapshot` to capture state, `rollback` to undo changes, `status` to check drift, or `reset` to clear the snapshot.",
                    ),
                    None,
                ),
            }),
        }
    }
}

impl SandboxTool {
    fn resolve_paths(&self, raw: &[Value]) -> Vec<PathBuf> {
        raw.iter()
            .filter_map(|v| v.as_str())
            .map(|s| {
                let p = std::path::Path::new(s);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    self.project_root.join(p)
                }
            })
            .collect()
    }

    fn handle_snapshot(&self, invocation: &ToolInvocation) -> Result<ToolResult> {
        let raw_paths = match invocation.input.get("paths") {
            Some(Value::Array(arr)) if !arr.is_empty() => arr.clone(),
            _ => {
                return Ok(helpers::ok(
                    invocation.id.clone(),
                    json!({
                        "status": "error",
                        "error": "Missing or empty `paths` argument. Provide at least one file or directory to snapshot.",
                        "hint": "Example: {\"action\": \"snapshot\", \"paths\": [\".\"]}",
                    }),
                ));
            }
        };

        let paths = self.resolve_paths(&raw_paths);

        // Verify all paths exist.
        let missing: Vec<String> = paths
            .iter()
            .filter(|p| !p.exists())
            .map(|p| p.display().to_string())
            .collect();
        if !missing.is_empty() {
            return Ok(helpers::ok(
                invocation.id.clone(),
                json!({
                    "status": "error",
                    "error": format!("Paths do not exist: {}", missing.join(", ")),
                    "missing_paths": missing,
                }),
            ));
        }

        let snapshot = SandboxManager::create_snapshot(&paths);

        // Store for later rollback/status.
        if let Ok(mut guard) = LAST_SNAPSHOT.lock() {
            *guard = Some(snapshot.clone());
        }

        Ok(helpers::ok(
            invocation.id.clone(),
            json!({
                "status": "ok",
                "snapshot_id": snapshot.id,
                "files_snapshotted": snapshot.entries.len(),
                "created_at": snapshot.created_at,
            }),
        ))
    }

    fn handle_rollback(&self, invocation: &ToolInvocation) -> Result<ToolResult> {
        let snapshot = {
            let guard = LAST_SNAPSHOT
                .lock()
                .map_err(|e| anyhow::anyhow!("failed to acquire snapshot lock: {e}"))?;
            guard.as_ref().cloned()
        };

        let Some(snapshot) = snapshot else {
            return Ok(helpers::ok(
                invocation.id.clone(),
                json!({
                    "status": "error",
                    "error": "No snapshot available. Call `sandbox` with `action: snapshot` first.",
                    "hint": "Example: {\"action\": \"snapshot\", \"paths\": [\".\"]}",
                }),
            ));
        };

        // Compute changes for reporting.
        let changes = SandboxManager::compute_changes(&snapshot);
        let had_changes = !changes.is_empty();

        if let Err(e) = SandboxManager::rollback(&snapshot) {
            return Ok(helpers::ok(
                invocation.id.clone(),
                json!({
                    "status": "error",
                    "error": e,
                    "hint": "Some files could not be restored. Check the error message for details.",
                }),
            ));
        }

        Ok(helpers::ok(
            invocation.id.clone(),
            json!({
                "status": "ok",
                "snapshot_id": snapshot.id,
                "rolled_back": had_changes,
                "files_restored": changes.files_modified.len() + changes.files_deleted.len(),
                "files_created_and_removed": changes.files_created.len(),
                "changes": serde_json::to_value(change_set_summary(&changes)).unwrap_or_default(),
            }),
        ))
    }

    fn handle_status(&self, invocation: &ToolInvocation) -> Result<ToolResult> {
        let snapshot = {
            let guard = LAST_SNAPSHOT
                .lock()
                .map_err(|e| anyhow::anyhow!("failed to acquire snapshot lock: {e}"))?;
            guard.as_ref().cloned()
        };

        let Some(snapshot) = snapshot else {
            return Ok(helpers::ok(
                invocation.id.clone(),
                json!({
                    "status": "error",
                    "error": "No snapshot available. Call `sandbox` with `action: snapshot` first.",
                    "hint": "Example: {\"action\": \"snapshot\", \"paths\": [\".\"]}",
                }),
            ));
        };

        let changes = SandboxManager::compute_changes(&snapshot);

        Ok(helpers::ok(
            invocation.id.clone(),
            json!({
                "status": "ok",
                "snapshot_id": snapshot.id,
                "has_changes": !changes.is_empty(),
                "changes": serde_json::to_value(change_set_summary(&changes)).unwrap_or_default(),
            }),
        ))
    }

    fn handle_reset(&self, invocation: &ToolInvocation) -> Result<ToolResult> {
        if let Ok(mut guard) = LAST_SNAPSHOT.lock() {
            *guard = None;
        }

        Ok(helpers::ok(
            invocation.id.clone(),
            json!({
                "status": "ok",
                "message": "In-memory snapshot cleared. Files are unchanged.",
            }),
        ))
    }
}

/// Convert a ChangeSet into a JSON-friendly summary for the model.
#[derive(serde::Serialize)]
struct ChangeSetSummary {
    files_created: Vec<String>,
    files_modified: Vec<String>,
    files_deleted: Vec<String>,
    total: usize,
}

fn change_set_summary(cs: &ChangeSet) -> ChangeSetSummary {
    ChangeSetSummary {
        files_created: cs
            .files_created
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        files_modified: cs
            .files_modified
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        files_deleted: cs
            .files_deleted
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        total: cs.total(),
    }
}
