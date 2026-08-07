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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Tool, ToolInvocation, ToolKind};
    use serde_json::json;

    fn make_policy(root: &std::path::Path) -> SecurityPolicy {
        let config = crate::config::SecurityConfig {
            permission_mode: crate::config::PermissionMode::Yolo,
            ..Default::default()
        };
        SecurityPolicy::new(root.to_path_buf(), root.join("data"), config).unwrap()
    }

    fn make_tool(root: &std::path::Path) -> CodeExecTool {
        CodeExecTool::new(make_policy(root))
    }

    fn make_invocation(id: &str, input: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            id: id.to_string(),
            tool_name: "code_exec".to_string(),
            input,
        }
    }

    // ── definition ────────────────────────────────────────────────────────

    #[test]
    fn definition_name_is_code_exec() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let def = tool.definition();
        assert_eq!(def.name, "code_exec");
    }

    #[test]
    fn definition_kind_is_write() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let def = tool.definition();
        assert_eq!(def.kind, ToolKind::Write);
    }

    #[test]
    fn definition_schema_requires_ops() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let def = tool.definition();
        let required = def.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(required.contains(&"ops"));
    }

    #[test]
    fn definition_schema_op_enum_values() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let def = tool.definition();
        let ops = def.input_schema["properties"]["ops"]["items"]["properties"]["op"]["enum"]
            .as_array()
            .unwrap();
        let names: Vec<&str> = ops.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(names.contains(&"repo-read"));
        assert!(names.contains(&"repo-search"));
        assert!(names.contains(&"repo-patch"));
        assert!(names.contains(&"ast-search"));
        assert!(names.contains(&"verify-run"));
        assert!(names.contains(&"trace-note"));
    }

    // ── CodeExecRequest deserialization ───────────────────────────────────

    #[test]
    fn code_exec_request_minimal() {
        let json = json!({"ops": [{"op": "trace-note", "note": "hello"}]});
        let req: CodeExecRequest = serde_json::from_value(json).unwrap();
        assert!(req.cell_id.is_none());
        assert!(req.max_ops.is_none());
        assert!(req.max_output_bytes.is_none());
        assert_eq!(req.ops.len(), 1);
    }

    #[test]
    fn code_exec_request_with_all_fields() {
        let json = json!({
            "cell_id": "cell-1",
            "max_ops": 50,
            "max_output_bytes": 4096,
            "ops": [{"op": "trace-note", "note": "test"}]
        });
        let req: CodeExecRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.cell_id.as_deref(), Some("cell-1"));
        assert_eq!(req.max_ops, Some(50));
        assert_eq!(req.max_output_bytes, Some(4096));
    }

    #[test]
    fn code_exec_request_empty_ops() {
        let json = json!({"ops": []});
        let req: CodeExecRequest = serde_json::from_value(json).unwrap();
        assert!(req.ops.is_empty());
    }

    #[test]
    fn code_exec_request_missing_ops_fails() {
        let json = json!({"cell_id": "x"});
        let result: Result<CodeExecRequest, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }

    // ── CodeExecOp deserialization ────────────────────────────────────────

    #[test]
    fn code_exec_op_repo_read() {
        let json = json!({"op": "repo-read", "path": "src/main.rs"});
        let op: CodeExecOp = serde_json::from_value(json).unwrap();
        assert!(matches!(op, CodeExecOp::RepoRead { ref path, .. } if path == "src/main.rs"));
    }

    #[test]
    fn code_exec_op_repo_read_with_lines() {
        let json =
            json!({"op": "repo-read", "path": "src/main.rs", "start_line": 10, "end_line": 20});
        let op: CodeExecOp = serde_json::from_value(json).unwrap();
        if let CodeExecOp::RepoRead {
            path,
            start_line,
            end_line,
        } = op
        {
            assert_eq!(path, "src/main.rs");
            assert_eq!(start_line, Some(10));
            assert_eq!(end_line, Some(20));
        } else {
            panic!("expected RepoRead");
        }
    }

    #[test]
    fn code_exec_op_repo_search() {
        let json = json!({"op": "repo-search", "pattern": "fn main"});
        let op: CodeExecOp = serde_json::from_value(json).unwrap();
        if let CodeExecOp::RepoSearch {
            pattern,
            path,
            max_results,
        } = op
        {
            assert_eq!(pattern, "fn main");
            assert_eq!(path, "."); // default
            assert!(max_results.is_none());
        } else {
            panic!("expected RepoSearch");
        }
    }

    #[test]
    fn code_exec_op_repo_search_with_path_and_max() {
        let json = json!({"op": "repo-search", "pattern": "fn", "path": "src/", "max_results": 10});
        let op: CodeExecOp = serde_json::from_value(json).unwrap();
        if let CodeExecOp::RepoSearch {
            pattern,
            path,
            max_results,
        } = op
        {
            assert_eq!(pattern, "fn");
            assert_eq!(path, "src/");
            assert_eq!(max_results, Some(10));
        } else {
            panic!("expected RepoSearch");
        }
    }

    #[test]
    fn code_exec_op_repo_patch() {
        let json = json!({"op": "repo-patch", "patch": "*** Begin Patch\n*** End Patch"});
        let op: CodeExecOp = serde_json::from_value(json).unwrap();
        assert!(matches!(op, CodeExecOp::RepoPatch { .. }));
    }

    #[test]
    fn code_exec_op_ast_search() {
        let json = json!({"op": "ast-search", "query": "main"});
        let op: CodeExecOp = serde_json::from_value(json).unwrap();
        if let CodeExecOp::AstSearch {
            query,
            kind,
            max_results,
        } = op
        {
            assert_eq!(query, "main");
            assert!(kind.is_none());
            assert!(max_results.is_none());
        } else {
            panic!("expected AstSearch");
        }
    }

    #[test]
    fn code_exec_op_ast_search_with_kind_and_max() {
        let json =
            json!({"op": "ast-search", "query": "main", "kind": "function", "max_results": 5});
        let op: CodeExecOp = serde_json::from_value(json).unwrap();
        if let CodeExecOp::AstSearch {
            query,
            kind,
            max_results,
        } = op
        {
            assert_eq!(query, "main");
            assert_eq!(kind.as_deref(), Some("function"));
            assert_eq!(max_results, Some(5));
        } else {
            panic!("expected AstSearch");
        }
    }

    #[test]
    fn code_exec_op_verify_run() {
        let json = json!({"op": "verify-run", "command": "echo hello"});
        let op: CodeExecOp = serde_json::from_value(json).unwrap();
        if let CodeExecOp::VerifyRun {
            command,
            verifier,
            timeout_ms,
        } = op
        {
            assert_eq!(command, "echo hello");
            assert_eq!(verifier, "command"); // default
            assert!(timeout_ms.is_none());
        } else {
            panic!("expected VerifyRun");
        }
    }

    #[test]
    fn code_exec_op_verify_run_with_timeout() {
        let json = json!({"op": "verify-run", "command": "test", "timeout_ms": 5000});
        let op: CodeExecOp = serde_json::from_value(json).unwrap();
        if let CodeExecOp::VerifyRun {
            command,
            timeout_ms,
            ..
        } = op
        {
            assert_eq!(command, "test");
            assert_eq!(timeout_ms, Some(5000));
        } else {
            panic!("expected VerifyRun");
        }
    }

    #[test]
    fn code_exec_op_trace_note() {
        let json = json!({"op": "trace-note", "note": "a note"});
        let op: CodeExecOp = serde_json::from_value(json).unwrap();
        assert!(matches!(op, CodeExecOp::TraceNote { ref note } if note == "a note"));
    }

    #[test]
    fn code_exec_op_unknown_op_fails() {
        let json = json!({"op": "unknown-op"});
        let result: Result<CodeExecOp, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }

    // ── nested_invocation ─────────────────────────────────────────────────

    #[test]
    fn nested_invocation_repo_read() {
        let op = CodeExecOp::RepoRead {
            path: "src/main.rs".into(),
            start_line: Some(1),
            end_line: Some(10),
        };
        let inv = nested_invocation(1, &op).unwrap();
        assert_eq!(inv.tool_name, "read");
        assert_eq!(inv.input["path"], "src/main.rs");
        assert_eq!(inv.input["start_line"], 1);
        assert_eq!(inv.input["end_line"], 10);
        assert_eq!(inv.id, "code-exec-1");
    }

    #[test]
    fn nested_invocation_repo_read_no_lines() {
        let op = CodeExecOp::RepoRead {
            path: "src/lib.rs".into(),
            start_line: None,
            end_line: None,
        };
        let inv = nested_invocation(1, &op).unwrap();
        assert_eq!(inv.tool_name, "read");
        assert_eq!(inv.input["path"], "src/lib.rs");
        assert!(inv.input.get("start_line").is_none());
        assert!(inv.input.get("end_line").is_none());
    }

    #[test]
    fn nested_invocation_repo_search() {
        let op = CodeExecOp::RepoSearch {
            pattern: "fn".into(),
            path: "src/".into(),
            max_results: Some(5),
        };
        let inv = nested_invocation(2, &op).unwrap();
        assert_eq!(inv.tool_name, "search");
        assert_eq!(inv.input["pattern"], "fn");
        assert_eq!(inv.input["path"], "src/");
        assert_eq!(inv.input["max_results"], 5);
        assert_eq!(inv.id, "code-exec-2");
    }

    #[test]
    fn nested_invocation_repo_search_no_max() {
        let op = CodeExecOp::RepoSearch {
            pattern: "fn".into(),
            path: ".".into(),
            max_results: None,
        };
        let inv = nested_invocation(1, &op).unwrap();
        assert_eq!(inv.tool_name, "search");
        assert!(inv.input.get("max_results").is_none());
    }

    #[test]
    fn nested_invocation_repo_patch() {
        let op = CodeExecOp::RepoPatch {
            patch: "*** Begin Patch\n*** End Patch".into(),
        };
        let inv = nested_invocation(3, &op).unwrap();
        assert_eq!(inv.tool_name, "apply_patch");
        assert_eq!(inv.input["patch"], "*** Begin Patch\n*** End Patch");
    }

    #[test]
    fn nested_invocation_ast_search() {
        let op = CodeExecOp::AstSearch {
            query: "main".into(),
            kind: Some("function".into()),
            max_results: Some(10),
        };
        let inv = nested_invocation(4, &op).unwrap();
        assert_eq!(inv.tool_name, "ast_search");
        assert_eq!(inv.input["query"], "main");
        assert_eq!(inv.input["kind"], "function");
        assert_eq!(inv.input["max_results"], 10);
    }

    #[test]
    fn nested_invocation_ast_search_no_optional_fields() {
        let op = CodeExecOp::AstSearch {
            query: "test".into(),
            kind: None,
            max_results: None,
        };
        let inv = nested_invocation(1, &op).unwrap();
        assert_eq!(inv.tool_name, "ast_search");
        assert_eq!(inv.input["query"], "test");
        assert!(inv.input.get("kind").is_none());
        assert!(inv.input.get("max_results").is_none());
    }

    #[test]
    fn nested_invocation_verify_run() {
        let op = CodeExecOp::VerifyRun {
            command: "echo test".into(),
            verifier: "command".into(),
            timeout_ms: Some(1000),
        };
        let inv = nested_invocation(5, &op).unwrap();
        assert_eq!(inv.tool_name, "bash");
        assert_eq!(inv.input["command"], "echo test");
        assert_eq!(inv.input["timeout_ms"], 1000);
    }

    #[test]
    fn nested_invocation_verify_run_no_timeout() {
        let op = CodeExecOp::VerifyRun {
            command: "echo test".into(),
            verifier: "command".into(),
            timeout_ms: None,
        };
        let inv = nested_invocation(1, &op).unwrap();
        assert_eq!(inv.tool_name, "bash");
        assert!(inv.input.get("timeout_ms").is_none());
    }

    #[test]
    fn nested_invocation_trace_note_errors() {
        let op = CodeExecOp::TraceNote { note: "x".into() };
        let result = nested_invocation(1, &op);
        assert!(result.is_err());
    }

    // ── op_name ───────────────────────────────────────────────────────────

    #[test]
    fn op_name_all_variants() {
        assert_eq!(
            op_name(&CodeExecOp::RepoRead {
                path: "x".into(),
                start_line: None,
                end_line: None,
            }),
            "repo-read"
        );
        assert_eq!(
            op_name(&CodeExecOp::RepoSearch {
                pattern: "x".into(),
                path: ".".into(),
                max_results: None,
            }),
            "repo-search"
        );
        assert_eq!(
            op_name(&CodeExecOp::RepoPatch { patch: "x".into() }),
            "repo-patch"
        );
        assert_eq!(
            op_name(&CodeExecOp::AstSearch {
                query: "x".into(),
                kind: None,
                max_results: None,
            }),
            "ast-search"
        );
        assert_eq!(
            op_name(&CodeExecOp::VerifyRun {
                command: "x".into(),
                verifier: "command".into(),
                timeout_ms: None,
            }),
            "verify-run"
        );
        assert_eq!(
            op_name(&CodeExecOp::TraceNote { note: "x".into() }),
            "trace-note"
        );
    }

    // ── truncate_value ────────────────────────────────────────────────────

    #[test]
    fn truncate_value_small_no_truncation() {
        let value = json!({"key": "value"});
        let (result, truncated) = truncate_value(value.clone(), 1024);
        assert!(!truncated);
        assert_eq!(result, value);
    }

    #[test]
    fn truncate_value_large_truncates() {
        let large = "x".repeat(2000);
        let value = json!({"content": large});
        let (result, truncated) = truncate_value(value, 100);
        assert!(truncated);
        assert_eq!(result["truncated"], true);
        assert!(
            result["content"]
                .as_str()
                .is_some_and(|c| c.contains("<truncated>")),
            "should contain truncation marker: {result:?}"
        );
    }

    #[test]
    fn truncate_value_exact_boundary_no_truncation() {
        let value = json!({"key": "val"});
        let serialized = value.to_string();
        let (result, truncated) = truncate_value(value.clone(), serialized.len());
        assert!(!truncated);
        assert_eq!(result, value);
    }

    #[test]
    fn truncate_value_one_byte_over_truncates() {
        let value = json!({"key": "value"});
        let serialized = value.to_string();
        let (result, truncated) = truncate_value(value, serialized.len() - 1);
        assert!(truncated);
        assert_eq!(result["truncated"], true);
    }

    #[test]
    fn truncate_value_empty_object() {
        let value = json!({});
        let (result, truncated) = truncate_value(value.clone(), 1024);
        assert!(!truncated);
        assert_eq!(result, value);
    }

    #[test]
    fn truncate_value_null() {
        let value = serde_json::Value::Null;
        let (result, truncated) = truncate_value(value.clone(), 1024);
        assert!(!truncated);
        assert_eq!(result, value);
    }

    // ── default functions ─────────────────────────────────────────────────

    #[test]
    fn default_dot_returns_dot() {
        assert_eq!(default_dot(), ".");
    }

    #[test]
    fn default_command_verifier_returns_command() {
        assert_eq!(default_command_verifier(), "command");
    }

    // ── invoke integration ────────────────────────────────────────────────

    #[tokio::test]
    async fn invoke_with_empty_ops_returns_passed() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation("e1", json!({"ops": []}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "passed");
        assert_eq!(result.output["ops_executed"], 0);
    }

    #[tokio::test]
    async fn invoke_with_trace_note_only_returns_passed() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation(
            "e2",
            json!({"ops": [{"op": "trace-note", "note": "hello"}]}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "passed");
        assert_eq!(result.output["ops_executed"], 1);
        assert_eq!(result.output["results"][0]["op"], "trace-note");
        assert_eq!(result.output["results"][0]["ok"], true);
        assert_eq!(result.output["results"][0]["output"]["note"], "hello");
    }

    #[tokio::test]
    async fn invoke_with_multiple_trace_notes() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation(
            "e3",
            json!({
                "ops": [
                    {"op": "trace-note", "note": "first"},
                    {"op": "trace-note", "note": "second"},
                    {"op": "trace-note", "note": "third"},
                ]
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "passed");
        assert_eq!(result.output["ops_executed"], 3);
        assert_eq!(result.output["results"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn invoke_with_invalid_request_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation("e4", json!({"cell_id": "x"})); // missing ops
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "missing ops should return Err");
    }

    #[tokio::test]
    async fn invoke_with_too_many_ops_returns_err() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let ops: Vec<_> = (0..200)
            .map(|i| json!({"op": "trace-note", "note": format!("note {i}")}))
            .collect();
        let inv = make_invocation("e5", json!({"ops": ops}));
        let result = tool.invoke(inv).await;
        assert!(result.is_err(), "too many ops should return Err");
    }

    #[tokio::test]
    async fn invoke_with_max_ops_override() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        // 200 ops with max_ops=500 should succeed.
        let ops: Vec<_> = (0..200)
            .map(|i| json!({"op": "trace-note", "note": format!("note {i}")}))
            .collect();
        let inv = make_invocation("e6", json!({"ops": ops, "max_ops": 500}));
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "passed");
    }

    #[tokio::test]
    async fn invoke_with_repo_read_on_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("file.txt"), "hello world").unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation(
            "e7",
            json!({"ops": [{"op": "repo-read", "path": "file.txt"}]}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "passed");
        assert_eq!(result.output["results"][0]["op"], "repo-read");
        assert_eq!(result.output["results"][0]["tool"], "read");
    }

    #[tokio::test]
    async fn invoke_with_repo_read_nonexistent_fails_op() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation(
            "e8",
            json!({"ops": [{"op": "repo-read", "path": "nonexistent.rs"}]}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "failed");
        assert_eq!(result.output["failed_op"], 0);
        assert_eq!(result.output["ops_executed"], 1);
    }

    #[tokio::test]
    async fn invoke_with_cell_id_preserved_in_output() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation(
            "e9",
            json!({"cell_id": "my-cell", "ops": [{"op": "trace-note", "note": "x"}]}),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert_eq!(result.output["cell_id"], "my-cell");
    }

    #[tokio::test]
    async fn invoke_with_trace_note_then_failed_op() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation(
            "e10",
            json!({
                "ops": [
                    {"op": "trace-note", "note": "ok"},
                    {"op": "repo-read", "path": "nonexistent.rs"},
                    {"op": "trace-note", "note": "should not run"},
                ]
            }),
        );
        let result = tool.invoke(inv).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.output["status"], "failed");
        assert_eq!(result.output["failed_op"], 1);
        assert_eq!(result.output["ops_executed"], 2);
        // Third op should not have run.
        assert_eq!(result.output["results"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn invoke_with_schema_version() {
        let temp = tempfile::tempdir().unwrap();
        let tool = make_tool(temp.path());
        let inv = make_invocation("e11", json!({"ops": [{"op": "trace-note", "note": "x"}]}));
        let result = tool.invoke(inv).await.unwrap();
        assert_eq!(
            result.output["schema_version"],
            helpers::SPECIALIZED_SCHEMA_VERSION
        );
    }
}
