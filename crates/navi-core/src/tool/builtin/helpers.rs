use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

use crate::tool::{ToolDefinition, ToolKind, ToolResult};

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

pub(super) fn collect_files(
    root: &Path,
    filter: Option<&str>,
    max_results: usize,
    files: &mut Vec<String>,
) -> Result<()> {
    if files.len() >= max_results || should_skip(root) {
        return Ok(());
    }
    if root.is_file() {
        let display = root.display().to_string();
        if filter.is_none_or(|filter| display.contains(filter)) {
            files.push(display);
        }
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("failed to list {}", root.display()))? {
        if files.len() >= max_results {
            break;
        }
        collect_files(&entry?.path(), filter, max_results, files)?;
    }
    Ok(())
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
                ".git" | "target" | "node_modules" | ".cache" | ".venv" | "__pycache__"
            )
        })
}
