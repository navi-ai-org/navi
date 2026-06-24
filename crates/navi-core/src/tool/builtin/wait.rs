use crate::file_lock::FileLockManager;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
// use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

/// A tool that waits for a file lock to be released.
///
/// When the model encounters a "file_locked" error from `write_file` or
/// `apply_patch`, it can call this tool with the `file_path` it wants to wait
/// for. The tool blocks until the lock is released (up to a configurable
/// timeout), then returns a summary of the file state so the model can
/// proceed safely.
pub(crate) struct WaitTool {
    lock_manager: Arc<FileLockManager>,
}

impl WaitTool {
    pub(crate) fn new(lock_manager: Arc<FileLockManager>) -> Self {
        Self { lock_manager }
    }
}

#[async_trait]
impl Tool for WaitTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "wait",
            "Wait for a file lock to be released by another instance of NAVI.\n\n\
             Use this tool when `write_file` or `apply_patch` returns a `file_locked` error. \
             Provide the `file_path` of the locked file. The tool will block until the other \
             instance releases the lock (or the timeout expires), then report the file's current \
             state so you can proceed safely.\n\n\
             If you just need a simple delay without a file path, provide only the `timeout_seconds`.",
            ToolKind::Command,
            wait_json_schema(),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let file_path = helpers::optional_string(&invocation.input, "file_path");
        let timeout_s = helpers::optional_u64(&invocation.input, "timeout_seconds").unwrap_or(120);
        let poll_ms = helpers::optional_u64(&invocation.input, "poll_interval_ms").unwrap_or(500);

        let timeout = Duration::from_secs(timeout_s);
        let poll_interval = Duration::from_millis(poll_ms);

        if let Some(ref fp) = file_path {
            let path = std::path::PathBuf::from(fp);
            let lm = self.lock_manager.clone();
            let fp_clone = fp.clone();

            // Run the blocking wait on a dedicated thread via spawn_blocking.
            let (last_info, file_info_result) = tokio::task::spawn_blocking(move || {
                // Check if the file is actually locked.
                let is_locked = lm.is_locked(&path).ok().flatten();

                let last = if let Some(ref info) = is_locked {
                    tracing::info!(
                        file = %fp_clone,
                        locked_by = %info.instance_id,
                        timeout_s = timeout_s,
                        "wait tool waiting for file unlock"
                    );
                    lm.wait_for_unlock(&path, timeout, poll_interval)
                        .ok()
                        .flatten()
                } else {
                    None
                };

                // Read the current file state.
                let info = if last.is_none() && is_locked.is_some() {
                    // Lock was released - read file state
                    let content = std::fs::read_to_string(&fp_clone).ok();
                    let stat = std::fs::metadata(&fp_clone).ok();
                    let total_lines = content.as_ref().map(|c| c.lines().count()).unwrap_or(0);
                    let bytes = stat.as_ref().map(|s| s.len()).unwrap_or(0);
                    json!({
                        "total_lines": total_lines,
                        "bytes": bytes,
                    })
                } else {
                    json!({})
                };

                (last, info)
            })
            .await
            .map_err(|e| anyhow::anyhow!("wait task join error: {e}"))?;

            match last_info {
                Some(info) => {
                    // Timed out — lock still held.
                    Ok(ToolResult {
                        invocation_id: invocation.id,
                        ok: false,
                        output: json!({
                            "error": format!(
                                "Tempo limite de {}s excedido. O arquivo `{}` continua bloqueado \
                                 pela instância `{}` (sessão `{}`). Tente novamente mais tarde.",
                                timeout_s, fp, info.instance_id, info.session_id
                            ),
                            "error_code": "wait_timeout",
                            "file_path": fp,
                            "timeout_seconds": timeout_s,
                            "locked_by_instance": info.instance_id,
                            "locked_by_session": info.session_id,
                        }),
                    })
                }
                None => {
                    // File is unlocked (or was never locked).
                    let total_lines = file_info_result["total_lines"].as_u64().unwrap_or(0);
                    let message = if total_lines > 0 {
                        format!(
                            "O arquivo `{}` foi liberado. \
                             Leia o arquivo novamente com `read_file` para verificar o estado \
                             atual antes de fazer alterações. \
                             O arquivo agora tem {} linhas no total.",
                            fp, total_lines,
                        )
                    } else {
                        format!("O arquivo `{}` não está bloqueado. Pode prosseguir.", fp)
                    };

                    Ok(ToolResult {
                        invocation_id: invocation.id,
                        ok: true,
                        output: json!({
                            "message": message,
                            "status": "unlocked",
                            "file_path": fp,
                            "file_info": file_info_result,
                            "hint": "Read the file with `read_file` to inspect current content before applying changes.",
                        }),
                    })
                }
            }
        } else {
            // No file_path: simple async sleep-based wait.
            tracing::info!(timeout_s = timeout_s, "wait tool sleeping (no file_path)");
            tokio::time::sleep(timeout).await;
            Ok(ToolResult {
                invocation_id: invocation.id,
                ok: true,
                output: json!({
                    "message": format!("Aguardou {} segundos.", timeout_s),
                    "status": "slept",
                    "seconds": timeout_s,
                }),
            })
        }
    }
}

fn wait_json_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "file_path": {
                "type": "string",
                "description": "Project-relative path to the locked file to wait for. When provided, the tool waits until the lock is released (or the timeout expires). When omitted, the tool simply sleeps for the specified duration."
            },
            "timeout_seconds": {
                "type": "integer",
                "description": "Maximum time to wait, in seconds. Defaults to 120 (2 minutes). Max 600 (10 minutes).",
                "default": 120,
                "maximum": 600
            },
            "poll_interval_ms": {
                "type": "integer",
                "description": "How often to check the lock status, in milliseconds. Defaults to 500.",
                "default": 500,
                "maximum": 5000
            }
        },
        "additionalProperties": false
    })
}
