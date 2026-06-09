use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

use crate::tool::{ToolDefinition, ToolKind, ToolResult};

pub(super) const SPECIALIZED_SCHEMA_VERSION: u32 = 1;

pub(super) fn definition(
    name: &str,
    description: &str,
    kind: ToolKind,
    input_schema: Value,
) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        kind,
        input_schema,
    }
}

pub(super) fn json_schema(properties: &[(&str, &str)], required: &[&str]) -> Value {
    let properties = properties
        .iter()
        .map(|(name, description)| {
            let is_integer = name.starts_with("max_")
                || *name == "timeout_ms"
                || name.ends_with("_line")
                || name.starts_with("start_")
                || name.starts_with("end_");
            (
                (*name).to_string(),
                json!({ "type": if is_integer { "integer" } else { "string" }, "description": description }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

pub(super) fn bash_json_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "Shell command to run. Required when starting a new command."
            },
            "description": {
                "type": "string",
                "description": "Short reason for running this command, used for approval and trace context."
            },
            "timeout_ms": {
                "type": "integer",
                "description": "Maximum command lifetime in milliseconds, capped internally."
            },
            "wait_ms": {
                "type": "integer",
                "description": "How long to wait for foreground observation before returning running status for background tasks."
            },
            "background": {
                "type": "boolean",
                "description": "When true, keep the command running after wait_ms and return a task_id for polling."
            },
            "task_id": {
                "type": "string",
                "description": "Background task id returned by an earlier bash call."
            },
            "action": {
                "type": "string",
                "enum": ["poll", "cancel", "list"],
                "description": "Use poll/cancel with task_id, or list to show background tasks."
            }
        },
        "anyOf": [
            { "required": ["command"] },
            { "required": ["task_id"] },
            { "properties": { "action": { "const": "list" } }, "required": ["action"] }
        ],
        "additionalProperties": false,
    })
}

pub(super) fn required_string<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("missing required string `{key}`"))
}

pub(super) fn optional_string(input: &Value, key: &str) -> Option<String> {
    input.get(key).and_then(Value::as_str).map(str::to_string)
}

pub(super) fn optional_u64(input: &Value, key: &str) -> Option<u64> {
    input.get(key).and_then(Value::as_u64)
}

pub(super) fn optional_bool(input: &Value, key: &str) -> Option<bool> {
    input.get(key).and_then(Value::as_bool)
}

pub(super) fn ok(invocation_id: String, output: Value) -> ToolResult {
    ToolResult {
        invocation_id,
        ok: true,
        output,
    }
}

pub(super) fn versioned<T: Serialize>(output: T) -> Value {
    let mut value = serde_json::to_value(output).expect("tool output serializes");
    if let Value::Object(ref mut object) = value {
        object.insert(
            "schema_version".to_string(),
            json!(SPECIALIZED_SCHEMA_VERSION),
        );
    }
    value
}

pub(super) fn tool_error(
    error_code: &str,
    message: impl Into<String>,
    recoverable: bool,
    hint: Option<&str>,
    stderr: Option<String>,
) -> Value {
    json!({
        "schema_version": SPECIALIZED_SCHEMA_VERSION,
        "status": "error",
        "error_code": error_code,
        "message": message.into(),
        "recoverable": recoverable,
        "hint": hint,
        "stderr": stderr,
    })
}

pub(crate) fn truncate_tool_result(mut result: ToolResult) -> ToolResult {
    result.output = truncate_json(result.output, 128 * 1024);
    result
}

fn truncate_json(value: Value, max_bytes: usize) -> Value {
    let serialized = value.to_string();
    if serialized.len() <= max_bytes {
        value
    } else {
        json!({ "truncated": true, "content": truncate_string(serialized, max_bytes) })
    }
}

pub(super) fn truncate_string(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    value.truncate(max_bytes);
    while !value.is_char_boundary(value.len()) {
        value.pop();
    }
    value.push_str("\n<truncated>");
    value
}

pub(super) fn grep_path(
    root: &Path,
    pattern: &str,
    max_results: usize,
    matches: &mut Vec<Value>,
) -> Result<()> {
    if matches.len() >= max_results || should_skip(root) {
        return Ok(());
    }
    if root.is_file() {
        if let Ok(content) = fs::read_to_string(root) {
            for (index, line) in content.lines().enumerate() {
                if line.contains(pattern) {
                    matches.push(json!({
                        "path": root.display().to_string(),
                        "line": index + 1,
                        "text": line,
                    }));
                    if matches.len() >= max_results {
                        break;
                    }
                }
            }
        }
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("failed to list {}", root.display()))? {
        if matches.len() >= max_results {
            break;
        }
        grep_path(&entry?.path(), pattern, max_results, matches)?;
    }
    Ok(())
}

fn should_skip(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                ".git"
                    | "target"
                    | "node_modules"
                    | ".cache"
                    | ".venv"
                    | "venv"
                    | "__pycache__"
                    | ".tox"
                    | "vendor"
                    | "dist"
                    | "build"
                    | "out"
                    | ".next"
                    | ".nuxt"
                    | ".output"
                    | ".parcel-cache"
                    | ".turbo"
                    | ".eslintcache"
                    | "coverage"
                    | ".nyc_output"
                    | "htmlcov"
                    | ".idea"
                    | ".vscode"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── required_string ────────────────────────────────────────────────────

    #[test]
    fn required_string_returns_value() {
        let input = json!({ "key": "hello" });
        assert_eq!(required_string(&input, "key").unwrap(), "hello");
    }

    #[test]
    fn required_string_fails_for_missing() {
        let input = json!({});
        assert!(required_string(&input, "key").is_err());
    }

    #[test]
    fn required_string_fails_for_empty() {
        let input = json!({ "key": "" });
        assert!(required_string(&input, "key").is_err());
    }

    #[test]
    fn required_string_fails_for_non_string() {
        let input = json!({ "key": 42 });
        assert!(required_string(&input, "key").is_err());
    }

    // ── optional_string ────────────────────────────────────────────────────

    #[test]
    fn optional_string_returns_some() {
        let input = json!({ "key": "value" });
        assert_eq!(optional_string(&input, "key"), Some("value".to_string()));
    }

    #[test]
    fn optional_string_returns_none_for_missing() {
        let input = json!({});
        assert_eq!(optional_string(&input, "key"), None);
    }

    #[test]
    fn optional_string_returns_empty_string() {
        let input = json!({ "key": "" });
        assert_eq!(optional_string(&input, "key"), Some("".to_string()));
    }

    #[test]
    fn optional_string_returns_none_for_non_string() {
        let input = json!({ "key": 42 });
        assert_eq!(optional_string(&input, "key"), None);
    }

    // ── optional_bool ──────────────────────────────────────────────────────

    #[test]
    fn optional_bool_returns_some() {
        let input = json!({ "flag": true });
        assert_eq!(optional_bool(&input, "flag"), Some(true));
    }

    #[test]
    fn optional_bool_returns_none_for_missing() {
        let input = json!({});
        assert_eq!(optional_bool(&input, "flag"), None);
    }

    #[test]
    fn optional_bool_returns_none_for_non_bool() {
        let input = json!({ "flag": "yes" });
        assert_eq!(optional_bool(&input, "flag"), None);
    }

    // ── optional_u64 ───────────────────────────────────────────────────────

    #[test]
    fn optional_u64_returns_some() {
        let input = json!({ "count": 42 });
        assert_eq!(optional_u64(&input, "count"), Some(42));
    }

    #[test]
    fn optional_u64_returns_none_for_missing() {
        let input = json!({});
        assert_eq!(optional_u64(&input, "count"), None);
    }

    #[test]
    fn optional_u64_returns_none_for_negative() {
        let input = json!({ "count": -1 });
        assert_eq!(optional_u64(&input, "count"), None);
    }

    #[test]
    fn optional_u64_returns_none_for_string() {
        let input = json!({ "count": "42" });
        assert_eq!(optional_u64(&input, "count"), None);
    }

    // ── truncate_string ────────────────────────────────────────────────────

    #[test]
    fn truncate_string_no_change_when_under_limit() {
        let s = "hello".to_string();
        assert_eq!(truncate_string(s, 100), "hello");
    }

    #[test]
    fn truncate_string_exact_limit_unchanged() {
        let s = "hello".to_string();
        assert_eq!(truncate_string(s, 5), "hello");
    }

    #[test]
    fn truncate_string_truncates_and_appends_marker() {
        let s = "hello world".to_string();
        let result = truncate_string(s, 5);
        assert!(result.starts_with("hello"));
        assert!(result.ends_with("<truncated>"));
    }

    #[test]
    fn truncate_string_respects_char_boundaries() {
        // 2-byte UTF-8 char: 'é' (2 bytes)
        let s = "aébéc".to_string(); // 1+2+1+2+1 = 7 bytes, 5 chars
        let result = truncate_string(s, 4);
        // Truncates to 4 bytes, pops until char boundary, then appends "\n<truncated>"
        assert!(result.ends_with("<truncated>"));
        // Should not panic on char boundary
    }

    #[test]
    fn truncate_string_empty_input() {
        assert_eq!(truncate_string(String::new(), 100), "");
    }

    // ── definition ─────────────────────────────────────────────────────────

    #[test]
    fn definition_creates_correct_structure() {
        let def = definition(
            "my_tool",
            "does things",
            ToolKind::Read,
            json!({"type": "object"}),
        );
        assert_eq!(def.name, "my_tool");
        assert_eq!(def.description, "does things");
        assert!(matches!(def.kind, ToolKind::Read));
        assert_eq!(def.input_schema["type"], "object");
    }

    // ── ok ─────────────────────────────────────────────────────────────────

    #[test]
    fn ok_creates_success_result() {
        let result = ok("inv-1".to_string(), json!({"data": 42}));
        assert!(result.ok);
        assert_eq!(result.invocation_id, "inv-1");
        assert_eq!(result.output["data"], 42);
    }

    #[test]
    fn versioned_adds_schema_version() {
        let output = versioned(json!({"status": "ok"}));
        assert_eq!(output["schema_version"], SPECIALIZED_SCHEMA_VERSION);
        assert_eq!(output["status"], "ok");
    }

    #[test]
    fn tool_error_uses_structured_contract() {
        let output = tool_error(
            "missing_input",
            "Missing input",
            true,
            Some("Provide input."),
            Some("stderr".to_string()),
        );
        assert_eq!(output["schema_version"], SPECIALIZED_SCHEMA_VERSION);
        assert_eq!(output["status"], "error");
        assert_eq!(output["error_code"], "missing_input");
        assert_eq!(output["message"], "Missing input");
        assert_eq!(output["recoverable"], true);
        assert_eq!(output["hint"], "Provide input.");
        assert_eq!(output["stderr"], "stderr");
    }

    // ── truncate_tool_result ───────────────────────────────────────────────

    #[test]
    fn truncate_tool_result_preserves_small_output() {
        let result = ToolResult {
            invocation_id: "inv".to_string(),
            ok: true,
            output: json!({"small": "data"}),
        };
        let truncated = truncate_tool_result(result);
        assert_eq!(truncated.output["small"], "data");
    }

    #[test]
    fn truncate_tool_result_wraps_large_output() {
        let large_string = "x".repeat(200 * 1024);
        let result = ToolResult {
            invocation_id: "inv".to_string(),
            ok: true,
            output: json!({"data": large_string}),
        };
        let truncated = truncate_tool_result(result);
        assert_eq!(truncated.output["truncated"], true);
        assert!(truncated.output["content"].as_str().is_some());
    }

    // ── Mutation-killing: should_skip ─────────────────────────────────────

    #[test]
    fn should_skip_returns_false_for_normal_path() {
        assert!(!should_skip(Path::new("/project/src/main.rs")));
    }

    #[test]
    fn should_skip_returns_false_for_root() {
        // Path with no file_name (root "/")
        assert!(!should_skip(Path::new("/")));
    }

    #[test]
    fn should_skip_returns_true_for_each_special_dir() {
        for name in [
            ".git",
            "target",
            "node_modules",
            ".cache",
            ".venv",
            "__pycache__",
        ] {
            assert!(
                should_skip(Path::new(format!("/project/{name}").as_str())),
                "should_skip should return true for {name}"
            );
        }
    }

    // ── Mutation-killing: grep_path max_results boundary ──────────────────

    #[test]
    fn grep_path_stops_at_exact_max_results() {
        let tempdir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(tempdir.path().join(format!("f{i}.txt")), "needle").unwrap();
        }
        let mut matches = Vec::new();
        grep_path(tempdir.path(), "needle", 3, &mut matches).unwrap();
        assert_eq!(matches.len(), 3);
    }

    #[test]
    fn grep_path_returns_all_when_under_limit() {
        let tempdir = tempfile::tempdir().unwrap();
        for i in 0..3 {
            std::fs::write(tempdir.path().join(format!("f{i}.txt")), "needle").unwrap();
        }
        let mut matches = Vec::new();
        grep_path(tempdir.path(), "needle", 10, &mut matches).unwrap();
        assert_eq!(matches.len(), 3);
    }

    // ── Mutation-killing: truncate_tool_result boundary ───────────────────

    #[test]
    fn truncate_tool_result_preserves_small_json() {
        let result = ToolResult {
            invocation_id: "inv".to_string(),
            ok: true,
            output: json!({"data": "small"}),
        };
        let truncated = truncate_tool_result(result);
        // Small output should NOT be wrapped
        assert!(truncated.output["truncated"].is_null());
        assert_eq!(truncated.output["data"], "small");
    }

    #[test]
    fn truncate_tool_result_wraps_large_json() {
        let data = "x".repeat(200 * 1024);
        let result = ToolResult {
            invocation_id: "inv".to_string(),
            ok: true,
            output: json!({"data": data}),
        };
        let truncated = truncate_tool_result(result);
        assert_eq!(truncated.output["truncated"], true);
    }
}
