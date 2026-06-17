use serde::{Deserialize, Serialize};

/// Status of a background bash command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
}

impl BackgroundTaskStatus {
    pub fn is_final(self) -> bool {
        self != Self::Running
    }
}

impl std::fmt::Display for BackgroundTaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => f.write_str("running"),
            Self::Completed => f.write_str("completed"),
            Self::Failed => f.write_str("failed"),
            Self::TimedOut => f.write_str("timed_out"),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}

/// Snapshot of a background bash command's current state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundCommandSnapshot {
    pub task_id: String,
    pub command: String,
    pub description: Option<String>,
    pub status: BackgroundTaskStatus,
    pub elapsed_ms: u64,
    pub timeout_ms: u64,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub error: Option<String>,
}

impl BackgroundCommandSnapshot {
    /// Parse from the JSON output of a `bash` tool invocation with
    /// `action=list`, `task_id=...`, or `background=true` spawn result.
    pub fn from_json(value: &serde_json::Value) -> Option<Self> {
        let obj = value.as_object()?;
        let task_id = obj.get("task_id")?.as_str()?.to_string();
        let command = obj.get("command")?.as_str()?.to_string();
        let description = obj
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from);
        let status_str = obj.get("status")?.as_str()?;
        let status = match status_str {
            "running" => BackgroundTaskStatus::Running,
            "completed" => BackgroundTaskStatus::Completed,
            "failed" => BackgroundTaskStatus::Failed,
            "timed_out" => BackgroundTaskStatus::TimedOut,
            "cancelled" => BackgroundTaskStatus::Cancelled,
            _ => return None,
        };
        let elapsed_ms = obj.get("elapsed_ms")?.as_u64()?;
        let timeout_ms = obj.get("timeout_ms")?.as_u64()?;
        let exit_code = obj
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32);
        let stdout = obj
            .get("stdout")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let stderr = obj
            .get("stderr")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let stdout_truncated = obj
            .get("stdout_truncated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let stderr_truncated = obj
            .get("stderr_truncated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let error = obj.get("error").and_then(|v| v.as_str()).map(String::from);

        Some(Self {
            task_id,
            command,
            description,
            status,
            elapsed_ms,
            timeout_ms,
            exit_code,
            stdout,
            stderr,
            stdout_truncated,
            stderr_truncated,
            error,
        })
    }

    /// True if the task is still running.
    pub fn is_running(&self) -> bool {
        self.status == BackgroundTaskStatus::Running
    }
}
