use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use std::fs;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

const DEFAULT_READ_LINE_LIMIT: usize = 400;

pub(crate) struct ReadFileTool;

impl ReadFileTool {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "read_file",
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
        let path = helpers::required_string(&invocation.input, "path")?.to_string();
        let path_clone = path.clone();
        let content = tokio::task::spawn_blocking(move || {
            fs::read_to_string(&path_clone).with_context(|| format!("failed to read {path_clone}"))
        })
        .await
        .map_err(|e| anyhow::anyhow!("task join error: {}", e))??;

        if content.is_empty() {
            return Ok(helpers::ok(
                invocation.id,
                json!({
                    "path": path,
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
            (Some((end_idx + 1) as u64), Some((total_lines - end_idx) as u64))
        } else {
            (None, None)
        };

        Ok(helpers::ok(
            invocation.id,
            json!({
                "path": path,
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
