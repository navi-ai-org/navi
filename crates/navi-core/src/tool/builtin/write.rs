use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use std::fs;
use std::path::Path;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

pub(crate) struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "write_file",
            "Write full UTF-8 text content to a project file.",
            ToolKind::Write,
            helpers::json_schema(
                &[
                    ("path", "Project-relative file path to write."),
                    ("content", "Full UTF-8 content to write."),
                ],
                &["path", "content"],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let path = helpers::required_string(&invocation.input, "path")?.to_string();
        let content = helpers::required_string(&invocation.input, "content")?.to_string();
        let path_clone = path.clone();
        let content_clone = content.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(parent) = Path::new(&path_clone).parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("failed to create {}", parent.display()))?;
                }
            }
            fs::write(&path_clone, content_clone)
                .with_context(|| format!("failed to write {path_clone}"))
        })
        .await
        .map_err(|e| anyhow::anyhow!("task join error: {}", e))??;
        Ok(helpers::ok(
            invocation.id,
            json!({ "path": path, "bytes": content.len() }),
        ))
    }
}
