use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::path::Path;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

pub(crate) struct ListFilesTool;

#[async_trait]
impl Tool for ListFilesTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "list_files",
            "List project files, optionally filtering by substring.",
            ToolKind::Read,
            helpers::json_schema(
                &[
                    ("path", "Directory to list, defaults to current project."),
                    ("filter", "Optional substring filter."),
                    ("max_results", "Maximum number of files to return."),
                ],
                &[],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let root =
            helpers::optional_string(&invocation.input, "path").unwrap_or_else(|| ".".to_string());
        let filter = helpers::optional_string(&invocation.input, "filter");
        let max_results =
            helpers::optional_u64(&invocation.input, "max_results").unwrap_or(200) as usize;
        let result = tokio::task::spawn_blocking(move || {
            let mut files = Vec::new();
            helpers::collect_files(Path::new(&root), filter.as_deref(), max_results, &mut files)?;
            let truncated = files.len() >= max_results;
            Ok::<_, anyhow::Error>((files, truncated))
        })
        .await
        .map_err(|e| anyhow::anyhow!("task join error: {}", e))??;
        Ok(helpers::ok(
            invocation.id,
            json!({ "files": result.0, "truncated": result.1 }),
        ))
    }
}
