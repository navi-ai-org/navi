use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::path::Path;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

pub(crate) struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "grep",
            "Search project text files for a literal pattern.",
            ToolKind::Read,
            helpers::json_schema(
                &[
                    ("pattern", "Literal text pattern to search for."),
                    (
                        "path",
                        "Directory or file to search, defaults to project root.",
                    ),
                    ("max_results", "Maximum number of matches to return."),
                ],
                &["pattern"],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let pattern = helpers::required_string(&invocation.input, "pattern")?.to_string();
        let root =
            helpers::optional_string(&invocation.input, "path").unwrap_or_else(|| ".".to_string());
        let max_results =
            helpers::optional_u64(&invocation.input, "max_results").unwrap_or(200) as usize;
        let result = tokio::task::spawn_blocking(move || {
            let mut matches = Vec::new();
            helpers::grep_path(Path::new(&root), &pattern, max_results, &mut matches)?;
            let truncated = matches.len() >= max_results;
            Ok::<_, anyhow::Error>((matches, truncated))
        })
        .await
        .map_err(|e| anyhow::anyhow!("task join error: {}", e))??;
        Ok(helpers::ok(
            invocation.id,
            json!({ "matches": result.0, "truncated": result.1 }),
        ))
    }
}
