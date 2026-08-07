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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Tool, ToolInvocation};
    use serde_json::json;
    use std::sync::Mutex;

    // Serialize tests that touch the global LAST_SNAPSHOT to prevent race
    // conditions between parallel tests.
    static SNAPSHOT_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn make_tool(root: &std::path::Path) -> SandboxTool {
        SandboxTool::new(root.to_path_buf())
    }

    fn make_invocation(id: &str, input: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            id: id.to_string(),
            tool_name: "sandbox".to_string(),
            input,
        }
    }

    /// Acquire the global lock and reset LAST_SNAPSHOT before each test.
    /// The guard is held for the duration of the test.
    fn lock_and_reset() -> std::sync::MutexGuard<'static, ()> {
        let guard = SNAPSHOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Reset the global snapshot.
        if let Ok(mut g) = LAST_SNAPSHOT.lock() {
            *g = None;
        }
        guard
    }

    // ── definition ────────────────────────────────────────────────────────

    #[test]
    fn definition_name_is_sandbox() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let def = tool.definition();
        assert_eq!(def.name, "sandbox");
    }

    #[test]
    fn definition_kind_is_command() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let def = tool.definition();
        assert_eq!(def.kind, ToolKind::Command);
    }

    #[test]
    fn definition_schema_action_enum() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let def = tool.definition();
        let actions = def.input_schema["properties"]["action"]["enum"]
            .as_array()
            .unwrap();
        let names: Vec<&str> = actions.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(names.contains(&"snapshot"));
        assert!(names.contains(&"rollback"));
        assert!(names.contains(&"status"));
        assert!(names.contains(&"reset"));
    }

    #[test]
    fn definition_schema_requires_action() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let def = tool.definition();
        let required = def.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(required.contains(&"action"));
    }

    // ── resolve_paths ─────────────────────────────────────────────────────

    #[test]
    fn resolve_paths_absolute_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let abs = temp.path().join("file.txt");
        let resolved = tool.resolve_paths(&[json!(abs.to_string_lossy())]);
        assert_eq!(resolved, vec![abs.clone()]);
    }

    #[test]
    fn resolve_paths_relative_joined_to_root() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let resolved = tool.resolve_paths(&[json!("src/main.rs")]);
        assert_eq!(resolved, vec![temp.path().join("src/main.rs")]);
    }

    #[test]
    fn resolve_paths_empty_vec() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let resolved = tool.resolve_paths(&[]);
        assert!(resolved.is_empty());
    }

    #[test]
    fn resolve_paths_filters_non_strings() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let resolved = tool.resolve_paths(&[json!("ok"), json!(42), json!(null)]);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0], temp.path().join("ok"));
    }

    #[test]
    fn resolve_paths_mixed_absolute_and_relative() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let abs = temp.path().join("abs.txt");
        let resolved = tool.resolve_paths(&[json!(abs.to_string_lossy()), json!("rel.txt")]);
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0], abs);
        assert_eq!(resolved[1], temp.path().join("rel.txt"));
    }

    // ── handle_snapshot ───────────────────────────────────────────────────

    #[tokio::test]
    async fn snapshot_with_missing_paths_returns_error() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation("s1", json!({"action": "snapshot"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "error");
        assert!(
            result.output["error"]
                .as_str()
                .is_some_and(|e| e.contains("paths")),
            "should mention paths: {result:?}"
        );
    }

    #[tokio::test]
    async fn snapshot_with_empty_paths_array_returns_error() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation("s2", json!({"action": "snapshot", "paths": []}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "error");
    }

    #[tokio::test]
    async fn snapshot_with_nonexistent_path_returns_error() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation(
            "s3",
            json!({"action": "snapshot", "paths": ["nonexistent_file.txt"]}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "error");
        assert!(
            result.output["error"]
                .as_str()
                .is_some_and(|e| e.contains("do not exist")),
            "should mention missing paths: {result:?}"
        );
        assert!(result.output["missing_paths"].is_array());
    }

    #[tokio::test]
    async fn snapshot_with_existing_file_succeeds() {
        let _guard = lock_and_reset();
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("file.txt"), "content").unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation("s4", json!({"action": "snapshot", "paths": ["file.txt"]}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "ok");
        assert!(
            result.output["snapshot_id"].as_str().is_some(),
            "should have snapshot_id: {result:?}"
        );
        assert!(
            result.output["files_snapshotted"].as_u64().is_some(),
            "should have files_snapshotted: {result:?}"
        );
    }

    #[tokio::test]
    async fn snapshot_with_directory_succeeds() {
        let _guard = lock_and_reset();
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "fn main() {}").unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation("s5", json!({"action": "snapshot", "paths": ["src"]}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "ok");
    }

    #[tokio::test]
    async fn snapshot_with_absolute_path_succeeds() {
        let _guard = lock_and_reset();
        let temp = tempfile::tempdir().unwrap();
        let abs = temp.path().join("abs.txt");
        std::fs::write(&abs, "content").unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation(
            "s6",
            json!({"action": "snapshot", "paths": [abs.to_string_lossy()]}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "ok");
    }

    // ── handle_rollback ───────────────────────────────────────────────────

    #[tokio::test]
    async fn rollback_without_snapshot_returns_error() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        // First reset to ensure no snapshot from a previous test.
        let _guard = lock_and_reset();

        let inv = make_invocation("r1", json!({"action": "rollback"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "error");
        assert!(
            result.output["error"]
                .as_str()
                .is_some_and(|e| e.contains("No snapshot")),
            "should mention no snapshot: {result:?}"
        );
    }

    #[tokio::test]
    async fn rollback_after_snapshot_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("file.txt"), "original").unwrap();
        let tool = make_tool(temp.path());
        let _guard = lock_and_reset();

        // Snapshot
        let inv = make_invocation("r2s", json!({"action": "snapshot", "paths": ["file.txt"]}));
        tool.invoke(inv).await.unwrap();

        // Modify file
        std::fs::write(temp.path().join("file.txt"), "modified").unwrap();

        // Rollback
        let inv = make_invocation("r2r", json!({"action": "rollback"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "ok");
        assert_eq!(result.output["rolled_back"], true);
        // File should be restored.
        let content = std::fs::read_to_string(temp.path().join("file.txt")).unwrap();
        assert_eq!(content, "original");
    }

    #[tokio::test]
    async fn rollback_with_no_changes_reports_rolled_back_false() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("file.txt"), "unchanged").unwrap();
        let tool = make_tool(temp.path());
        let _guard = lock_and_reset();

        // Snapshot
        let inv = make_invocation("r3s", json!({"action": "snapshot", "paths": ["file.txt"]}));
        tool.invoke(inv).await.unwrap();

        // Rollback without changes
        let inv = make_invocation("r3r", json!({"action": "rollback"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "ok");
        assert_eq!(result.output["rolled_back"], false);
    }

    // ── handle_status ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn status_without_snapshot_returns_error() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let _guard = lock_and_reset();

        let inv = make_invocation("st1", json!({"action": "status"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "error");
        assert!(
            result.output["error"]
                .as_str()
                .is_some_and(|e| e.contains("No snapshot")),
            "should mention no snapshot: {result:?}"
        );
    }

    #[tokio::test]
    async fn status_after_snapshot_no_changes() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("file.txt"), "content").unwrap();
        let tool = make_tool(temp.path());
        let _guard = lock_and_reset();

        let inv = make_invocation("st2s", json!({"action": "snapshot", "paths": ["file.txt"]}));
        tool.invoke(inv).await.unwrap();

        let inv = make_invocation("st2", json!({"action": "status"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "ok");
        assert_eq!(result.output["has_changes"], false);
    }

    #[tokio::test]
    async fn status_after_modification_reports_changes() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("file.txt"), "original").unwrap();
        let tool = make_tool(temp.path());
        let _guard = lock_and_reset();

        let inv = make_invocation("st3s", json!({"action": "snapshot", "paths": ["file.txt"]}));
        tool.invoke(inv).await.unwrap();

        // Modify
        std::fs::write(temp.path().join("file.txt"), "modified").unwrap();

        let inv = make_invocation("st3", json!({"action": "status"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "ok");
        assert_eq!(result.output["has_changes"], true);
    }

    // ── handle_reset ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn reset_clears_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("file.txt"), "content").unwrap();
        let tool = make_tool(temp.path());
        let _guard = lock_and_reset();

        // Snapshot
        let inv = make_invocation("rs1s", json!({"action": "snapshot", "paths": ["file.txt"]}));
        tool.invoke(inv).await.unwrap();

        // Reset
        let inv = make_invocation("rs1", json!({"action": "reset"}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "ok");
        assert!(
            result.output["message"]
                .as_str()
                .is_some_and(|m| m.contains("cleared")),
            "should mention cleared: {result:?}"
        );

        // Status should now report no snapshot.
        let inv = make_invocation("rs1v", json!({"action": "status"}));
        let result = tool.invoke(inv).await.unwrap();
        assert_eq!(result.output["status"], "error");
    }

    // ── unknown action ────────────────────────────────────────────────────

    #[tokio::test]
    async fn unknown_action_returns_error() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation("u1", json!({"action": "unknown"}));
        let result = tool.invoke(inv).await.unwrap();
        // Unknown action returns ok=false (not a tool error, but a logical error).
        assert!(!result.ok);
        assert_eq!(result.output["error_code"], "unknown_action");
        assert!(
            result.output["message"]
                .as_str()
                .is_some_and(|m| m.contains("unknown sandbox action")),
            "should mention unknown action: {result:?}"
        );
    }

    // ── missing action field ──────────────────────────────────────────────

    #[tokio::test]
    async fn missing_action_returns_error() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation("m1", json!({}));
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "missing action should return Err");
    }

    // ── change_set_summary ────────────────────────────────────────────────

    #[test]
    fn change_set_summary_empty() {
        let cs = ChangeSet {
            files_created: vec![],
            files_modified: vec![],
            files_deleted: vec![],
            diff: None,
        };
        let summary = change_set_summary(&cs);
        assert!(summary.files_created.is_empty());
        assert!(summary.files_modified.is_empty());
        assert!(summary.files_deleted.is_empty());
        assert_eq!(summary.total, 0);
    }

    #[test]
    fn change_set_summary_with_entries() {
        let cs = ChangeSet {
            files_created: vec![std::path::PathBuf::from("new.txt")],
            files_modified: vec![std::path::PathBuf::from("mod.txt")],
            files_deleted: vec![std::path::PathBuf::from("del.txt")],
            diff: None,
        };
        let summary = change_set_summary(&cs);
        assert_eq!(summary.files_created, vec!["new.txt"]);
        assert_eq!(summary.files_modified, vec!["mod.txt"]);
        assert_eq!(summary.files_deleted, vec!["del.txt"]);
        assert_eq!(summary.total, 3);
    }
}
