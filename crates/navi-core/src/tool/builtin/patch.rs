use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};
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

        let affected = extract_patched_files(patch);

        // Ensure parent directories exist for new files.
        for file in &affected {
            let full = self.project_root.join(file);
            if let Some(parent) = full.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
        }

        // Apply the patch.
        let output = run_git_apply(&self.project_root, patch).await?;

        if output.status.success() {
            Ok(ToolResult {
                invocation_id: invocation.id,
                ok: true,
                output: json!({
                    "status": output.status.code(),
                    "stdout": String::from_utf8_lossy(&output.stdout),
                    "stderr": String::from_utf8_lossy(&output.stderr),
                    "files_patched": affected.len(),
                }),
            })
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let hint = git_apply_error_hint(&stderr);

            Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: helpers::tool_error(
                    "patch_failed",
                    format!("git apply failed: {}", stderr.trim()),
                    true,
                    Some(hint),
                    Some(stderr.to_string()),
                ),
            })
        }
    }
}

/// Run `git apply` with the given patch on stdin.
async fn run_git_apply(project_root: &Path, patch: &str) -> Result<std::process::Output> {
    let mut child = Command::new("git")
        .args(["apply", "--whitespace=nowarn", "-"])
        .current_dir(project_root)
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
    child
        .wait_with_output()
        .await
        .context("failed to wait for git apply")
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

/// Map common git apply stderr messages to actionable recovery hints.
fn git_apply_error_hint(stderr: &str) -> &'static str {
    let lower = stderr.to_lowercase();
    if lower.contains("corrupt patch") {
        "Patch is malformed. Ensure it uses valid unified diff format: \
         --- a/path, +++ b/path, @@ hunk headers with line counts, and context lines (starting with space) \
         that exactly match the file on disk."
    } else if lower.contains("patch does not apply") || lower.contains("does not apply") {
        "The patch context lines don't match the file on disk. Re-read the file with read_file and \
         regenerate the diff against the content you see. Ensure the @@ hunk line numbers and counts \
         are correct for the target file."
    } else if lower.contains("no such file or directory") {
        "The target file doesn't exist. For new files, use --- /dev/null and +++ b/newfile/path. \
         For renames, ensure both old and new paths are correct."
    } else if lower.contains("already exists") {
        "The file already exists. To modify an existing file, use --- a/path and +++ b/path. \
         For new files, the file must not already exist."
    } else if lower.contains("permission denied") {
        "Permission denied. Check file permissions on the target file or directory."
    } else {
        "Check that the patch uses unified diff format with correct --- a/ and +++ b/ headers, \
         @@ hunk headers with accurate line numbers, and context lines that match the file content. \
         Re-read the file before regenerating the patch."
    }
}
