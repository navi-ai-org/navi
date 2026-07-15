use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

const DEFAULT_READ_LINE_LIMIT: usize = 400;

pub(crate) struct ReadTool {
    project_root: PathBuf,
    name: &'static str,
}

impl ReadTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            name: "read_file",
        }
    }

    pub(crate) fn alias(project_root: PathBuf, name: &'static str) -> Self {
        Self { project_root, name }
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            self.name,
            "Read a UTF-8 text file from the current project, optionally specifying a line range.",
            ToolKind::Read,
            helpers::json_schema(
                &[
                    ("path", "Project-relative file path to read."),
                    (
                        "start_line",
                        "Line number to start reading from (1-indexed, defaults to 1).",
                    ),
                    (
                        "end_line",
                        "Line number to stop reading at (1-indexed, inclusive, defaults to start_line + 399).",
                    ),
                ],
                &["path"],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let raw_path = helpers::required_string(&invocation.input, "path")?.to_string();
        let path = Path::new(&raw_path);
        let full_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.project_root.join(path)
        };
        let full_path_str = full_path.to_string_lossy().to_string();

        let path_clone = full_path_str.clone();
        let content = tokio::task::spawn_blocking(move || {
            fs::read_to_string(&path_clone).with_context(|| format!("failed to read {path_clone}"))
        })
        .await
        .map_err(|e| anyhow::anyhow!("task join error: {e}"))??;

        if content.is_empty() {
            return Ok(helpers::ok(
                invocation.id,
                json!({
                    "path": raw_path,
                    "content": "",
                    "start_line": 1,
                    "end_line": 0,
                    "total_lines": 0,
                    "truncated": false,
                }),
            ));
        }

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start_line = helpers::optional_u64(&invocation.input, "start_line").unwrap_or(1);
        let end_line = helpers::optional_u64(&invocation.input, "end_line");

        let start_idx = (start_line.max(1) - 1) as usize;
        let end_idx = if start_idx >= total_lines {
            total_lines
        } else {
            match end_line {
                Some(e) => (e as usize).clamp(start_idx, total_lines),
                None => (start_idx + DEFAULT_READ_LINE_LIMIT).min(total_lines),
            }
        };

        let sliced_lines = if start_idx < total_lines {
            &lines[start_idx..end_idx]
        } else {
            &[]
        };

        let mut sliced_content = sliced_lines.join("\n");
        if !sliced_content.is_empty()
            && ((end_idx == total_lines && content.ends_with('\n')) || end_idx < total_lines)
        {
            sliced_content.push('\n');
        }

        let truncated = start_idx > 0 || end_idx < total_lines;

        let (next_start, remaining) = if end_idx < total_lines {
            (
                Some((end_idx + 1) as u64),
                Some((total_lines - end_idx) as u64),
            )
        } else {
            (None, None)
        };

        Ok(helpers::ok(
            invocation.id,
            json!({
                "path": raw_path,
                "content": sliced_content,
                "next_start_line": next_start,
                "remaining_lines": remaining,
                "start_line": start_idx + 1,
                "end_line": end_idx,
                "total_lines": total_lines,
                "truncated": truncated,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{ToolDefinition, ToolKind};
    use serde_json::Value;

    // ── Definition ────────────────────────────────────────────────────────

    #[test]
    fn definition_has_correct_name() {
        let tool = ReadTool::new(PathBuf::from("/tmp"));
        let def: ToolDefinition = tool.definition();
        assert_eq!(def.name, "read_file");
        assert!(def.description.contains("UTF-8 text file"));
        assert!(matches!(def.kind, ToolKind::Read));
    }

    #[test]
    fn definition_has_required_path() {
        let tool = ReadTool::new(PathBuf::from("/tmp"));
        let def = tool.definition();
        let required = def.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "path"));
    }

    #[test]
    fn definition_has_all_properties() {
        let tool = ReadTool::new(PathBuf::from("/tmp"));
        let def = tool.definition();
        let props = def.input_schema["properties"].as_object().unwrap();
        assert!(props.contains_key("path"));
        assert!(props.contains_key("start_line"));
        assert!(props.contains_key("end_line"));
    }

    // ── Invoke: basic reading ─────────────────────────────────────────────

    #[tokio::test]
    async fn invoke_reads_full_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello\nworld\n").unwrap();

        let tool = ReadTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "t1".into(),
                tool_name: "read_file".into(),
                input: json!({ "path": "test.txt" }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["path"], "test.txt");
        assert_eq!(result.output["content"], "hello\nworld\n");
        assert_eq!(result.output["start_line"], 1);
        assert_eq!(result.output["end_line"], 2);
        assert_eq!(result.output["total_lines"], 2);
        assert!(!result.output["truncated"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn invoke_reads_with_line_range() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "one\ntwo\nthree\nfour\nfive\n").unwrap();

        let tool = ReadTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "t2".into(),
                tool_name: "read_file".into(),
                input: json!({ "path": "test.txt", "start_line": 2, "end_line": 4 }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["content"], "two\nthree\nfour\n");
        assert_eq!(result.output["start_line"], 2);
        assert_eq!(result.output["end_line"], 4);
        assert_eq!(result.output["total_lines"], 5);
        assert!(result.output["truncated"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn invoke_handles_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("empty.txt");
        fs::write(&file_path, "").unwrap();

        let tool = ReadTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "t3".into(),
                tool_name: "read_file".into(),
                input: json!({ "path": "empty.txt" }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["content"], "");
        assert_eq!(result.output["total_lines"], 0);
    }

    #[tokio::test]
    async fn invoke_defaults_to_400_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("long.txt");
        let many_lines: String = (1..=500).map(|i| format!("line {i}\n")).collect();
        fs::write(&file_path, &many_lines).unwrap();

        let tool = ReadTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "t4".into(),
                tool_name: "read_file".into(),
                input: json!({ "path": "long.txt" }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["start_line"], 1);
        assert_eq!(result.output["end_line"], 400);
        assert_eq!(result.output["total_lines"], 500);
        assert!(result.output["truncated"].as_bool().unwrap());
        assert_eq!(result.output["next_start_line"], json!(401));
        assert_eq!(result.output["remaining_lines"], json!(100));
    }

    #[tokio::test]
    async fn invoke_resolves_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("abs.txt");
        fs::write(&file_path, "absolute content\n").unwrap();

        let tool = ReadTool::new(PathBuf::from("/nonexistent"));
        let result = tool
            .invoke(ToolInvocation {
                id: "t5".into(),
                tool_name: "read_file".into(),
                input: json!({ "path": file_path.to_string_lossy() }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["content"], "absolute content\n");
    }

    #[tokio::test]
    async fn invoke_returns_next_start_and_remaining_when_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("partial.txt");
        let lines: String = (1..=450).map(|i| format!("line {i}\n")).collect();
        fs::write(&file_path, &lines).unwrap();

        let tool = ReadTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "t6".into(),
                tool_name: "read_file".into(),
                input: json!({ "path": "partial.txt" }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["end_line"], 400);
        assert_eq!(result.output["total_lines"], 450);
        assert_eq!(result.output["next_start_line"], json!(401));
        assert_eq!(result.output["remaining_lines"], json!(50));
    }

    #[tokio::test]
    async fn invoke_single_line_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("single.txt");
        fs::write(&file_path, "only one line\n").unwrap();

        let tool = ReadTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "t7".into(),
                tool_name: "read_file".into(),
                input: json!({ "path": "single.txt" }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["content"], "only one line\n");
        assert_eq!(result.output["start_line"], 1);
        assert_eq!(result.output["end_line"], 1);
        assert_eq!(result.output["total_lines"], 1);
        assert!(!result.output["truncated"].as_bool().unwrap());
        assert_eq!(result.output["next_start_line"], Value::Null);
        assert_eq!(result.output["remaining_lines"], Value::Null);
    }

    #[tokio::test]
    async fn invoke_file_not_found_error() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ReadTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "t8".into(),
                tool_name: "read_file".into(),
                input: json!({ "path": "nonexistent.txt" }),
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn invoke_start_line_exceeds_total_lines_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("short.txt");
        fs::write(&file_path, "hello\n").unwrap();

        let tool = ReadTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "t9".into(),
                tool_name: "read_file".into(),
                input: json!({ "path": "short.txt", "start_line": 10 }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["content"], "");
        assert_eq!(result.output["start_line"], 10);
        assert_eq!(result.output["end_line"], 1);
        assert_eq!(result.output["total_lines"], 1);
    }

    #[tokio::test]
    async fn invoke_end_line_exceeds_file_returns_until_end() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("med.txt");
        let lines: String = (1..=10).map(|i| format!("line {i}\n")).collect();
        fs::write(&file_path, &lines).unwrap();

        let tool = ReadTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "t10".into(),
                tool_name: "read_file".into(),
                input: json!({ "path": "med.txt", "start_line": 5, "end_line": 999 }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["start_line"], 5);
        assert_eq!(result.output["end_line"], 10);
        assert_eq!(result.output["total_lines"], 10);
    }

    #[tokio::test]
    async fn invoke_file_without_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("notrail.txt");
        fs::write(&file_path, "no newline at end").unwrap();

        let tool = ReadTool::new(dir.path().to_path_buf());
        let result = tool
            .invoke(ToolInvocation {
                id: "t11".into(),
                tool_name: "read_file".into(),
                input: json!({ "path": "notrail.txt" }),
            })
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(result.output["content"], "no newline at end");
        assert_eq!(result.output["start_line"], 1);
        assert_eq!(result.output["end_line"], 1);
        assert_eq!(result.output["total_lines"], 1);
    }
}
