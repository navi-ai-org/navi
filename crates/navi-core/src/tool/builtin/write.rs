use crate::file_lock::FileLockManager;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

pub(crate) struct WriteFileTool {
    lock_manager: Option<Arc<FileLockManager>>,
}

impl WriteFileTool {
    pub(crate) fn new() -> Self {
        Self { lock_manager: None }
    }

    pub(crate) fn with_lock_manager(lock_manager: Arc<FileLockManager>) -> Self {
        Self {
            lock_manager: Some(lock_manager),
        }
    }
}

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

        // Acquire file lock if lock manager is configured.
        let _guard = if let Some(ref lm) = self.lock_manager {
            let lock_path = Path::new(&path_clone);
            match lm.try_lock(lock_path) {
                Ok(Some(guard)) => Some(guard),
                Ok(None) => {
                    return Ok(ToolResult {
                        invocation_id: invocation.id,
                        ok: false,
                        output: json!({
                            "error": format!(
                                "O arquivo `{}` está bloqueado por outra instância do NAVI. \
                                 Use a ferramenta `wait` com `file_path=\"{}\"` para aguardar.",
                                path_clone, path_clone
                            ),
                            "error_code": "file_locked",
                            "file_path": path_clone,
                        }),
                    });
                }
                Err(e) => {
                    tracing::warn!(path = %path_clone, error = %e, "failed to acquire file lock");
                    None
                }
            }
        } else {
            None
        };

        let (line_counts, _existing_content) = tokio::task::spawn_blocking(move || {
            let existing = fs::read_to_string(&path_clone).ok();
            let lines_removed = existing.as_ref().map(|c| count_lines(c)).unwrap_or(0);

            if let Some(parent) = Path::new(&path_clone).parent()
                && !parent.as_os_str().is_empty()
            {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            fs::write(&path_clone, content_clone)
                .with_context(|| format!("failed to write {path_clone}"))?;

            Ok::<_, anyhow::Error>((lines_removed, existing))
        })
        .await
        .map_err(|e| anyhow::anyhow!("task join error: {}", e))??;

        let lines_added = count_lines(&content);
        let output = json!({
            "path": path,
            "bytes": content.len(),
            "lines_added": lines_added,
            "lines_removed": line_counts,
            "total_lines": lines_added,
        });

        // Lock guard released automatically when _guard drops here.
        Ok(helpers::ok(invocation.id, output))
    }
}

fn count_lines(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.lines().count().max(1)
    }
}
