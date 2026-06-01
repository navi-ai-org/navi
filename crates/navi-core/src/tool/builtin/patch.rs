use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

pub(crate) struct ApplyPatchTool {
    project_root: PathBuf,
}

impl ApplyPatchTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self { project_root }
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
        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: output.status.success(),
            output: json!({
                "status": output.status.code(),
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
            }),
        })
    }
}
