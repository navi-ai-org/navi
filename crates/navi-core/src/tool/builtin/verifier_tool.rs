//! Verifier tool — runs verification commands (build, test, typecheck, lint, or custom).
//!
//! Three actions:
//! - `run` — execute a verifier spec immediately, return the result
//! - `status` — retrieve a previously stored verification result by key
//! - `list` — list all stored verification results
//!
//! The tool is registered with `Deferred` exposure by default, so it does not
//! clutter the model's primary tool palette but can be discovered through
//! `tool.search`.

use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};
use crate::verifier::{VerificationStore, VerifierRunner, VerifierSpec};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

use super::helpers;

struct VerifierToolState {
    project_root: PathBuf,
    store: VerificationStore,
}

/// A tool that wraps the `VerifierRunner` for LLM-facing use.
///
/// Accepts a `VerifierSpec`-style input and returns a `VerifierResult`.
/// Results are stored in the shared `VerificationStore` for later retrieval.
pub(crate) struct VerifierTool {
    inner: Arc<VerifierToolState>,
}

impl VerifierTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self {
            inner: Arc::new(VerifierToolState {
                project_root,
                store: VerificationStore::new(),
            }),
        }
    }
}

#[async_trait]
impl Tool for VerifierTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "verifier",
            "Run verification commands (build, test, typecheck, lint) and capture structured results.\n\n\
             Actions:\n\
             - `run`: execute a verification command with optional timeout and cwd.\n\
             - `status`: retrieve a stored verification result by key.\n\
             - `list`: show all stored verification results.\n\n\
             When a write tool has a verifier hint in its metadata, use `run` to verify after writing.",
            ToolKind::Command,
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["run", "status", "list"],
                        "description": "What the verifier should do."
                    },
                    "verifier": {
                        "type": "string",
                        "enum": ["build", "test", "typecheck", "lint", "command"],
                        "description": "Category of verification (used for error classification). Required when action=run."
                    },
                    "command": {
                        "type": "string",
                        "description": "Shell command to run. Required when action=run."
                    },
                    "key": {
                        "type": "string",
                        "description": "Store/retrieval key for the verification result. Defaults to '<verifier>:<command>' when action=run. Required when action=status."
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory override (project-relative or absolute). Defaults to project root."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Timeout in milliseconds. Defaults to 120000 (2 minutes)."
                    },
                    "required": {
                        "type": "boolean",
                        "description": "Whether this verification is required. When false, a non-zero exit code does not block progress."
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let action = helpers::required_string(&invocation.input, "action")?.to_string();

        match action.as_str() {
            "run" => self.invoke_run(invocation).await,
            "status" => self.invoke_status(invocation).await,
            "list" => self.invoke_list(invocation).await,
            other => Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: json!({
                    "error_code": "invalid_action",
                    "error": format!("Unknown action `{other}`. Use run, status, or list."),
                }),
            }),
        }
    }
}

impl VerifierTool {
    async fn invoke_run(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let verifier_type = helpers::optional_string(&invocation.input, "verifier")
            .unwrap_or_else(|| "command".to_string());
        let command = helpers::required_string(&invocation.input, "command")?.to_string();
        let key = helpers::optional_string(&invocation.input, "key")
            .unwrap_or_else(|| format!("{verifier_type}:{command}"));
        let cwd = helpers::optional_string(&invocation.input, "cwd");
        let timeout_ms = helpers::optional_u64(&invocation.input, "timeout_ms");
        let required = helpers::optional_bool(&invocation.input, "required").unwrap_or(true);

        let spec = VerifierSpec {
            verifier_type,
            command,
            cwd,
            timeout_ms,
            required,
        };

        let project_root = self.inner.project_root.clone();
        let result = VerifierRunner::run(&spec, &project_root).await;

        // Store the result for later retrieval.
        let store = self.inner.store.clone();
        store.store(key.clone(), result.clone()).await;

        // Return the full VerifierResult plus metadata.
        let mut output =
            serde_json::to_value(&result).context("failed to serialize verifier result")?;
        if let Value::Object(ref mut map) = output {
            map.insert("key".to_string(), json!(key));
            map.insert("schema_version".to_string(), json!(1));
            // Add a human-readable summary.
            let summary = match result.status.as_str() {
                "pass" => format!("{} passed ({} ms)", result.command, result.duration_ms),
                "fail" => {
                    let ec = result.exit_code.unwrap_or(-1);
                    format!(
                        "{} failed (exit code {}) — {}",
                        result.command,
                        ec,
                        result.error_class.as_deref().unwrap_or("unknown error")
                    )
                }
                "error" => format!(
                    "{} error — {}",
                    result.command,
                    result.error_class.as_deref().unwrap_or("system error")
                ),
                "skipped" => format!("{} skipped", result.command),
                other => format!("{}: {}", other, result.command),
            };
            map.insert("summary".to_string(), json!(summary));
        }

        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: matches!(result.status.as_str(), "pass" | "skipped"),
            output,
        })
    }

    async fn invoke_status(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let key = helpers::required_string(&invocation.input, "key")?.to_string();
        let store = self.inner.store.clone();
        match store.get(&key).await {
            Some(result) => {
                let output =
                    serde_json::to_value(&result).context("failed to serialize verifier result")?;
                Ok(helpers::ok(invocation.id, output))
            }
            None => Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: json!({
                    "error_code": "not_found",
                    "error": format!("No verification result found for key `{key}`"),
                }),
            }),
        }
    }

    async fn invoke_list(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let store = self.inner.store.clone();
        let items = store.list().await;
        let results: Vec<Value> = items
            .into_iter()
            .map(|(key, result)| {
                let mut v = serde_json::to_value(&result).unwrap_or(json!({}));
                if let Value::Object(ref mut map) = v {
                    map.insert("key".to_string(), json!(key));
                }
                v
            })
            .collect();

        Ok(helpers::ok(
            invocation.id,
            json!({
                "results": results,
                "total": results.len(),
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── VerifierTool definition is correct ────────────────────────────────

    #[test]
    fn verifier_tool_definition_has_correct_name() {
        let tool = VerifierTool::new(PathBuf::from("/tmp"));
        let def = tool.definition();
        assert_eq!(def.name, "verifier");
        assert_eq!(def.kind, ToolKind::Command);
    }

    #[test]
    fn verifier_tool_definition_has_required_fields() {
        let tool = VerifierTool::new(PathBuf::from("/tmp"));
        let def = tool.definition();
        let required = def.input_schema["required"]
            .as_array()
            .expect("required array");
        assert!(required.contains(&json!("action")));
    }

    #[test]
    fn verifier_tool_definition_has_action_enum() {
        let tool = VerifierTool::new(PathBuf::from("/tmp"));
        let def = tool.definition();
        let enum_vals = def.input_schema["properties"]["action"]["enum"]
            .as_array()
            .expect("enum array");
        assert!(enum_vals.contains(&json!("run")));
        assert!(enum_vals.contains(&json!("status")));
        assert!(enum_vals.contains(&json!("list")));
    }

    #[test]
    fn verifier_tool_definition_has_verifier_enum() {
        let tool = VerifierTool::new(PathBuf::from("/tmp"));
        let def = tool.definition();
        let enum_vals = def.input_schema["properties"]["verifier"]["enum"]
            .as_array()
            .expect("enum array");
        assert!(enum_vals.contains(&json!("build")));
        assert!(enum_vals.contains(&json!("test")));
        assert!(enum_vals.contains(&json!("typecheck")));
        assert!(enum_vals.contains(&json!("lint")));
        assert!(enum_vals.contains(&json!("command")));
    }

    // ── invoke_run with echo ──────────────────────────────────────────────

    #[tokio::test]
    async fn verifier_tool_run_echo() {
        let dir = tempfile::tempdir().unwrap();
        let tool = VerifierTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "test".to_string(),
                tool_name: "verifier".to_string(),
                input: json!({
                    "action": "run",
                    "verifier": "command",
                    "command": "echo hello world",
                }),
            })
            .await
            .unwrap();
        assert!(result.ok, "echo should pass: {:?}", result.output);
        assert_eq!(result.output["status"], "pass");
        assert_eq!(result.output["exit_code"], 0);
    }

    // ── invoke_list ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn verifier_tool_list() {
        let dir = tempfile::tempdir().unwrap();
        let tool = VerifierTool::new(dir.path().to_path_buf());
        // Run a verifier first.
        tool.invoke(ToolInvocation {
            id: "run1".to_string(),
            tool_name: "verifier".to_string(),
            input: json!({
                "action": "run",
                "verifier": "command",
                "command": "true",
            }),
        })
        .await
        .unwrap();

        let result = tool
            .invoke(ToolInvocation {
                id: "list".to_string(),
                tool_name: "verifier".to_string(),
                input: json!({ "action": "list" }),
            })
            .await
            .unwrap();
        assert!(result.ok);
        let results = result.output["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["key"], "command:true");
    }

    // ── invoke_status with key ────────────────────────────────────────────

    #[tokio::test]
    async fn verifier_tool_status() {
        let dir = tempfile::tempdir().unwrap();
        let tool = VerifierTool::new(dir.path().to_path_buf());
        tool.invoke(ToolInvocation {
            id: "run".to_string(),
            tool_name: "verifier".to_string(),
            input: json!({
                "action": "run",
                "verifier": "build",
                "command": "echo built",
                "key": "my-build",
            }),
        })
        .await
        .unwrap();

        let result = tool
            .invoke(ToolInvocation {
                id: "status".to_string(),
                tool_name: "verifier".to_string(),
                input: json!({ "action": "status", "key": "my-build" }),
            })
            .await
            .unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "pass");
    }

    #[tokio::test]
    async fn verifier_tool_status_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let tool = VerifierTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "status".to_string(),
                tool_name: "verifier".to_string(),
                input: json!({ "action": "status", "key": "nonexistent" }),
            })
            .await
            .unwrap();
        assert!(!result.ok);
        assert_eq!(result.output["error_code"], "not_found");
    }

    // ── invoke with invalid action ────────────────────────────────────────

    #[tokio::test]
    async fn verifier_tool_invalid_action() {
        let dir = tempfile::tempdir().unwrap();
        let tool = VerifierTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "bad".to_string(),
                tool_name: "verifier".to_string(),
                input: json!({ "action": "bake" }),
            })
            .await
            .unwrap();
        assert!(!result.ok);
        assert_eq!(result.output["error_code"], "invalid_action");
    }
}
