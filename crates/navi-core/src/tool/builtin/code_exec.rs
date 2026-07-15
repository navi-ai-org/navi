use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::helpers;
use crate::security::SecurityPolicy;
use crate::tool::{Tool, ToolDefinition, ToolExecutor, ToolInvocation, ToolKind, ToolResult};

const DEFAULT_MAX_OPS: usize = 100;
const MAX_OPS: usize = 1_000;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 128 * 1024;
const MAX_OUTPUT_BYTES: usize = 512 * 1024;

pub(crate) struct CodeExecTool {
    policy: SecurityPolicy,
}

impl CodeExecTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CodeExecRequest {
    #[serde(default)]
    cell_id: Option<String>,
    #[serde(default)]
    max_ops: Option<usize>,
    #[serde(default)]
    max_output_bytes: Option<usize>,
    ops: Vec<CodeExecOp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
enum CodeExecOp {
    RepoRead {
        path: String,
        #[serde(default)]
        start_line: Option<u64>,
        #[serde(default)]
        end_line: Option<u64>,
    },
    RepoSearch {
        pattern: String,
        #[serde(default = "default_dot")]
        path: String,
        #[serde(default)]
        max_results: Option<u64>,
    },
    RepoPatch {
        patch: String,
    },
    AstSearch {
        query: String,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        max_results: Option<u64>,
    },
    VerifyRun {
        command: String,
        #[serde(default = "default_command_verifier")]
        verifier: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    TraceNote {
        note: String,
    },
}

#[async_trait]
impl Tool for CodeExecTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "code_exec",
            "Execute a typed code-mode plan with controlled nested tools. Supported ops: repo-read, repo-search, repo-patch, ast-search, verify-run (via bash), trace-note.",
            ToolKind::Write,
            json!({
                "type": "object",
                "properties": {
                    "cell_id": { "type": "string" },
                    "max_ops": { "type": "integer" },
                    "max_output_bytes": { "type": "integer" },
                    "ops": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "op": {
                                    "type": "string",
                                    "enum": ["repo-read", "repo-search", "repo-patch", "ast-search", "verify-run", "trace-note"]
                                },
                                "path": { "type": "string" },
                                "start_line": { "type": "integer" },
                                "end_line": { "type": "integer" },
                                "pattern": { "type": "string" },
                                "patch": { "type": "string" },
                                "query": { "type": "string" },
                                "kind": { "type": "string" },
                                "command": { "type": "string" },
                                "verifier": { "type": "string" },
                                "timeout_ms": { "type": "integer" },
                                "max_results": { "type": "integer" },
                                "note": { "type": "string" }
                            },
                            "required": ["op"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["ops"],
                "additionalProperties": false
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let request: CodeExecRequest = serde_json::from_value(invocation.input.clone())
            .context("invalid code_exec request")?;
        let max_ops = request.max_ops.unwrap_or(DEFAULT_MAX_OPS).clamp(1, MAX_OPS);
        if request.ops.len() > max_ops {
            bail!(
                "code_exec requested {} ops but max_ops is {max_ops}",
                request.ops.len()
            );
        }
        let max_output_bytes = request
            .max_output_bytes
            .unwrap_or(DEFAULT_MAX_OUTPUT_BYTES)
            .clamp(1024, MAX_OUTPUT_BYTES);

        let executor = ToolExecutor::new_code_exec_host(self.policy.clone());
        let mut results = Vec::new();
        for (idx, op) in request.ops.iter().enumerate() {
            if let CodeExecOp::TraceNote { note } = op {
                results.push(json!({
                    "index": idx,
                    "op": "trace-note",
                    "tool": null,
                    "ok": true,
                    "output": { "note": note },
                    "output_truncated": false,
                }));
                continue;
            }
            let nested = nested_invocation(idx + 1, op)?;
            let result = executor.invoke(nested.clone()).await;
            let (output, output_truncated) = truncate_value(result.output, max_output_bytes);
            let ok = result.ok;
            results.push(json!({
                "index": idx,
                "op": op_name(op),
                "tool": nested.tool_name,
                "ok": ok,
                "output": output,
                "output_truncated": output_truncated,
            }));
            if !ok {
                return Ok(helpers::ok(
                    invocation.id,
                    json!({
                        "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                        "cell_id": request.cell_id,
                        "status": "failed",
                        "failed_op": idx,
                        "ops_executed": idx + 1,
                        "results": results,
                        "artifact": request,
                    }),
                ));
            }
        }

        Ok(helpers::ok(
            invocation.id,
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "cell_id": request.cell_id,
                "status": "passed",
                "ops_executed": request.ops.len(),
                "results": results,
                "artifact": request,
            }),
        ))
    }
}

fn nested_invocation(index: usize, op: &CodeExecOp) -> Result<ToolInvocation> {
    let (tool_name, input) = match op {
        CodeExecOp::RepoRead {
            path,
            start_line,
            end_line,
        } => {
            let mut input = json!({ "path": path });
            if let Value::Object(ref mut map) = input {
                if let Some(value) = start_line {
                    map.insert("start_line".to_string(), json!(value));
                }
                if let Some(value) = end_line {
                    map.insert("end_line".to_string(), json!(value));
                }
            }
            ("read".to_string(), input)
        }
        CodeExecOp::RepoSearch {
            pattern,
            path,
            max_results,
        } => {
            let mut input = json!({ "pattern": pattern, "path": path });
            if let Some(value) = max_results
                && let Value::Object(ref mut map) = input
            {
                map.insert("max_results".to_string(), json!(value));
            }
            ("search".to_string(), input)
        }
        CodeExecOp::RepoPatch { patch } => ("apply_patch".to_string(), json!({ "patch": patch })),
        CodeExecOp::AstSearch {
            query,
            kind,
            max_results,
        } => {
            let mut input = json!({ "query": query });
            if let Value::Object(ref mut map) = input {
                if let Some(value) = kind {
                    map.insert("kind".to_string(), json!(value));
                }
                if let Some(value) = max_results {
                    map.insert("max_results".to_string(), json!(value));
                }
            }
            ("ast_search".to_string(), input)
        }
        CodeExecOp::VerifyRun {
            command,
            verifier: _,
            timeout_ms,
        } => {
            // `verifier` tool was removed; run verification commands via bash.
            let mut input = json!({ "command": command });
            if let Some(value) = timeout_ms
                && let Value::Object(ref mut map) = input
            {
                map.insert("timeout_ms".to_string(), json!(value));
            }
            ("bash".to_string(), input)
        }
        CodeExecOp::TraceNote { .. } => bail!("trace-note is handled internally"),
    };
    Ok(ToolInvocation {
        id: format!("code-exec-{index}"),
        tool_name,
        input,
    })
}

fn op_name(op: &CodeExecOp) -> &'static str {
    match op {
        CodeExecOp::RepoRead { .. } => "repo-read",
        CodeExecOp::RepoSearch { .. } => "repo-search",
        CodeExecOp::RepoPatch { .. } => "repo-patch",
        CodeExecOp::AstSearch { .. } => "ast-search",
        CodeExecOp::VerifyRun { .. } => "verify-run",
        CodeExecOp::TraceNote { .. } => "trace-note",
    }
}

fn truncate_value(value: Value, max_bytes: usize) -> (Value, bool) {
    let serialized = value.to_string();
    if serialized.len() <= max_bytes {
        return (value, false);
    }
    let mut content = serialized;
    let mut end = max_bytes.min(content.len());
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    content.truncate(end);
    content.push_str("\n<truncated>");
    (json!({ "truncated": true, "content": content }), true)
}

fn default_dot() -> String {
    ".".to_string()
}

fn default_command_verifier() -> String {
    "command".to_string()
}
