use anyhow::{Context, Result};
use async_trait::async_trait;
use navi_vfs::VfsEngine;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

pub(crate) struct ApplyPatchTool {
    project_root: PathBuf,
    vfs: Option<Arc<VfsEngine>>,
}

impl ApplyPatchTool {
    pub(crate) fn new(project_root: PathBuf, vfs: Option<Arc<VfsEngine>>) -> Self {
        Self { project_root, vfs }
    }
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "apply_patch",
            "Apply a unified diff patch to the current project.",
            ToolKind::Write,
            helpers::json_schema(&[("patch", "Unified diff patch text.")], &["patch"]),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let patch = helpers::required_string(&invocation.input, "patch")?;

        // Extract affected file paths before applying.
        let affected_files = extract_patched_files(patch);

        let mut child = Command::new("git")
            .args(["apply", "--whitespace=nowarn", "-"])
            .current_dir(&self.project_root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("failed to spawn git apply")?;
        child
            .stdin
            .as_mut()
            .context("failed to open git apply stdin")?
            .write_all(patch.as_bytes())
            .await
            .context("failed to send patch to git apply")?;
        let output = child
            .wait_with_output()
            .await
            .context("failed to wait for git apply")?;

        // VFS: format affected files after successful patch.
        let vfs_formatted = if output.status.success() {
            if let Some(ref vfs) = self.vfs {
                let paths: Vec<PathBuf> = affected_files
                    .iter()
                    .map(|f| self.project_root.join(f))
                    .collect();
                let path_refs: Vec<&std::path::Path> = paths.iter().map(|p| p.as_path()).collect();
                vfs.format_after_patch(&path_refs);
                paths.len()
            } else {
                0
            }
        } else {
            0
        };

        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: output.status.success(),
            output: json!({
                "status": output.status.code(),
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
                "vfs_formatted": vfs_formatted,
            }),
        })
    }
}

/// Extract file paths from a unified diff patch.
fn extract_patched_files(patch: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in patch.lines() {
        // Unified diff: "--- a/path/to/file" and "+++ b/path/to/file"
        if let Some(path) = line.strip_prefix("--- a/")
            && !files.contains(&path.to_string())
        {
            files.push(path.to_string());
        }
        if let Some(path) = line.strip_prefix("+++ b/")
            && !files.contains(&path.to_string())
        {
            files.push(path.to_string());
        }
    }
    files
}
