use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures_util::future::join_all;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

// ── Constants ─────────────────────────────────────────────────────────────────

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_BG_TIMEOUT_MS: u64 = 600_000;
const MAX_BG_TIMEOUT_MS: u64 = 1_800_000;
const DEFAULT_WAIT_MS: u64 = 15_000;
const MAX_WAIT_MS: u64 = 60_000;
const OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
const MAX_PROCESSES: usize = 8;

// ── ProcessManager (quota config) ─────────────────────────────────────────────

/// Quota and configuration for process execution.
#[derive(Clone)]
pub(crate) struct ProcessManager {
    pub max_processes: usize,
    pub default_timeout_ms: u64,
    pub max_output_bytes: usize,
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self {
            max_processes: MAX_PROCESSES,
            default_timeout_ms: DEFAULT_TIMEOUT_MS,
            max_output_bytes: OUTPUT_LIMIT_BYTES,
        }
    }
}

impl ProcessManager {
    #[cfg(test)]
    pub fn new(max_processes: usize, default_timeout_ms: u64, max_output_bytes: usize) -> Self {
        Self {
            max_processes,
            default_timeout_ms,
            max_output_bytes,
        }
    }
}

// ── ProcessTool ────────────────────────────────────────────────────────────────

pub(crate) struct ProcessTool {
    registry: Arc<ProcessRegistry>,
    project_root: PathBuf,
    config: ProcessManager,
}

impl ProcessTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self {
            registry: Arc::new(ProcessRegistry::default()),
            project_root,
            config: ProcessManager::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_config(project_root: PathBuf, config: ProcessManager) -> Self {
        Self {
            registry: Arc::new(ProcessRegistry::default()),
            project_root,
            config,
        }
    }
}

// ── Process schema ─────────────────────────────────────────────────────────────

fn process_json_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["exec", "stdin", "wait", "list", "cancel"],
                "description": "Action to perform: exec (run command), stdin (write data to process stdin), wait (wait for process to finish), list (show running processes), cancel (kill a running process)."
            },
            "command": {
                "type": "string",
                "description": "Command to execute. Required for the exec action. The first whitespace-separated token is the program, the rest are arguments. No shell expansion is performed."
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
                "description": "How long to wait for background observation before returning a running status."
            },
            "background": {
                "type": "boolean",
                "description": "When true, keep the command running after the initial wait and return a process_id for polling."
            },
            "process_id": {
                "type": "string",
                "description": "Process ID returned by an earlier exec call. Required for stdin, wait, and cancel actions."
            },
            "stdin_data": {
                "type": "string",
                "description": "Data to write to a running process's stdin. Required for the stdin action."
            }
        },
        "anyOf": [
            { "required": ["action", "command"], "properties": { "action": { "const": "exec" } } },
            { "required": ["action", "process_id", "stdin_data"], "properties": { "action": { "const": "stdin" } } },
            { "required": ["action", "process_id"], "properties": { "action": { "const": "wait" } } },
            { "required": ["action", "process_id"], "properties": { "action": { "const": "cancel" } } },
            { "required": ["action"], "properties": { "action": { "const": "list" } } }
        ],
        "additionalProperties": false
    })
}

// ── Process status types ───────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
enum ProcStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
}

impl std::fmt::Display for ProcStatus {
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

impl ProcStatus {
    fn is_final(&self) -> bool {
        *self != Self::Running
    }
}

#[derive(Clone)]
struct ProcState {
    status: ProcStatus,
    exit_code: Option<i32>,
    error: Option<String>,
}

impl ProcState {
    fn running() -> Self {
        Self {
            status: ProcStatus::Running,
            exit_code: None,
            error: None,
        }
    }

    fn completed(ok: bool, exit_code: Option<i32>) -> Self {
        Self {
            status: if ok {
                ProcStatus::Completed
            } else {
                ProcStatus::Failed
            },
            exit_code,
            error: None,
        }
    }

    fn failed(error: String) -> Self {
        Self {
            status: ProcStatus::Failed,
            exit_code: None,
            error: Some(error),
        }
    }

    fn timed_out() -> Self {
        Self {
            status: ProcStatus::TimedOut,
            exit_code: None,
            error: Some("process timed out: deadline has elapsed".to_string()),
        }
    }

    fn cancelled() -> Self {
        Self {
            status: ProcStatus::Cancelled,
            exit_code: None,
            error: Some("process cancelled".to_string()),
        }
    }

    fn is_final(&self) -> bool {
        self.status.is_final()
    }
}

// ── ProcessRegistry ────────────────────────────────────────────────────────────

#[derive(Default)]
struct ProcessRegistry {
    next_id: AtomicU64,
    processes: tokio::sync::Mutex<HashMap<String, Arc<ManagedProcess>>>,
}

impl ProcessRegistry {
    async fn spawn(
        &self,
        command: String,
        description: Option<String>,
        project_root: PathBuf,
        timeout_ms: u64,
        max_output_bytes: usize,
        max_processes: usize,
    ) -> Result<Arc<ManagedProcess>> {
        let mut procs = self.processes.lock().await;
        let running = procs
            .values()
            .filter(|p| !p.snapshot_state().is_final())
            .count();
        if running >= max_processes {
            anyhow::bail!("too many running processes (max: {max_processes})");
        }

        let process_id = format!("proc_{}", self.next_id.fetch_add(1, Ordering::SeqCst) + 1);
        let process = Arc::new(ManagedProcess::spawn(
            process_id.clone(),
            command,
            description,
            project_root,
            timeout_ms,
            max_output_bytes,
        )?);
        procs.insert(process_id, process.clone());
        Ok(process)
    }

    async fn get(&self, process_id: &str) -> Option<Arc<ManagedProcess>> {
        self.processes.lock().await.get(process_id).cloned()
    }

    async fn list(&self, invocation_id: String) -> ToolResult {
        let procs = self.processes.lock().await;
        let futures: Vec<_> = procs.values().map(|p| p.snapshot_json()).collect();
        let mut values = join_all(futures).await;
        values.sort_by(|a, b| {
            a.get("process_id")
                .and_then(Value::as_str)
                .cmp(&b.get("process_id").and_then(Value::as_str))
        });
        helpers::ok(invocation_id, json!({ "processes": values }))
    }
}

// ── ManagedProcess ─────────────────────────────────────────────────────────────

struct ManagedProcess {
    process_id: String,
    command: String,
    description: Option<String>,
    started_at: Instant,
    timeout_ms: u64,
    max_output_bytes: usize,
    child: tokio::sync::Mutex<Option<tokio::process::Child>>,
    stdin_handle: tokio::sync::Mutex<Option<tokio::process::ChildStdin>>,
    stdout: Arc<tokio::sync::Mutex<Vec<u8>>>,
    stderr: Arc<tokio::sync::Mutex<Vec<u8>>>,
    state: std::sync::Mutex<ProcState>,
}

impl ManagedProcess {
    fn spawn(
        process_id: String,
        command: String,
        description: Option<String>,
        project_root: PathBuf,
        timeout_ms: u64,
        max_output_bytes: usize,
    ) -> Result<Self> {
        let (program, args) = split_command(&command);
        let mut child = tokio::process::Command::new(&program)
            .args(&args)
            .current_dir(&project_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to spawn process: {command}"))?;

        let stdin_handle = child.stdin.take();
        let stdout = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let stderr = Arc::new(tokio::sync::Mutex::new(Vec::new()));

        if let Some(stdout_pipe) = child.stdout.take() {
            spawn_output_reader(stdout_pipe, stdout.clone(), max_output_bytes);
        }
        if let Some(stderr_pipe) = child.stderr.take() {
            spawn_output_reader(stderr_pipe, stderr.clone(), max_output_bytes);
        }

        Ok(Self {
            process_id,
            command,
            description,
            started_at: Instant::now(),
            timeout_ms,
            max_output_bytes,
            child: tokio::sync::Mutex::new(Some(child)),
            stdin_handle: tokio::sync::Mutex::new(stdin_handle),
            stdout,
            stderr,
            state: std::sync::Mutex::new(ProcState::running()),
        })
    }

    fn snapshot_state(&self) -> ProcState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    async fn write_stdin(&self, data: &str) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        let mut handle = self.stdin_handle.lock().await;
        let Some(stdin) = handle.as_mut() else {
            anyhow::bail!("stdin is not available (process may have closed it or finished)");
        };
        stdin.write_all(data.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn cancel_inner(&self) {
        self.refresh_status().await;
        let mut child = self.child.lock().await;
        if let Some(child_ref) = child.as_mut() {
            let _ = child_ref.kill().await;
            let _ = child_ref.wait().await;
        }
        *child = None;
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.is_final() {
                *state = ProcState::cancelled();
            }
        }
    }

    async fn cancel(&self, invocation_id: String) -> ToolResult {
        self.cancel_inner().await;
        helpers::ok(invocation_id, self.observation_json().await)
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
        helpers::ok(invocation_id, self.observation_json().await)
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
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                *state = ProcState::completed(status.success(), status.code());
            }
            Ok(None) if timed_out => {
                let _ = child_ref.kill().await;
                let _ = child_ref.wait().await;
                *child = None;
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                *state = ProcState::timed_out();
            }
            Ok(None) => {}
            Err(err) => {
                *child = None;
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                *state = ProcState::failed(format!("failed to poll process: {err}"));
            }
        }
    }

    async fn snapshot_json(&self) -> Value {
        let state = self.snapshot_state();
        let stdout = String::from_utf8_lossy(&self.stdout.lock().await).into_owned();
        let stderr = String::from_utf8_lossy(&self.stderr.lock().await).into_owned();
        let mut output = json!({
            "process_id": self.process_id,
            "command": self.command,
            "description": self.description,
            "background": true,
            "status": state.status.to_string(),
            "elapsed_ms": self.started_at.elapsed().as_millis() as u64,
            "timeout_ms": self.timeout_ms,
            "stdout": helpers::truncate_string(stdout, self.max_output_bytes),
            "stderr": helpers::truncate_string(stderr, self.max_output_bytes),
        });
        if let Some(code) = state.exit_code {
            output["exit_code"] = json!(code);
        }
        if let Some(error) = state.error {
            output["error"] = json!(error);
        }
        output
    }

    async fn observation_json(&self) -> Value {
        let state = self.snapshot_state();
        let mut value = self.snapshot_json().await;
        if !state.is_final() {
            value["message"] = json!(format!(
                "Process is still running. Wait with process({{\"action\":\"wait\",\"process_id\":\"{}\"}}) or cancel with process({{\"action\":\"cancel\",\"process_id\":\"{}\"}}).",
                self.process_id, self.process_id
            ));
        }
        value
    }
}

// ── Output reader ──────────────────────────────────────────────────────────────

fn spawn_output_reader<R>(mut reader: R, output: Arc<tokio::sync::Mutex<Vec<u8>>>, max_bytes: usize)
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
            if data.len() < max_bytes {
                let remaining = max_bytes - data.len();
                data.extend_from_slice(&buf[..n.min(remaining)]);
            }
        }
    });
}

// ── Command splitting ──────────────────────────────────────────────────────────

/// Splits a command string into program and args using shell-like word splitting.
/// Handles single-quoted and double-quoted strings. The first token is the
/// program name; the rest are arguments. Pipe/redirect/semicolon metacharacters
/// cause the split to stop (no shell pipeline support).
fn split_command(command: &str) -> (String, Vec<String>) {
    let command = command.trim();
    let tokens = split_shell_words(command);
    match tokens {
        Some(mut t) if !t.is_empty() => {
            let program = t.remove(0);
            (program, t)
        }
        _ => (String::new(), vec![]),
    }
}

/// Shell-like word splitting that handles single and double quotes.
/// Returns None when shell metacharacters (|, &, ;, >, <) are encountered.
fn split_shell_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut chars = command.chars().peekable();

    while let Some(ch) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            ' ' | '\t' | '\n' if !current.is_empty() => {
                words.push(std::mem::take(&mut current));
                while chars.peek().is_some_and(|next| next.is_whitespace()) {
                    chars.next();
                }
            }
            ' ' | '\t' | '\n' => {}
            '|' | '&' | ';' | '>' | '<' => return None,
            _ => current.push(ch),
        }
    }

    if escaped || quote.is_some() {
        return None;
    }
    if !current.is_empty() {
        words.push(current);
    }
    Some(words)
}

// ── Foreground execution ──────────────────────────────────────────────────────

async fn run_foreground(
    command: &str,
    timeout_ms: u64,
    max_output_bytes: usize,
    project_root: &PathBuf,
    invocation_id: String,
) -> Result<ToolResult> {
    let (program, args) = split_command(command);
    if program.is_empty() {
        return Ok(ToolResult {
            invocation_id,
            ok: false,
            output: json!({ "error": "empty command" }),
        });
    }

    let mut child = tokio::process::Command::new(&program)
        .args(&args)
        .current_dir(project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to spawn process: {command}"))?;

    let stdout_data = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let stderr_data = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    let stdout = child.stdout.take().context("stdout was not piped")?;
    let stderr = child.stderr.take().context("stderr was not piped")?;
    spawn_output_reader(stdout, stdout_data.clone(), max_output_bytes);
    spawn_output_reader(stderr, stderr_data.clone(), max_output_bytes);

    let timeout_duration = Duration::from_millis(timeout_ms);
    let status_result = tokio::time::timeout(timeout_duration, child.wait()).await;

    let (ok, status_code, error_msg) = match status_result {
        Ok(Ok(status)) => (status.success(), status.code(), None),
        Ok(Err(e)) => (
            false,
            None,
            Some(format!("failed to wait for process: {e}")),
        ),
        Err(_) => (
            false,
            None,
            Some("process timed out: deadline has elapsed".to_string()),
        ),
    };

    // Give readers a moment to drain remaining output.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let stdout_bytes = stdout_data.lock().await.clone();
    let stderr_bytes = stderr_data.lock().await.clone();

    let stdout_str = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let stderr_str = String::from_utf8_lossy(&stderr_bytes).into_owned();

    if let Some(err) = error_msg {
        Ok(ToolResult {
            invocation_id,
            ok: false,
            output: json!({
                "error": err,
                "stdout": helpers::truncate_string(stdout_str, max_output_bytes),
                "stderr": helpers::truncate_string(stderr_str, max_output_bytes),
            }),
        })
    } else {
        Ok(ToolResult {
            invocation_id,
            ok,
            output: json!({
                "exit_code": status_code,
                "stdout": helpers::truncate_string(stdout_str, max_output_bytes),
                "stderr": helpers::truncate_string(stderr_str, max_output_bytes),
            }),
        })
    }
}

// ── Tool trait ─────────────────────────────────────────────────────────────────

#[async_trait]
impl Tool for ProcessTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "process",
            "Execute commands as child processes and manage their lifecycle. \
Supports foreground execution, writing to stdin, waiting, listing, and cancelling. \
Commands are run without a shell.",
            ToolKind::Command,
            process_json_schema(),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let action = helpers::optional_string(&invocation.input, "action")
            .unwrap_or_else(|| "exec".to_string());

        match action.as_str() {
            "list" => {
                return Ok(self.registry.list(invocation.id).await);
            }
            "cancel" => {
                let process_id = helpers::required_string(&invocation.input, "process_id")?;
                let process = self.registry.get(process_id).await;
                let Some(process) = process else {
                    return Ok(ToolResult {
                        invocation_id: invocation.id,
                        ok: false,
                        output: json!({ "error": format!("unknown process `{process_id}`") }),
                    });
                };
                return Ok(process.cancel(invocation.id).await);
            }
            "wait" => {
                let process_id = helpers::required_string(&invocation.input, "process_id")?;
                let process = self.registry.get(process_id).await;
                let Some(process) = process else {
                    return Ok(ToolResult {
                        invocation_id: invocation.id,
                        ok: false,
                        output: json!({ "error": format!("unknown process `{process_id}`") }),
                    });
                };
                let wait_ms = helpers::optional_u64(&invocation.input, "wait_ms")
                    .unwrap_or(DEFAULT_WAIT_MS)
                    .min(MAX_WAIT_MS);
                return Ok(process.observe(wait_ms, invocation.id).await);
            }
            "stdin" => {
                let process_id = helpers::required_string(&invocation.input, "process_id")?;
                let data = helpers::required_string(&invocation.input, "stdin_data")?;
                let process = self.registry.get(process_id).await;
                let Some(process) = process else {
                    return Ok(ToolResult {
                        invocation_id: invocation.id,
                        ok: false,
                        output: json!({ "error": format!("unknown process `{process_id}`") }),
                    });
                };
                match process.write_stdin(data).await {
                    Ok(()) => Ok(helpers::ok(
                        invocation.id,
                        json!({"process_id": process_id, "bytes_written": data.len()}),
                    )),
                    Err(e) => Ok(ToolResult {
                        invocation_id: invocation.id,
                        ok: false,
                        output: json!({"error": format!("{e:#}")}),
                    }),
                }
            }
            _ => {
                // "exec" action (default)
                let command = helpers::required_string(&invocation.input, "command")?;

                if helpers::optional_bool(&invocation.input, "background").unwrap_or(false) {
                    let timeout_ms = helpers::optional_u64(&invocation.input, "timeout_ms")
                        .unwrap_or(DEFAULT_BG_TIMEOUT_MS)
                        .min(MAX_BG_TIMEOUT_MS);
                    let wait_ms = helpers::optional_u64(&invocation.input, "wait_ms")
                        .unwrap_or(DEFAULT_WAIT_MS)
                        .min(MAX_WAIT_MS);

                    let process = self
                        .registry
                        .spawn(
                            command.to_string(),
                            helpers::optional_string(&invocation.input, "description"),
                            self.project_root.clone(),
                            timeout_ms,
                            self.config.max_output_bytes,
                            self.config.max_processes,
                        )
                        .await
                        .map_err(|e| anyhow::anyhow!("{e:#}"))?;

                    return Ok(process.observe(wait_ms, invocation.id).await);
                }

                let timeout_ms = helpers::optional_u64(&invocation.input, "timeout_ms")
                    .unwrap_or(self.config.default_timeout_ms)
                    .min(MAX_TIMEOUT_MS);

                run_foreground(
                    command,
                    timeout_ms,
                    self.config.max_output_bytes,
                    &self.project_root,
                    invocation.id,
                )
                .await
            }
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::runtime::Runtime;

    fn rt() -> Runtime {
        tokio::runtime::Runtime::new().unwrap()
    }

    fn test_tool() -> ProcessTool {
        ProcessTool::new(PathBuf::from("/tmp"))
    }

    // ── exec returns structured output ─────────────────────────────────────────

    #[test]
    fn exec_returns_structured_output() {
        let result = rt().block_on(async {
            let tool = test_tool();
            let inv = ToolInvocation {
                id: "test-1".into(),
                tool_name: "process".into(),
                input: json!({"action": "exec", "command": "echo hello world"}),
            };
            tool.invoke(inv).await.unwrap()
        });

        assert!(result.ok, "exec should succeed");
        assert_eq!(result.output["exit_code"], 0);
        let stdout = result.output["stdout"].as_str().unwrap();
        assert!(
            stdout.contains("hello world"),
            "stdout should contain output"
        );
    }

    #[test]
    fn exec_captures_stderr() {
        let result = rt().block_on(async {
            let tool = test_tool();
            let inv = ToolInvocation {
                id: "test-2".into(),
                tool_name: "process".into(),
                input: json!({"action": "exec", "command": "sh -c 'echo errmsg >&2'"}),
            };
            tool.invoke(inv).await.unwrap()
        });

        assert!(result.ok, "exec should succeed");
        let stderr = result.output["stderr"].as_str().unwrap();
        assert!(stderr.contains("errmsg"), "stderr should contain output");
    }

    #[test]
    fn exec_reports_nonzero_exit() {
        let result = rt().block_on(async {
            let tool = test_tool();
            let inv = ToolInvocation {
                id: "test-3".into(),
                tool_name: "process".into(),
                input: json!({"action": "exec", "command": "sh -c 'exit 42'"}),
            };
            tool.invoke(inv).await.unwrap()
        });

        assert!(!result.ok, "exec with nonzero exit should report failure");
        assert_eq!(result.output["exit_code"], 42);
    }

    #[test]
    fn exec_timeout_returns_error() {
        let result = rt().block_on(async {
            let tool = test_tool();
            let inv = ToolInvocation {
                id: "test-4".into(),
                tool_name: "process".into(),
                input: json!({"action": "exec", "command": "sleep 10", "timeout_ms": 100}),
            };
            tool.invoke(inv).await.unwrap()
        });

        assert!(!result.ok, "timed out exec should report failure");
        assert!(
            result.output["error"]
                .as_str()
                .unwrap()
                .contains("timed out"),
            "error should mention timeout"
        );
    }

    // ── list returns running processes ─────────────────────────────────────────

    #[test]
    fn list_returns_processes() {
        let result = rt().block_on(async {
            let tool = test_tool();
            // Start a background process
            let start = ToolInvocation {
                id: "test-list-1".into(),
                tool_name: "process".into(),
                input: json!({"action": "exec", "command": "sleep 5", "background": true, "wait_ms": 0}),
            };
            let _start_result = tool.invoke(start).await.unwrap();

            // List processes
            let list_inv = ToolInvocation {
                id: "test-list-2".into(),
                tool_name: "process".into(),
                input: json!({"action": "list"}),
            };
            tool.invoke(list_inv).await.unwrap()
        });

        assert!(result.ok, "list should succeed");
        let processes = result.output["processes"].as_array().unwrap();
        assert!(!processes.is_empty(), "should have at least one process");
        assert_eq!(processes[0]["status"], "running");
    }

    #[test]
    fn list_returns_empty_when_none_running() {
        let result = rt().block_on(async {
            let tool = test_tool();
            let inv = ToolInvocation {
                id: "test-list-empty".into(),
                tool_name: "process".into(),
                input: json!({"action": "list"}),
            };
            tool.invoke(inv).await.unwrap()
        });

        assert!(result.ok);
        let processes = result.output["processes"].as_array().unwrap();
        assert!(processes.is_empty(), "should have no processes");
    }

    // ── cancel works ───────────────────────────────────────────────────────────

    #[test]
    fn cancel_terminates_background_process() {
        let result = rt().block_on(async {
            let tool = test_tool();

            // Start a long-running background process
            let start = ToolInvocation {
                id: "test-cancel-1".into(),
                tool_name: "process".into(),
                input: json!({"action": "exec", "command": "sleep 60", "background": true, "wait_ms": 0}),
            };
            let start_result = tool.invoke(start).await.unwrap();
            let process_id = start_result.output["process_id"]
                .as_str()
                .unwrap()
                .to_string();

            // Cancel it
            let cancel_inv = ToolInvocation {
                id: "test-cancel-2".into(),
                tool_name: "process".into(),
                input: json!({"action": "cancel", "process_id": process_id}),
            };
            let cancel_result = tool.invoke(cancel_inv).await.unwrap();
            assert!(cancel_result.ok, "cancel should succeed");
            assert_eq!(cancel_result.output["status"], "cancelled");
            cancel_result
        });

        assert_eq!(result.output["status"], "cancelled");
    }

    #[test]
    fn cancel_unknown_process_returns_error() {
        let result = rt().block_on(async {
            let tool = test_tool();
            let inv = ToolInvocation {
                id: "test-cancel-unknown".into(),
                tool_name: "process".into(),
                input: json!({"action": "cancel", "process_id": "nonexistent"}),
            };
            tool.invoke(inv).await.unwrap()
        });

        assert!(!result.ok);
        assert!(result.output["error"].as_str().unwrap().contains("unknown"));
    }

    // ── quotas are enforced ────────────────────────────────────────────────────

    #[test]
    fn quotas_are_enforced() {
        let result = rt().block_on(async {
            let tool = ProcessTool::with_config(
                PathBuf::from("/tmp"),
                ProcessManager::new(2, 30_000, 65536),
            );

            // Start max concurrent processes (2)
            let inv1 = ToolInvocation {
                id: "test-quota-1".into(),
                tool_name: "process".into(),
                input: json!({"action": "exec", "command": "sleep 10", "background": true, "wait_ms": 0}),
            };
            let _r1 = tool.invoke(inv1).await.unwrap();

            let inv2 = ToolInvocation {
                id: "test-quota-2".into(),
                tool_name: "process".into(),
                input: json!({"action": "exec", "command": "sleep 10", "background": true, "wait_ms": 0}),
            };
            let _r2 = tool.invoke(inv2).await.unwrap();

            // Third should be rejected
            let inv3 = ToolInvocation {
                id: "test-quota-3".into(),
                tool_name: "process".into(),
                input: json!({"action": "exec", "command": "sleep 10", "background": true, "wait_ms": 0}),
            };
            tool.invoke(inv3).await
        });

        assert!(result.is_err(), "quota should produce an error from invoke");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("too many"),
            "error should mention quota: {err}"
        );
    }

    // ── stdin writes to running process ────────────────────────────────────────

    #[test]
    fn stdin_writes_to_process() {
        let result = rt().block_on(async {
            let tool = test_tool();

            // A process that reads a line from stdin and echoes it back
            let start = ToolInvocation {
                id: "test-stdin-1".into(),
                tool_name: "process".into(),
                input: json!({"action": "exec", "command": "sh -c 'read line; echo \"got: $line\"'", "background": true, "wait_ms": 0}),
            };
            let start_result = tool.invoke(start).await.unwrap();
            let process_id = start_result.output["process_id"]
                .as_str()
                .unwrap()
                .to_string();

            // Wait a moment for process to start
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Write to stdin
            let stdin_inv = ToolInvocation {
                id: "test-stdin-2".into(),
                tool_name: "process".into(),
                input: json!({"action": "stdin", "process_id": process_id, "stdin_data": "hello\n"}),
            };
            let stdin_result = tool.invoke(stdin_inv).await.unwrap();
            assert!(stdin_result.ok, "stdin write should succeed");
            assert_eq!(stdin_result.output["bytes_written"], 6);

            // Wait for process to finish and check output
            let wait_inv = ToolInvocation {
                id: "test-stdin-3".into(),
                tool_name: "process".into(),
                input: json!({"action": "wait", "process_id": process_id, "wait_ms": 3000}),
            };
            let wait_result = tool.invoke(wait_inv).await.unwrap();
            assert_eq!(wait_result.output["status"], "completed");
            let stdout = wait_result.output["stdout"].as_str().unwrap_or("");
            assert!(stdout.contains("got: hello"), "stdout should contain echoed input");
            wait_result
        });

        let stdout = result.output["stdout"].as_str().unwrap_or("");
        assert!(
            stdout.contains("got: hello"),
            "stdout should contain 'got: hello'"
        );
    }

    // ── wait for process completion ────────────────────────────────────────────

    #[test]
    fn wait_for_process_completion() {
        let result = rt().block_on(async {
            let tool = test_tool();

            // Start a short background process
            let start = ToolInvocation {
                id: "test-wait-1".into(),
                tool_name: "process".into(),
                input: json!({"action": "exec", "command": "echo done", "background": true, "wait_ms": 5000}),
            };
            let start_result = tool.invoke(start).await.unwrap();
            assert_eq!(start_result.output["status"], "completed");
            assert_eq!(start_result.output["stdout"].as_str().unwrap_or(""), "done\n");
            start_result
        });

        assert_eq!(result.output["status"], "completed");
    }

    // ── exec with no action defaults to exec ───────────────────────────────────

    #[test]
    fn default_action_is_exec() {
        let result = rt().block_on(async {
            let tool = test_tool();
            let inv = ToolInvocation {
                id: "test-default".into(),
                tool_name: "process".into(),
                input: json!({"command": "echo default exec"}),
            };
            tool.invoke(inv).await.unwrap()
        });

        assert!(result.ok);
        let stdout = result.output["stdout"].as_str().unwrap();
        assert!(stdout.contains("default exec"));
    }

    // ── split_command ──────────────────────────────────────────────────────────

    #[test]
    fn split_command_basic() {
        let (prog, args) = split_command("echo hello world");
        assert_eq!(prog, "echo");
        assert_eq!(args, vec!["hello", "world"]);
    }

    #[test]
    fn split_command_single() {
        let (prog, args) = split_command("ls");
        assert_eq!(prog, "ls");
        assert!(args.is_empty());
    }

    #[test]
    fn split_command_empty() {
        let (prog, args) = split_command("");
        assert_eq!(prog, "");
        assert!(args.is_empty());
    }

    #[test]
    fn split_command_whitespace() {
        let (prog, args) = split_command("  cat   -n   file.txt  ");
        assert_eq!(prog, "cat");
        assert_eq!(args, vec!["-n", "file.txt"]);
    }
}
