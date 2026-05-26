use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

pub(super) struct ReadFileTool;
pub(super) struct WriteFileTool;
pub(super) struct ApplyPatchTool;
pub(super) struct ListFilesTool;
pub(super) struct GrepTool;
pub(super) struct BashTool {
    background: Arc<BashBackgroundRegistry>,
}

impl BashTool {
    pub(super) fn new() -> Self {
        Self {
            background: Arc::new(BashBackgroundRegistry::default()),
        }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn definition(&self) -> ToolDefinition {
        definition(
            "read_file",
            "Read a UTF-8 text file from the current project, optionally specifying a line range.",
            ToolKind::Read,
            json_schema(
                &[
                    ("path", "Project-relative file path to read."),
                    (
                        "start_line",
                        "Line number to start reading from (1-indexed, defaults to 1).",
                    ),
                    (
                        "end_line",
                        "Line number to stop reading at (1-indexed, inclusive, defaults to start_line + 999).",
                    ),
                ],
                &["path"],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let path = required_string(&invocation.input, "path")?;
        let content =
            fs::read_to_string(&path).with_context(|| format!("failed to read {path}"))?;

        if content.is_empty() {
            return Ok(ok(
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

        let start_line = optional_u64(&invocation.input, "start_line").unwrap_or(1);
        let end_line = optional_u64(&invocation.input, "end_line");

        let start_idx = (start_line.max(1) - 1) as usize;
        let end_idx = match end_line {
            Some(e) => (e as usize).clamp(start_idx, total_lines),
            None => (start_idx + 1000).min(total_lines),
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

        Ok(ok(
            invocation.id,
            json!({
                "path": path,
                "content": sliced_content,
                "start_line": start_idx + 1,
                "end_line": end_idx,
                "total_lines": total_lines,
                "truncated": truncated
            }),
        ))
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn definition(&self) -> ToolDefinition {
        definition(
            "write_file",
            "Write full UTF-8 text content to a project file.",
            ToolKind::Write,
            json_schema(
                &[
                    ("path", "Project-relative file path to write."),
                    ("content", "Full UTF-8 content to write."),
                ],
                &["path", "content"],
            ),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let path = required_string(&invocation.input, "path")?;
        let content = required_string(&invocation.input, "content")?;
        if let Some(parent) = Path::new(&path).parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
        }
        fs::write(&path, content).with_context(|| format!("failed to write {path}"))?;
        Ok(ok(
            invocation.id,
            json!({ "path": path, "bytes": content.len() }),
        ))
    }
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn definition(&self) -> ToolDefinition {
        definition(
            "apply_patch",
            "Apply a unified diff patch to the current project.",
            ToolKind::Write,
            json_schema(&[("patch", "Unified diff patch text.")], &["patch"]),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let patch = required_string(&invocation.input, "patch")?;
        let mut child = Command::new("git")
            .args(["apply", "--whitespace=nowarn", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn git apply")?;
        child
            .stdin
            .as_mut()
            .context("failed to open git apply stdin")?
            .write_all(patch.as_bytes())
            .context("failed to send patch to git apply")?;
        let output = child
            .wait_with_output()
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

#[async_trait]
impl Tool for ListFilesTool {
    fn definition(&self) -> ToolDefinition {
        definition(
            "list_files",
            "List project files, optionally filtering by substring.",
            ToolKind::Read,
            json_schema(
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
        let root = optional_string(&invocation.input, "path").unwrap_or_else(|| ".".to_string());
        let filter = optional_string(&invocation.input, "filter");
        let max_results = optional_u64(&invocation.input, "max_results").unwrap_or(200) as usize;
        let mut files = Vec::new();
        collect_files(Path::new(&root), filter.as_deref(), max_results, &mut files)?;
        let truncated = files.len() >= max_results;
        Ok(ok(
            invocation.id,
            json!({ "files": files, "truncated": truncated }),
        ))
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn definition(&self) -> ToolDefinition {
        definition(
            "grep",
            "Search project text files for a literal pattern.",
            ToolKind::Read,
            json_schema(
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
        let pattern = required_string(&invocation.input, "pattern")?;
        let root = optional_string(&invocation.input, "path").unwrap_or_else(|| ".".to_string());
        let max_results = optional_u64(&invocation.input, "max_results").unwrap_or(200) as usize;
        let mut matches = Vec::new();
        grep_path(Path::new(&root), &pattern, max_results, &mut matches)?;
        let truncated = matches.len() >= max_results;
        Ok(ok(
            invocation.id,
            json!({ "matches": matches, "truncated": truncated }),
        ))
    }
}

const BASH_DEFAULT_TIMEOUT_MS: u64 = 30_000;
const BASH_MAX_TIMEOUT_MS: u64 = 120_000;
const BASH_DEFAULT_BACKGROUND_TIMEOUT_MS: u64 = 600_000;
const BASH_MAX_BACKGROUND_TIMEOUT_MS: u64 = 1_800_000;
const BASH_DEFAULT_WAIT_MS: u64 = 15_000;
const BASH_MAX_WAIT_MS: u64 = 60_000;
const BASH_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
const BASH_MAX_BACKGROUND_TASKS: usize = 8;

#[derive(Default)]
struct BashBackgroundRegistry {
    next_id: AtomicU64,
    tasks: tokio::sync::Mutex<HashMap<String, Arc<BashBackgroundTask>>>,
}

impl BashBackgroundRegistry {
    async fn spawn_task(
        &self,
        command: String,
        description: Option<String>,
        timeout_ms: u64,
    ) -> Result<Arc<BashBackgroundTask>> {
        let mut tasks = self.tasks.lock().await;
        let running_tasks = tasks
            .values()
            .filter(|task| !task.snapshot_state().is_final())
            .count();
        if running_tasks >= BASH_MAX_BACKGROUND_TASKS {
            anyhow::bail!("too many background bash tasks running");
        }

        let task_id = format!("bg_{}", self.next_id.fetch_add(1, Ordering::SeqCst) + 1);
        let task = Arc::new(BashBackgroundTask::spawn(
            task_id.clone(),
            command,
            description,
            timeout_ms,
        )?);
        tasks.insert(task_id, task.clone());
        Ok(task)
    }

    async fn get(&self, task_id: &str) -> Option<Arc<BashBackgroundTask>> {
        self.tasks.lock().await.get(task_id).cloned()
    }

    async fn list(&self, invocation_id: String) -> ToolResult {
        let tasks = self.tasks.lock().await;
        let mut values = Vec::new();
        for task in tasks.values() {
            task.refresh_status().await;
            values.push(task.snapshot_json().await);
        }
        values.sort_by(|a, b| {
            a.get("task_id")
                .and_then(Value::as_str)
                .cmp(&b.get("task_id").and_then(Value::as_str))
        });
        ok(invocation_id, json!({ "tasks": values }))
    }
}

struct BashBackgroundTask {
    task_id: String,
    command: String,
    description: Option<String>,
    started_at: Instant,
    timeout_ms: u64,
    child: tokio::sync::Mutex<Option<tokio::process::Child>>,
    stdout: Arc<tokio::sync::Mutex<Vec<u8>>>,
    stderr: Arc<tokio::sync::Mutex<Vec<u8>>>,
    state: std::sync::Mutex<BashBackgroundState>,
}

impl BashBackgroundTask {
    fn spawn(
        task_id: String,
        command: String,
        description: Option<String>,
        timeout_ms: u64,
    ) -> Result<Self> {
        let mut child = tokio::process::Command::new("bash")
            .arg("-lc")
            .arg(&command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("failed to spawn bash")?;

        let stdout = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let stderr = Arc::new(tokio::sync::Mutex::new(Vec::new()));

        if let Some(stdout_pipe) = child.stdout.take() {
            spawn_output_reader(stdout_pipe, stdout.clone());
        }
        if let Some(stderr_pipe) = child.stderr.take() {
            spawn_output_reader(stderr_pipe, stderr.clone());
        }

        Ok(Self {
            task_id,
            command,
            description,
            started_at: Instant::now(),
            timeout_ms,
            child: tokio::sync::Mutex::new(Some(child)),
            stdout,
            stderr,
            state: std::sync::Mutex::new(BashBackgroundState::running()),
        })
    }

    fn snapshot_state(&self) -> BashBackgroundState {
        self.state.lock().unwrap().clone()
    }

    async fn observe(&self, wait_ms: u64, invocation_id: String) -> ToolResult {
        let deadline = Instant::now() + Duration::from_millis(wait_ms);
        loop {
            self.refresh_status().await;
            if self.snapshot_state().is_final() || Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        ok(invocation_id, self.observation_json().await)
    }

    async fn cancel(&self, invocation_id: String) -> ToolResult {
        let mut child = self.child.lock().await;
        if let Some(child) = child.as_mut() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        *child = None;
        {
            let mut state = self.state.lock().unwrap();
            if !state.is_final() {
                *state = BashBackgroundState::cancelled();
            }
        }
        ok(invocation_id, self.observation_json().await)
    }

    async fn refresh_status(&self) {
        if self.snapshot_state().is_final() {
            return;
        }

        let timed_out = self.started_at.elapsed() >= Duration::from_millis(self.timeout_ms);
        let mut child = self.child.lock().await;
        let Some(child_ref) = child.as_mut() else {
            return;
        };

        match child_ref.try_wait() {
            Ok(Some(status)) => {
                *child = None;
                let mut state = self.state.lock().unwrap();
                *state = BashBackgroundState::completed(status.success(), status.code());
            }
            Ok(None) if timed_out => {
                let _ = child_ref.kill().await;
                let _ = child_ref.wait().await;
                *child = None;
                let mut state = self.state.lock().unwrap();
                *state = BashBackgroundState::timed_out();
            }
            Ok(None) => {}
            Err(err) => {
                *child = None;
                let mut state = self.state.lock().unwrap();
                *state = BashBackgroundState::failed(format!("failed to poll command: {err}"));
            }
        }
    }

    async fn observation_json(&self) -> Value {
        let state = self.snapshot_state();
        let mut value = self.snapshot_json().await;
        if !state.is_final() {
            value["message"] = json!(format!(
                "Command is still running. Poll with bash({{\"task_id\":\"{}\"}}) or cancel with bash({{\"task_id\":\"{}\",\"action\":\"cancel\"}}).",
                self.task_id, self.task_id
            ));
        }
        value
    }

    async fn snapshot_json(&self) -> Value {
        let state = self.snapshot_state();
        let stdout = String::from_utf8_lossy(&self.stdout.lock().await).into_owned();
        let stderr = String::from_utf8_lossy(&self.stderr.lock().await).into_owned();
        let mut output = json!({
            "task_id": self.task_id,
            "command": self.command,
            "description": self.description,
            "background": true,
            "status": state.label,
            "elapsed_ms": self.started_at.elapsed().as_millis() as u64,
            "timeout_ms": self.timeout_ms,
            "stdout": truncate_string(stdout, BASH_OUTPUT_LIMIT_BYTES),
            "stderr": truncate_string(stderr, BASH_OUTPUT_LIMIT_BYTES),
        });
        if let Some(code) = state.exit_code {
            output["exit_code"] = json!(code);
        }
        if let Some(error) = state.error {
            output["error"] = json!(error);
        }
        output
    }
}

#[derive(Clone)]
struct BashBackgroundState {
    label: &'static str,
    exit_code: Option<i32>,
    error: Option<String>,
}

impl BashBackgroundState {
    fn running() -> Self {
        Self {
            label: "running",
            exit_code: None,
            error: None,
        }
    }

    fn completed(ok: bool, exit_code: Option<i32>) -> Self {
        Self {
            label: if ok { "completed" } else { "failed" },
            exit_code,
            error: None,
        }
    }

    fn failed(error: String) -> Self {
        Self {
            label: "failed",
            exit_code: None,
            error: Some(error),
        }
    }

    fn timed_out() -> Self {
        Self {
            label: "timed_out",
            exit_code: None,
            error: Some("command timed out: deadline has elapsed".to_string()),
        }
    }

    fn cancelled() -> Self {
        Self {
            label: "cancelled",
            exit_code: None,
            error: Some("command cancelled".to_string()),
        }
    }

    fn is_final(&self) -> bool {
        self.label != "running"
    }
}

fn spawn_output_reader<R>(mut reader: R, output: Arc<tokio::sync::Mutex<Vec<u8>>>)
where
    R: tokio::io::AsyncRead + Send + Unpin + 'static,
{
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut buf = [0; 4096];
        while let Ok(n) = reader.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let mut data = output.lock().await;
            if data.len() < BASH_OUTPUT_LIMIT_BYTES {
                let remaining = BASH_OUTPUT_LIMIT_BYTES - data.len();
                data.extend_from_slice(&buf[..n.min(remaining)]);
            }
        }
    });
}

#[async_trait]
impl Tool for BashTool {
    fn definition(&self) -> ToolDefinition {
        definition(
            "bash",
            "Run a shell command in the current project. Use background=true and wait_ms for long-running commands, then poll or cancel with task_id.",
            ToolKind::Command,
            bash_json_schema(),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let action = optional_string(&invocation.input, "action");
        if action.as_deref() == Some("list") {
            return Ok(self.background.list(invocation.id).await);
        }

        if let Some(task_id) = optional_string(&invocation.input, "task_id") {
            let Some(task) = self.background.get(&task_id).await else {
                return Ok(ToolResult {
                    invocation_id: invocation.id,
                    ok: false,
                    output: json!({ "error": format!("unknown background task `{task_id}`") }),
                });
            };
            if action.as_deref() == Some("cancel") {
                return Ok(task.cancel(invocation.id).await);
            }
            let wait_ms = optional_u64(&invocation.input, "wait_ms")
                .unwrap_or(BASH_DEFAULT_WAIT_MS)
                .min(BASH_MAX_WAIT_MS);
            return Ok(task.observe(wait_ms, invocation.id).await);
        }

        let command = required_string(&invocation.input, "command")?;
        if optional_bool(&invocation.input, "background").unwrap_or(false) {
            let timeout_ms = optional_u64(&invocation.input, "timeout_ms")
                .unwrap_or(BASH_DEFAULT_BACKGROUND_TIMEOUT_MS)
                .min(BASH_MAX_BACKGROUND_TIMEOUT_MS);
            let wait_ms = optional_u64(&invocation.input, "wait_ms")
                .unwrap_or(BASH_DEFAULT_WAIT_MS)
                .min(BASH_MAX_WAIT_MS);
            let task = self
                .background
                .spawn_task(
                    command.to_string(),
                    optional_string(&invocation.input, "description"),
                    timeout_ms,
                )
                .await?;
            return Ok(task.observe(wait_ms, invocation.id).await);
        }

        let timeout_ms = optional_u64(&invocation.input, "timeout_ms")
            .unwrap_or(BASH_DEFAULT_TIMEOUT_MS)
            .min(BASH_MAX_TIMEOUT_MS);

        let mut child = tokio::process::Command::new("bash")
            .arg("-lc")
            .arg(&command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("failed to spawn bash")?;

        let mut stdout_pipe = child.stdout.take().unwrap();
        let mut stderr_pipe = child.stderr.take().unwrap();

        let stdout_data = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let stderr_data = Arc::new(tokio::sync::Mutex::new(Vec::new()));

        let stdout_data_clone = stdout_data.clone();
        let mut stdout_task = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buf = [0; 4096];
            while let Ok(n) = stdout_pipe.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut data = stdout_data_clone.lock().await;
                if data.len() < 64 * 1024 {
                    data.extend_from_slice(&buf[..n]);
                }
            }
        });

        let stderr_data_clone = stderr_data.clone();
        let mut stderr_task = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buf = [0; 4096];
            while let Ok(n) = stderr_pipe.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut data = stderr_data_clone.lock().await;
                if data.len() < 64 * 1024 {
                    data.extend_from_slice(&buf[..n]);
                }
            }
        });

        let timeout_duration = Duration::from_millis(timeout_ms);
        let status_result = tokio::time::timeout(timeout_duration, child.wait()).await;

        let (ok, status_code, error_msg) = match status_result {
            Ok(Ok(status)) => (status.success(), status.code(), None),
            Ok(Err(e)) => (
                false,
                None,
                Some(format!("failed to wait for command: {e}")),
            ),
            Err(_) => (
                false,
                None,
                Some("command timed out: deadline has elapsed".to_string()),
            ),
        };

        let _ = tokio::time::timeout(Duration::from_millis(50), async {
            let _ = tokio::join!(&mut stdout_task, &mut stderr_task);
        })
        .await;

        stdout_task.abort();
        stderr_task.abort();

        let stdout_bytes = stdout_data.lock().await.clone();
        let stderr_bytes = stderr_data.lock().await.clone();

        let stdout_str = String::from_utf8_lossy(&stdout_bytes).into_owned();
        let stderr_str = String::from_utf8_lossy(&stderr_bytes).into_owned();

        if let Some(err) = error_msg {
            Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: json!({
                    "error": err,
                    "stdout": truncate_string(stdout_str, 64 * 1024),
                    "stderr": truncate_string(stderr_str, 64 * 1024),
                }),
            })
        } else {
            Ok(ToolResult {
                invocation_id: invocation.id,
                ok,
                output: json!({
                    "status": status_code,
                    "stdout": truncate_string(stdout_str, 64 * 1024),
                    "stderr": truncate_string(stderr_str, 64 * 1024),
                }),
            })
        }
    }
}

fn definition(
    name: &str,
    description: &str,
    kind: ToolKind,
    input_schema: Value,
) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        kind,
        input_schema,
    }
}

fn json_schema(properties: &[(&str, &str)], required: &[&str]) -> Value {
    let properties = properties
        .iter()
        .map(|(name, description)| {
            let is_integer = name.starts_with("max_")
                || *name == "timeout_ms"
                || name.ends_with("_line")
                || name.starts_with("start_")
                || name.starts_with("end_");
            (
                (*name).to_string(),
                json!({ "type": if is_integer { "integer" } else { "string" }, "description": description }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn bash_json_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "Shell command to run. Required when starting a new command."
            },
            "description": {
                "type": "string",
                "description": "Short reason for running this command, used for approval and trace context."
            },
            "timeout_ms": {
                "type": "integer",
                "description": "Maximum command lifetime in milliseconds, capped internally."
            },
            "wait_ms": {
                "type": "integer",
                "description": "How long to wait for foreground observation before returning running status for background tasks."
            },
            "background": {
                "type": "boolean",
                "description": "When true, keep the command running after wait_ms and return a task_id for polling."
            },
            "task_id": {
                "type": "string",
                "description": "Background task id returned by an earlier bash call."
            },
            "action": {
                "type": "string",
                "enum": ["poll", "cancel", "list"],
                "description": "Use poll/cancel with task_id, or list to show background tasks."
            }
        },
        "anyOf": [
            { "required": ["command"] },
            { "required": ["task_id"] },
            { "properties": { "action": { "const": "list" } }, "required": ["action"] }
        ],
        "additionalProperties": false,
    })
}

fn required_string<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("missing required string `{key}`"))
}

fn optional_string(input: &Value, key: &str) -> Option<String> {
    input.get(key).and_then(Value::as_str).map(str::to_string)
}

fn optional_u64(input: &Value, key: &str) -> Option<u64> {
    input.get(key).and_then(Value::as_u64)
}

fn optional_bool(input: &Value, key: &str) -> Option<bool> {
    input.get(key).and_then(Value::as_bool)
}

fn ok(invocation_id: String, output: Value) -> ToolResult {
    ToolResult {
        invocation_id,
        ok: true,
        output,
    }
}

pub(super) fn truncate_tool_result(mut result: ToolResult) -> ToolResult {
    result.output = truncate_json(result.output, 128 * 1024);
    result
}

fn truncate_json(value: Value, max_bytes: usize) -> Value {
    let serialized = value.to_string();
    if serialized.len() <= max_bytes {
        value
    } else {
        json!({ "truncated": true, "content": truncate_string(serialized, max_bytes) })
    }
}

fn truncate_string(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    value.truncate(max_bytes);
    while !value.is_char_boundary(value.len()) {
        value.pop();
    }
    value.push_str("\n<truncated>");
    value
}

fn collect_files(
    root: &Path,
    filter: Option<&str>,
    max_results: usize,
    files: &mut Vec<String>,
) -> Result<()> {
    if files.len() >= max_results || should_skip(root) {
        return Ok(());
    }
    if root.is_file() {
        let display = root.display().to_string();
        if filter.is_none_or(|filter| display.contains(filter)) {
            files.push(display);
        }
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("failed to list {}", root.display()))? {
        if files.len() >= max_results {
            break;
        }
        collect_files(&entry?.path(), filter, max_results, files)?;
    }
    Ok(())
}

fn grep_path(
    root: &Path,
    pattern: &str,
    max_results: usize,
    matches: &mut Vec<Value>,
) -> Result<()> {
    if matches.len() >= max_results || should_skip(root) {
        return Ok(());
    }
    if root.is_file() {
        if let Ok(content) = fs::read_to_string(root) {
            for (index, line) in content.lines().enumerate() {
                if line.contains(pattern) {
                    matches.push(json!({
                        "path": root.display().to_string(),
                        "line": index + 1,
                        "text": line,
                    }));
                    if matches.len() >= max_results {
                        break;
                    }
                }
            }
        }
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("failed to list {}", root.display()))? {
        if matches.len() >= max_results {
            break;
        }
        grep_path(&entry?.path(), pattern, max_results, matches)?;
    }
    Ok(())
}

fn should_skip(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                ".git" | "target" | "node_modules" | ".cache" | ".venv" | "__pycache__"
            )
        })
}
