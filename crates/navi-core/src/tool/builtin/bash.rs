use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

const BASH_DEFAULT_TIMEOUT_MS: u64 = 30_000;
const BASH_MAX_TIMEOUT_MS: u64 = 120_000;
const BASH_DEFAULT_BACKGROUND_TIMEOUT_MS: u64 = 600_000;
const BASH_MAX_BACKGROUND_TIMEOUT_MS: u64 = 1_800_000;
const BASH_DEFAULT_WAIT_MS: u64 = 15_000;
const BASH_MAX_WAIT_MS: u64 = 60_000;
const BASH_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
const BASH_MAX_BACKGROUND_TASKS: usize = 8;

pub(crate) struct BashTool {
    background: Arc<BashBackgroundRegistry>,
    project_root: PathBuf,
}

impl BashTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self {
            background: Arc::new(BashBackgroundRegistry::default()),
            project_root,
        }
    }
}

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
        project_root: PathBuf,
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
            project_root,
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
        helpers::ok(invocation_id, json!({ "tasks": values }))
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
        project_root: PathBuf,
        timeout_ms: u64,
    ) -> Result<Self> {
        let mut child = tokio::process::Command::new("bash")
            .arg("-lc")
            .arg(&command)
            .current_dir(&project_root)
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
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
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

    async fn cancel(&self, invocation_id: String) -> ToolResult {
        let mut child = self.child.lock().await;
        if let Some(child) = child.as_mut() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        *child = None;
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.is_final() {
                *state = BashBackgroundState::cancelled();
            }
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
                *state = BashBackgroundState::completed(status.success(), status.code());
            }
            Ok(None) if timed_out => {
                let _ = child_ref.kill().await;
                let _ = child_ref.wait().await;
                *child = None;
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                *state = BashBackgroundState::timed_out();
            }
            Ok(None) => {}
            Err(err) => {
                *child = None;
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
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
            "status": state.status.to_string(),
            "elapsed_ms": self.started_at.elapsed().as_millis() as u64,
            "timeout_ms": self.timeout_ms,
            "stdout": helpers::truncate_string(stdout, BASH_OUTPUT_LIMIT_BYTES),
            "stderr": helpers::truncate_string(stderr, BASH_OUTPUT_LIMIT_BYTES),
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum BashTaskStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
}

impl std::fmt::Display for BashTaskStatus {
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

impl BashTaskStatus {
    fn is_final(&self) -> bool {
        *self != Self::Running
    }
}

#[derive(Clone)]
struct BashBackgroundState {
    status: BashTaskStatus,
    exit_code: Option<i32>,
    error: Option<String>,
}

impl BashBackgroundState {
    fn running() -> Self {
        Self {
            status: BashTaskStatus::Running,
            exit_code: None,
            error: None,
        }
    }

    fn completed(ok: bool, exit_code: Option<i32>) -> Self {
        Self {
            status: if ok {
                BashTaskStatus::Completed
            } else {
                BashTaskStatus::Failed
            },
            exit_code,
            error: None,
        }
    }

    fn failed(error: String) -> Self {
        Self {
            status: BashTaskStatus::Failed,
            exit_code: None,
            error: Some(error),
        }
    }

    fn timed_out() -> Self {
        Self {
            status: BashTaskStatus::TimedOut,
            exit_code: None,
            error: Some("command timed out: deadline has elapsed".to_string()),
        }
    }

    fn cancelled() -> Self {
        Self {
            status: BashTaskStatus::Cancelled,
            exit_code: None,
            error: Some("command cancelled".to_string()),
        }
    }

    fn is_final(&self) -> bool {
        self.status.is_final()
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
        helpers::definition(
            "bash",
            "Run an ad-hoc shell command in the current project. Common git, test, build, grep, ls/find, and package-manager commands are not executed here; bash returns a native_tool_available suggestion so the agent can call the structured native tool instead. Use background=true and wait_ms for long-running ad-hoc commands, then poll or cancel with task_id.",
            ToolKind::Command,
            helpers::bash_json_schema(),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let action = helpers::optional_string(&invocation.input, "action");
        if action.as_deref() == Some("list") {
            return Ok(self.background.list(invocation.id).await);
        }

        if let Some(task_id) = helpers::optional_string(&invocation.input, "task_id") {
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
            let wait_ms = helpers::optional_u64(&invocation.input, "wait_ms")
                .unwrap_or(BASH_DEFAULT_WAIT_MS)
                .min(BASH_MAX_WAIT_MS);
            return Ok(task.observe(wait_ms, invocation.id).await);
        }

        let command = helpers::required_string(&invocation.input, "command")?;
        if let Some(suggestion) = native_tool_suggestion(command) {
            return Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: suggestion,
            });
        }
        if helpers::optional_bool(&invocation.input, "background").unwrap_or(false) {
            let timeout_ms = helpers::optional_u64(&invocation.input, "timeout_ms")
                .unwrap_or(BASH_DEFAULT_BACKGROUND_TIMEOUT_MS)
                .min(BASH_MAX_BACKGROUND_TIMEOUT_MS);
            let wait_ms = helpers::optional_u64(&invocation.input, "wait_ms")
                .unwrap_or(BASH_DEFAULT_WAIT_MS)
                .min(BASH_MAX_WAIT_MS);
            let task = self
                .background
                .spawn_task(
                    command.to_string(),
                    helpers::optional_string(&invocation.input, "description"),
                    self.project_root.clone(),
                    timeout_ms,
                )
                .await?;
            return Ok(task.observe(wait_ms, invocation.id).await);
        }

        let timeout_ms = helpers::optional_u64(&invocation.input, "timeout_ms")
            .unwrap_or(BASH_DEFAULT_TIMEOUT_MS)
            .min(BASH_MAX_TIMEOUT_MS);

        self.run_foreground(command, timeout_ms, invocation.id)
            .await
    }
}

fn native_tool_suggestion(command: &str) -> Option<Value> {
    let argv = split_shell_words(command)?;
    let program = argv.first()?.as_str();

    let suggestion = match program {
        "git" => suggest_git(&argv[1..])?,
        "cargo" => suggest_cargo(&argv[1..])?,
        "go" => suggest_go(&argv[1..])?,
        "npm" | "bun" => suggest_js_package_manager(program, &argv[1..])?,
        "rg" | "grep" => suggest_grep(program, &argv[1..])?,
        "ls" => suggest_list(&argv[1..]),
        "find" => suggest_find(&argv[1..]),
        _ => return None,
    };

    Some(json!({
        "error": "native_tool_available",
        "message": "This common shell command was not executed. Use the suggested native tool for structured output.",
        "original_command": command,
        "native_tool": suggestion.tool,
        "native_input": suggestion.input,
        "recoverable": true,
    }))
}

struct NativeSuggestion {
    tool: &'static str,
    input: Value,
}

fn suggest_git(args: &[String]) -> Option<NativeSuggestion> {
    let subcommand = args.first()?;
    let mapped = match subcommand.as_str() {
        "status" | "diff" | "log" | "branch" | "stash" | "remote" | "add" | "commit"
        | "restore" | "checkout" | "merge" | "rebase" | "pull" | "fetch" | "reset" | "clean"
        | "tag" | "rm" | "mv" | "init" | "clone" => subcommand.as_str(),
        "push"
            if args
                .iter()
                .any(|arg| matches!(arg.as_str(), "--force" | "-f")) =>
        {
            "push-force"
        }
        "push" if args.iter().any(|arg| arg == "--delete") => "push_delete",
        "push" => "push",
        _ => return None,
    };
    let mut input = json!({
        "command": mapped,
        "args": args[1..],
    });
    if matches!(mapped, "diff" | "log") {
        input["format"] = json!("json");
    }
    Some(NativeSuggestion {
        tool: "git_ops",
        input,
    })
}

fn suggest_cargo(args: &[String]) -> Option<NativeSuggestion> {
    let subcommand = args.first()?;
    match subcommand.as_str() {
        "test" | "nextest" => Some(NativeSuggestion {
            tool: "test_runner",
            input: flags_input(&args[1..]),
        }),
        "check" | "build" => {
            let input = cargo_build_input(&args[1..])?;
            Some(NativeSuggestion {
                tool: "build_runner",
                input,
            })
        }
        "add" | "remove" | "update" => Some(NativeSuggestion {
            tool: "package_manager",
            input: package_manager_input(map_package_action(subcommand), "cargo", &args[1..]),
        }),
        _ => None,
    }
}

fn suggest_go(args: &[String]) -> Option<NativeSuggestion> {
    let subcommand = args.first()?;
    match subcommand.as_str() {
        "test" => Some(NativeSuggestion {
            tool: "test_runner",
            input: flags_input(&args[1..]),
        }),
        "mod"
            if args
                .get(1)
                .is_some_and(|arg| matches!(arg.as_str(), "download" | "tidy")) =>
        {
            Some(NativeSuggestion {
                tool: "package_manager",
                input: json!({ "action": "install", "manager": "go" }),
            })
        }
        "get" => Some(NativeSuggestion {
            tool: "package_manager",
            input: package_manager_input("add", "go", &args[1..]),
        }),
        _ => None,
    }
}

fn suggest_js_package_manager(program: &str, args: &[String]) -> Option<NativeSuggestion> {
    let subcommand = args.first()?;
    let manager = program;
    match subcommand.as_str() {
        "install" | "i" if args.len() == 1 => Some(NativeSuggestion {
            tool: "package_manager",
            input: json!({ "action": "install", "manager": manager }),
        }),
        "install" | "i" | "add" => Some(NativeSuggestion {
            tool: "package_manager",
            input: package_manager_input("add", manager, &args[1..]),
        }),
        "remove" | "rm" | "uninstall" => Some(NativeSuggestion {
            tool: "package_manager",
            input: package_manager_input("remove", manager, &args[1..]),
        }),
        "update" | "upgrade" => Some(NativeSuggestion {
            tool: "package_manager",
            input: package_manager_input("update", manager, &args[1..]),
        }),
        "test" => Some(NativeSuggestion {
            tool: "test_runner",
            input: flags_input(&args[1..]),
        }),
        _ => None,
    }
}

fn suggest_grep(program: &str, args: &[String]) -> Option<NativeSuggestion> {
    let mut values = args
        .iter()
        .filter(|arg| !arg.starts_with('-'))
        .cloned()
        .collect::<Vec<_>>();
    if program == "grep" {
        values.retain(|arg| arg != "-R" && arg != "-r");
    }
    let pattern = values.first()?.clone();
    let path = values.get(1).cloned().unwrap_or_else(|| ".".to_string());
    Some(NativeSuggestion {
        tool: "grep",
        input: json!({ "pattern": pattern, "path": path }),
    })
}

fn suggest_list(args: &[String]) -> NativeSuggestion {
    let path = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .cloned()
        .unwrap_or_else(|| ".".to_string());
    NativeSuggestion {
        tool: "fs_browser",
        input: json!({ "action": "list", "path": path }),
    }
}

fn suggest_find(args: &[String]) -> NativeSuggestion {
    let path = args.first().cloned().unwrap_or_else(|| ".".to_string());
    let pattern = args
        .windows(2)
        .find_map(|window| (window[0] == "-name").then(|| window[1].clone()));
    let mut input = json!({ "action": "find", "path": path });
    if let Some(pattern) = pattern {
        input["pattern"] = json!(pattern.trim_matches('*'));
    }
    NativeSuggestion {
        tool: "fs_browser",
        input,
    }
}

fn flags_input(args: &[String]) -> Value {
    if args.is_empty() {
        json!({})
    } else {
        json!({ "flags": args.join(" ") })
    }
}

fn cargo_build_input(args: &[String]) -> Option<Value> {
    let mut input = json!({});
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--release" => input["profile"] = json!("release"),
            "--features" | "-F" => {
                index += 1;
                input["features"] = json!(args.get(index)?.clone());
            }
            flag if flag.starts_with("--features=") => {
                input["features"] = json!(flag.trim_start_matches("--features="));
            }
            _ => return None,
        }
        index += 1;
    }
    Some(input)
}

fn package_manager_input(action: &str, manager: &str, args: &[String]) -> Value {
    let dev = args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-D" | "--dev" | "--save-dev"));
    let packages = args
        .iter()
        .filter(|arg| !arg.starts_with('-'))
        .cloned()
        .collect::<Vec<_>>();
    json!({
        "action": action,
        "manager": manager,
        "packages": packages,
        "dev": dev,
    })
}

fn map_package_action(action: &str) -> &'static str {
    match action {
        "add" => "add",
        "remove" => "remove",
        "update" => "update",
        _ => "install",
    }
}

fn split_shell_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote = None;
    let mut escaped = false;

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
        if let Some(active_quote) = quote {
            if ch == active_quote {
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
    (!words.is_empty()).then_some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_git_ops_for_git_diff() {
        let suggestion = native_tool_suggestion("git diff -- Cargo.toml").expect("suggestion");

        assert_eq!(suggestion["native_tool"], "git_ops");
        assert_eq!(suggestion["native_input"]["command"], "diff");
        assert_eq!(
            suggestion["native_input"]["args"],
            json!(["--", "Cargo.toml"])
        );
        assert_eq!(suggestion["native_input"]["format"], "json");
        assert_eq!(suggestion["recoverable"], true);
    }

    #[test]
    fn suggests_test_runner_for_cargo_test() {
        let suggestion = native_tool_suggestion("cargo test -p navi-core").expect("suggestion");

        assert_eq!(suggestion["native_tool"], "test_runner");
        assert_eq!(
            suggestion["native_input"],
            json!({ "flags": "-p navi-core" })
        );
    }

    #[test]
    fn suggests_grep_for_rg() {
        let suggestion =
            native_tool_suggestion("rg \"fn main\" crates/navi-core/src").expect("suggestion");

        assert_eq!(suggestion["native_tool"], "grep");
        assert_eq!(suggestion["native_input"]["pattern"], "fn main");
        assert_eq!(suggestion["native_input"]["path"], "crates/navi-core/src");
    }

    #[test]
    fn suggests_fs_browser_for_ls() {
        let suggestion = native_tool_suggestion("ls -la crates").expect("suggestion");

        assert_eq!(suggestion["native_tool"], "fs_browser");
        assert_eq!(
            suggestion["native_input"],
            json!({ "action": "list", "path": "crates" })
        );
    }

    #[test]
    fn suggests_schema_valid_build_runner_for_cargo_build() {
        let suggestion =
            native_tool_suggestion("cargo build --release --features simd").expect("suggestion");

        assert_eq!(suggestion["native_tool"], "build_runner");
        assert_eq!(suggestion["native_input"]["profile"], "release");
        assert_eq!(suggestion["native_input"]["features"], "simd");
        assert!(suggestion["native_input"].get("flags").is_none());
    }

    #[test]
    fn leaves_unsupported_native_command_variants_to_bash() {
        assert!(native_tool_suggestion("pnpm install").is_none());
        assert!(native_tool_suggestion("cargo check -p navi-core").is_none());
    }

    #[test]
    fn leaves_ad_hoc_shell_commands_to_bash() {
        assert!(native_tool_suggestion("printf 'hello'").is_none());
        assert!(native_tool_suggestion("git diff | less").is_none());
    }
}

impl BashTool {
    async fn run_foreground(
        &self,
        command: &str,
        timeout_ms: u64,
        invocation_id: String,
    ) -> Result<ToolResult> {
        let mut child = tokio::process::Command::new("bash")
            .arg("-lc")
            .arg(command)
            .current_dir(&self.project_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("failed to spawn bash")?;

        let stdout_data = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let stderr_data = Arc::new(tokio::sync::Mutex::new(Vec::new()));

        let stdout = child.stdout.take().context("stdout was not piped")?;
        let stderr = child.stderr.take().context("stderr was not piped")?;
        spawn_output_reader(stdout, stdout_data.clone());
        spawn_output_reader(stderr, stderr_data.clone());

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
                    "stdout": helpers::truncate_string(stdout_str, 64 * 1024),
                    "stderr": helpers::truncate_string(stderr_str, 64 * 1024),
                }),
            })
        } else {
            Ok(ToolResult {
                invocation_id,
                ok,
                output: json!({
                    "status": status_code,
                    "stdout": helpers::truncate_string(stdout_str, 64 * 1024),
                    "stderr": helpers::truncate_string(stderr_str, 64 * 1024),
                }),
            })
        }
    }
}
