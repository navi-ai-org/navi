use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::helpers;
use crate::event::{AgentEvent, SudoPasswordRequest, SudoPasswordResponse};
use crate::tool::{
    Tool, ToolDefinition, ToolInvocation, ToolInvocationContext, ToolKind, ToolResult,
};

const BASH_DEFAULT_TIMEOUT_MS: u64 = 30_000;
const BASH_MAX_TIMEOUT_MS: u64 = 120_000;
const BASH_DEFAULT_BACKGROUND_TIMEOUT_MS: u64 = 600_000;
const BASH_MAX_BACKGROUND_TIMEOUT_MS: u64 = 1_800_000;
const BASH_DEFAULT_WAIT_MS: u64 = 15_000;
const BASH_MAX_WAIT_MS: u64 = 60_000;
const BASH_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
const BASH_MAX_BACKGROUND_TASKS: usize = 8;

/// Put the child in its own process group so timeout kills the whole tree
/// (pipelines, subshells, grandchildren), not just the top-level bash.
#[cfg(unix)]
fn configure_process_group(cmd: &mut tokio::process::Command) {
    // SAFETY: called before spawn; setpgid(0,0) makes the child a group leader.
    // tokio::process::Command re-exports the same pre_exec hook as std.
    unsafe {
        cmd.pre_exec(|| {
            let _ = libc_setpgid(0, 0);
            Ok(())
        });
    }
}

/// On Windows, hide the console window for child processes.
///
/// When navi runs under ConPTY (e.g. via NAVI Desktop's node-pty), the
/// pseudoconsole is attached only to navi.exe itself. Grandchildren spawned
/// by the shell (git, cargo, etc.) do not inherit it and each gets a new
/// visible console window on the desktop. `CREATE_NO_WINDOW` prevents that
/// without affecting stdio pipes.
#[cfg(windows)]
fn configure_process_group(cmd: &mut tokio::process::Command) {
    // CREATE_NO_WINDOW = 0x08000000 — tokio's Command exposes creation_flags
    // as an inherent method on Windows (delegates to std::process::Command).
    cmd.creation_flags(0x0800_0000);
}

#[cfg(not(any(unix, windows)))]
fn configure_process_group(_cmd: &mut tokio::process::Command) {}

/// Owns a Win32 Job Object with `KILL_ON_JOB_CLOSE`. When the handle is closed
/// (via [`ProcessTreeGuard::kill_tree`] or Drop), every process in the job —
/// including grandchildren spawned by the shell — is terminated immediately.
///
/// This mirrors the Unix `kill -KILL -PGID` behavior: the shell process is
/// assigned to the job right after spawn, and all its children inherit the
/// job membership automatically.
#[cfg(windows)]
mod win_job {
    use std::io;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::OpenProcess;

    const PROCESS_SET_QUOTA: u32 = 0x0100;
    const PROCESS_TERMINATE: u32 = 0x0001;

    pub struct Job {
        handle: HANDLE,
    }

    // SAFETY: HANDLE is a kernel object handle, not a pointer to thread-local
    // data. The handle is owned exclusively by `Job` (created in `new`, closed
    // in `Drop`), and Win32 job APIs are thread-safe.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        /// Create a job object that kills all member processes when closed.
        pub fn new() -> io::Result<Self> {
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                    return Err(io::Error::last_os_error());
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    let err = io::Error::last_os_error();
                    CloseHandle(handle);
                    return Err(err);
                }
                Ok(Self { handle })
            }
        }

        /// Assign an already-spawned process (by PID) to this job.
        ///
        /// Children spawned by that process afterwards inherit job membership
        /// automatically, so the assign must happen before the shell spawns
        /// subprocesses. In practice the shell startup (DLL load, parse) takes
        /// far longer than the `OpenProcess` + `AssignProcessToJobObject` pair,
        /// so the race window is negligible.
        pub fn assign_pid(&self, pid: u32) -> io::Result<()> {
            unsafe {
                let proc_handle = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if proc_handle.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let ok = AssignProcessToJobObject(self.handle, proc_handle);
                CloseHandle(proc_handle);
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.handle) };
        }
    }
}

/// Guards the lifetime of a spawned shell's process tree.
///
/// On Windows, holds a Job Object handle — dropping it or calling
/// [`ProcessTreeGuard::kill_tree`] kills every process in the tree.
/// On Unix, the process group is managed via `setpgid` + `kill -PGID`
/// inside [`kill_timed_out_child`], so this guard is a no-op.
#[cfg(windows)]
pub(crate) struct ProcessTreeGuard {
    job: Option<win_job::Job>,
}

#[cfg(windows)]
impl ProcessTreeGuard {
    pub fn empty() -> Self {
        Self { job: None }
    }

    /// Create a guard and assign the just-spawned child to a new job.
    pub fn for_child(pid: u32) -> Self {
        match win_job::Job::new().and_then(|job| job.assign_pid(pid).map(|()| job)) {
            Ok(job) => Self { job: Some(job) },
            Err(err) => {
                tracing::debug!(%err, pid, "failed to assign child to job object");
                Self { job: None }
            }
        }
    }

    /// Kill the entire process tree by closing the job handle.
    pub fn kill_tree(&mut self) {
        self.job = None;
    }
}

#[cfg(windows)]
impl Default for ProcessTreeGuard {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(not(windows))]
pub(crate) struct ProcessTreeGuard;

#[cfg(not(windows))]
impl ProcessTreeGuard {
    pub fn empty() -> Self {
        Self
    }

    pub fn for_child(_pid: u32) -> Self {
        Self
    }

    pub fn kill_tree(&mut self) {}
}

#[cfg(not(windows))]
impl Default for ProcessTreeGuard {
    fn default() -> Self {
        Self
    }
}

/// Resolve the path to `bash.exe` on Windows (Git Bash / MSYS2 / WSL).
///
/// Returns `Some(path)` when found on PATH or in well-known install
/// locations, so sessions started from PowerShell/cmd still get the familiar
/// bash behavior when Git for Windows is installed.
#[cfg(windows)]
fn find_windows_bash() -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    // 1) On PATH (Git Bash session, MSYS2 console, etc.)
    if Command::new("where")
        .arg("bash.exe")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .status()
        .is_ok_and(|s| s.success())
    {
        return Some(PathBuf::from("bash"));
    }

    // 2) Well-known Git for Windows layouts (scoop, chocolatey, plain installer).
    for base in [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
        r"C:\Program Files\Git\usr\bin\bash.exe",
        r"C:\Program Files (x86)\Git\usr\bin\bash.exe",
    ] {
        if Path::new(base).is_file() {
            return Some(PathBuf::from(base));
        }
    }
    // 3) Scoop-installed Git (versioned path via junction resolution).
    let mut scoop_dirs = Vec::new();
    if let Ok(scoop) = std::env::var("SCOOP") {
        scoop_dirs.push(PathBuf::from(scoop));
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        scoop_dirs.push(PathBuf::from(home).join("scoop"));
    }
    for scoop in scoop_dirs {
        for entry in [
            "apps/git/current/bin/bash.exe",
            "apps/git/current/usr/bin/bash.exe",
        ] {
            let candidate = scoop.join(entry);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

#[cfg(not(windows))]
fn find_windows_bash() -> Option<PathBuf> {
    None
}

/// Check whether a program exists on PATH (Windows only).
#[cfg(windows)]
fn windows_program_exists(program: &str) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("where")
        .arg(program)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn windows_program_exists(_program: &str) -> bool {
    false
}

/// Returns true when the user has explicitly opted into a bash shell on
/// Windows via the `NAVI_BASH_SHELL` environment variable.
///
/// The default Windows shell is PowerShell (see [`windows_shell_program`]).
/// Users who have Git Bash / MSYS2 installed and want the familiar bash
/// behavior can set `NAVI_BASH_SHELL=bash` (or a path to a bash executable)
/// to restore the previous bash-based execution. This avoids routing to
/// WSL's `bash.exe` launcher, which fails with exit 1 when no distribution
/// is installed.
#[cfg(windows)]
fn windows_use_bash() -> bool {
    use_bash_from_override(std::env::var("NAVI_BASH_SHELL").ok().as_deref())
}

#[cfg(not(windows))]
fn windows_use_bash() -> bool {
    false
}

/// Pure helper: decide whether a given override value selects bash.
fn use_bash_from_override(override_val: Option<&str>) -> bool {
    override_val.map(|s| !s.trim().is_empty()).unwrap_or(false)
}

/// Select the shell program for Windows.
///
/// Default: PowerShell — `pwsh.exe` (PowerShell 7+) when available, otherwise
/// the built-in `powershell.exe`. PowerShell is always present on Windows, so
/// the bash tool works out of the box without WSL or Git Bash.
///
/// Override: set `NAVI_BASH_SHELL=bash` (or a path to a bash executable) to
/// use Git Bash / MSYS2, keeping backward compat for users who configured it.
#[cfg(windows)]
fn windows_shell_program() -> PathBuf {
    shell_program_from_override(std::env::var("NAVI_BASH_SHELL").ok().as_deref())
}

#[cfg(not(windows))]
fn windows_shell_program() -> PathBuf {
    PathBuf::from("bash")
}

/// Pure helper: resolve the shell program from an optional override value.
/// Split out so tests can exercise both the default (PowerShell) and the
/// bash opt-in paths without mutating the process-wide environment.
#[cfg(windows)]
fn shell_program_from_override(override_val: Option<&str>) -> PathBuf {
    if let Some(shell) = override_val {
        let trimmed = shell.trim();
        if !trimmed.is_empty() {
            // "bash" => resolve via well-known Git Bash locations / PATH.
            if trimmed.eq_ignore_ascii_case("bash") {
                return find_windows_bash().unwrap_or_else(|| PathBuf::from("bash"));
            }
            // Any other value is treated as an explicit path to a shell.
            return PathBuf::from(trimmed);
        }
    }
    // Default: PowerShell. Prefer PowerShell 7 (pwsh) when on PATH.
    if windows_program_exists("pwsh.exe") {
        PathBuf::from("pwsh")
    } else {
        PathBuf::from("powershell")
    }
}

/// Select the shell program + args for the current platform.
///
/// Unix: `bash -lc`.
/// Windows: PowerShell (`-NoProfile -Command`) by default, or `bash -lc` when
/// the user opts in via `NAVI_BASH_SHELL`.
fn shell_argv(shell_cmd: &str) -> Vec<&str> {
    if cfg!(windows) {
        if windows_use_bash() {
            vec!["-lc", shell_cmd]
        } else {
            vec!["-NoProfile", "-Command", shell_cmd]
        }
    } else {
        vec!["-lc", shell_cmd]
    }
}

/// Build the shell command for the current platform.
///
/// Unix: `bash -lc <command>`.
/// Windows: PowerShell (`-NoProfile -Command <command>`) by default, or
/// `bash -lc <command>` when the user opts in via `NAVI_BASH_SHELL`.
fn shell_command(shell_cmd: &str, project_root: &std::path::Path) -> tokio::process::Command {
    let mut cmd = if cfg!(windows) {
        let program = windows_shell_program();
        let mut c = tokio::process::Command::new(program);
        for arg in shell_argv(shell_cmd) {
            c.arg(arg);
        }
        c
    } else {
        let mut c = tokio::process::Command::new("bash");
        c.arg("-lc").arg(shell_cmd);
        c
    };
    cmd.current_dir(project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true);
    configure_process_group(&mut cmd);
    cmd
}

/// Kill a timed-out child. On Unix, signal the whole process group first.
/// On Windows, close the Job Object handle (kills the whole tree), then
/// `start_kill` as a belt-and-suspenders.
async fn kill_timed_out_child(child: &mut tokio::process::Child, guard: &mut ProcessTreeGuard) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // Negative pid => kill process group. SIGKILL so stuck tools cannot ignore it.
            let _ = std::process::Command::new("kill")
                .args(["-KILL", &format!("-{pid}")])
                .status();
        }
    }
    #[cfg(windows)]
    {
        guard.kill_tree();
    }
    let _ = child.start_kill();
    // Bound the wait so a wedged reaper cannot stall the tool loop forever.
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
}

#[cfg(unix)]
fn libc_setpgid(pid: i32, pgid: i32) -> i32 {
    // Thin wrapper so we do not take a libc crate dependency.
    // SAFETY: direct setpgid syscall for the current process at pre_exec time.
    unsafe { libc_setpgid_raw(pid, pgid) }
}

#[cfg(unix)]
unsafe fn libc_setpgid_raw(pid: i32, pgid: i32) -> i32 {
    // Use the libc crate if linked transitively; otherwise fall back to 0 (no-op).
    #[allow(unused_unsafe)]
    {
        unsafe extern "C" {
            fn setpgid(pid: i32, pgid: i32) -> i32;
        }
        unsafe { setpgid(pid, pgid) }
    }
}

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
        sudo_env: Option<SudoAskpassEnv>,
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
            sudo_env,
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
    /// Keeps askpass temp files alive until the task finishes.
    _sudo_askpass: Option<SudoAskpassEnv>,
    /// On Windows, owns the Job Object that kills the whole process tree on
    /// timeout/cancel. On Unix, this is a no-op (process group is used instead).
    tree_guard: tokio::sync::Mutex<ProcessTreeGuard>,
}

impl BashBackgroundTask {
    fn spawn(
        task_id: String,
        command: String,
        description: Option<String>,
        project_root: PathBuf,
        timeout_ms: u64,
        sudo_env: Option<SudoAskpassEnv>,
    ) -> Result<Self> {
        let (shell_cmd, _guard) = wrap_command_for_sudo(&command, sudo_env.as_ref())?;
        let mut cmd = shell_command(&shell_cmd, &project_root);
        let mut child = cmd.spawn().context("failed to spawn shell")?;
        let tree_guard = ProcessTreeGuard::for_child(child.id().unwrap_or(0));

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
            _sudo_askpass: sudo_env,
            tree_guard: tokio::sync::Mutex::new(tree_guard),
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
        let output = self.observation_json().await;
        let state = self.snapshot_state();
        // Timed-out / failed / cancelled background tasks must not report ok:true,
        // otherwise the agent loop treats them as success and can stall waiting
        // for a follow-up that never comes.
        let ok = match state.status {
            BashTaskStatus::Completed | BashTaskStatus::Running => true,
            BashTaskStatus::Failed | BashTaskStatus::TimedOut | BashTaskStatus::Cancelled => false,
        };
        ToolResult {
            invocation_id,
            ok,
            output,
        }
    }

    async fn cancel(&self, invocation_id: String) -> ToolResult {
        // Refresh status first to avoid race condition with completed tasks
        self.refresh_status().await;

        let mut child = self.child.lock().await;
        if let Some(child) = child.as_mut() {
            let mut guard = self.tree_guard.lock().await;
            kill_timed_out_child(child, &mut guard).await;
        }
        *child = None;
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.is_final() {
                *state = BashBackgroundState::cancelled();
            }
        }
        ToolResult {
            invocation_id,
            ok: false,
            output: self.observation_json().await,
        }
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
                let mut guard = self.tree_guard.lock().await;
                kill_timed_out_child(child_ref, &mut guard).await;
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
            "Run an ad-hoc shell command in the current project. On Unix uses bash; on Windows uses bash (if installed), PowerShell, or cmd as fallback. Common git, test, build, and file-read commands (cat/sed/head/rg/ls/find) are not executed here; bash returns a native_tool_available suggestion pointing at read_file/search. Use background=true and wait_ms for long-running commands. Commands using sudo open a secure password modal in the TUI — the password is never shown to the model. Never use bash to dump project source for inspection.",
            ToolKind::Command,
            helpers::bash_json_schema(),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        self.invoke_with_context(invocation, ToolInvocationContext::default())
            .await
    }

    async fn invoke_with_context(
        &self,
        invocation: ToolInvocation,
        context: ToolInvocationContext,
    ) -> Result<ToolResult> {
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

        // Interactive sudo: collect password via TUI modal (never in model context).
        let sudo_env = if command_likely_needs_sudo(command) {
            match request_sudo_password(&context, &invocation.id, command).await {
                Ok(Some(password)) => Some(prepare_sudo_askpass(&password)?),
                Ok(None) => {
                    return Ok(ToolResult {
                        invocation_id: invocation.id,
                        ok: false,
                        output: json!({
                            "error": "sudo password cancelled by user",
                            "hint": "Re-run without sudo or approve the password prompt.",
                        }),
                    });
                }
                Err(msg) => {
                    return Ok(ToolResult {
                        invocation_id: invocation.id,
                        ok: false,
                        output: json!({ "error": msg }),
                    });
                }
            }
        } else {
            None
        };

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
                    sudo_env,
                )
                .await?;
            return Ok(task.observe(wait_ms, invocation.id).await);
        }

        let timeout_ms = helpers::optional_u64(&invocation.input, "timeout_ms")
            .unwrap_or(BASH_DEFAULT_TIMEOUT_MS)
            .min(BASH_MAX_TIMEOUT_MS);

        self.run_foreground(command, timeout_ms, invocation.id, sudo_env)
            .await
    }
}

fn native_tool_suggestion(command: &str) -> Option<Value> {
    // Multi-command scripts often chain readers (sed/cat/head) with other ops.
    // Prefer redirecting when the *primary intent* is dumping project source.
    if let Some(suggestion) = suggest_file_read_command(command) {
        return Some(native_tool_error(command, suggestion));
    }

    let argv = split_shell_words(command)?;
    let program = argv.first()?.as_str();

    let suggestion = match program {
        "rg" | "grep" | "ag" | "ack" => suggest_grep(program, &argv[1..])?,
        "ls" => suggest_list(&argv[1..]),
        "find" => suggest_find(&argv[1..]),
        _ => return None,
    };

    Some(native_tool_error(command, suggestion))
}

fn native_tool_error(command: &str, suggestion: NativeSuggestion) -> Value {
    json!({
        "error": "native_tool_available",
        "message": "This common shell command was not executed. Use the suggested native tool instead of dumping files via bash (keeps the TUI clean and uses structured tools).",
        "original_command": command,
        "native_tool": suggestion.tool,
        "native_input": suggestion.input,
        "recoverable": true,
    })
}

/// Redirect shell file readers (sed/cat/head/tail/less/…) to `read_file`.
///
/// Matches both simple commands and common inspection idioms used by models:
/// `sed -n '380,560p' path`, `cat path`, `head -n 40 path`, `nl -ba path`, etc.
fn suggest_file_read_command(command: &str) -> Option<NativeSuggestion> {
    // For pipelines / chained commands (`cmd1; cmd2`, `a | b`), inspect each segment.
    for segment in split_shell_command_segments(command) {
        let Some(argv) = split_shell_words(segment) else {
            continue;
        };
        if let Some(suggestion) = suggest_file_read_argv(&argv) {
            return Some(suggestion);
        }
    }
    None
}

fn split_shell_command_segments(command: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0usize;
    let mut chars = command.char_indices().peekable();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    while let Some((idx, ch)) = chars.next() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        // Split on ; | && || when not quoted.
        if ch == ';' {
            let piece = command[start..idx].trim();
            if !piece.is_empty() {
                segments.push(piece);
            }
            start = idx + ch.len_utf8();
            continue;
        }
        if ch == '|' {
            // || or single |
            let is_or = chars.peek().is_some_and(|(_, n)| *n == '|');
            let piece = command[start..idx].trim();
            if !piece.is_empty() {
                segments.push(piece);
            }
            if is_or {
                let _ = chars.next();
                start = idx + 2;
            } else {
                start = idx + 1;
            }
            continue;
        }
        if ch == '&' && chars.peek().is_some_and(|(_, n)| *n == '&') {
            let piece = command[start..idx].trim();
            if !piece.is_empty() {
                segments.push(piece);
            }
            let _ = chars.next();
            start = idx + 2;
        }
    }
    let piece = command[start..].trim();
    if !piece.is_empty() {
        segments.push(piece);
    }
    if segments.is_empty() {
        segments.push(command);
    }
    segments
}

fn suggest_file_read_argv(argv: &[String]) -> Option<NativeSuggestion> {
    // strip env assignments: FOO=1 sed ...
    let (program, args) = strip_leading_env_assignments(argv)?;

    match program {
        "sed" => suggest_sed_read(args),
        "cat" | "bat" | "batcat" => suggest_cat_read(args),
        "head" | "tail" => suggest_head_tail_read(program, args),
        "less" | "more" | "most" => suggest_pager_read(args),
        "nl" => suggest_nl_read(args),
        "tac" => suggest_cat_read(args),
        "awk" => suggest_awk_read(args),
        // python -c "print(open('f').read())" is harder; leave for later.
        _ => None,
    }
}

fn strip_leading_env_assignments(argv: &[String]) -> Option<(&str, &[String])> {
    let mut idx = 0;
    while idx < argv.len() {
        let tok = &argv[idx];
        if tok.contains('=') && !tok.starts_with('-') && !tok.contains('/') {
            // FOO=bar style assignment
            if tok
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            {
                idx += 1;
                continue;
            }
        }
        break;
    }
    let program = argv.get(idx)?.as_str();
    Some((program, &argv[idx + 1..]))
}

fn looks_like_path(token: &str) -> bool {
    if token.is_empty() || token == "-" {
        return false;
    }
    if token.starts_with('-') {
        return false;
    }
    // Paths commonly contain / or a file extension, or are relative project files.
    token.contains('/')
        || token.contains('.')
        || token == "README"
        || token == "Makefile"
        || token == "Cargo.toml"
        || token == "justfile"
}

fn parse_sed_range(expr: &str) -> Option<(u64, u64)> {
    // Forms: 10,20p  |  10,20p;  |  '10,20p' already unquoted by splitter
    let expr = expr.trim().trim_matches(';');
    let expr = expr.strip_suffix('p').unwrap_or(expr);
    let expr = expr.strip_suffix('P').unwrap_or(expr);
    let (start, end) = expr.split_once(',')?;
    let start: u64 = start.trim().parse().ok()?;
    let end: u64 = end.trim().parse().ok()?;
    if start == 0 || end == 0 || end < start {
        return None;
    }
    Some((start, end))
}

fn suggest_sed_read(args: &[String]) -> Option<NativeSuggestion> {
    // sed [-n] 'START,ENDp' path...
    // Also: sed -n START,ENDp path
    let mut quiet = false;
    let mut range = None;
    let mut paths = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-n" || arg == "--quiet" || arg == "--silent" {
            quiet = true;
            i += 1;
            continue;
        }
        if (arg == "-e" || arg == "--expression")
            && let Some(expr) = args.get(i + 1)
        {
            if let Some(r) = parse_sed_range(expr) {
                range = Some(r);
            }
            i += 2;
            continue;
        }
        if arg == "-f" || arg == "--file" {
            // script file — not a project source dump we can map cleanly
            return None;
        }
        if arg == "-i" || arg.starts_with("-i") {
            // in-place edit: not a read dump
            return None;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        if range.is_none()
            && let Some(r) = parse_sed_range(arg)
        {
            range = Some(r);
            i += 1;
            continue;
        }
        if looks_like_path(arg) {
            paths.push(arg.clone());
        }
        i += 1;
    }

    let path = paths.first()?.clone();
    // Only redirect classic "print line range" dumps (with or without -n).
    if let Some((start, end)) = range {
        return Some(NativeSuggestion {
            tool: "read_file",
            input: json!({
                "path": path,
                "start_line": start,
                "end_line": end,
            }),
        });
    }

    // sed without range but with a path is ambiguous (could be transform).
    // Only redirect when -n is present with a simple print script missing — skip.
    let _ = quiet;
    None
}

fn first_path_arg(args: &[String]) -> Option<String> {
    args.iter()
        .filter(|arg| looks_like_path(arg))
        .find(|arg| !arg.starts_with('-'))
        .cloned()
}

fn suggest_cat_read(args: &[String]) -> Option<NativeSuggestion> {
    let path = first_path_arg(args)?;
    Some(NativeSuggestion {
        tool: "read_file",
        input: json!({ "path": path }),
    })
}

fn suggest_pager_read(args: &[String]) -> Option<NativeSuggestion> {
    suggest_cat_read(args)
}

fn suggest_nl_read(args: &[String]) -> Option<NativeSuggestion> {
    suggest_cat_read(args)
}

fn suggest_head_tail_read(program: &str, args: &[String]) -> Option<NativeSuggestion> {
    let mut n: Option<u64> = None;
    let mut path = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if (arg == "-n" || arg == "--lines")
            && let Some(v) = args.get(i + 1)
        {
            n = v.trim_start_matches('+').parse().ok();
            i += 2;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("-n")
            && !rest.is_empty()
        {
            n = rest.trim_start_matches('+').parse().ok();
            i += 1;
            continue;
        }
        // head -20 file / tail -20 file
        if arg.starts_with('-') && arg.len() > 1 && arg[1..].chars().all(|c| c.is_ascii_digit()) {
            n = arg[1..].parse().ok();
            i += 1;
            continue;
        }
        if looks_like_path(arg) {
            path = Some(arg.clone());
        }
        i += 1;
    }
    let path = path?;
    if program == "head" {
        let end = n.unwrap_or(20).max(1);
        return Some(NativeSuggestion {
            tool: "read_file",
            input: json!({ "path": path, "start_line": 1, "end_line": end }),
        });
    }
    // tail: without total line count we can't map exactly; still force read_file
    // and let the model re-range. Avoid dumping via bash.
    Some(NativeSuggestion {
        tool: "read_file",
        input: json!({ "path": path }),
    })
}

fn suggest_awk_read(args: &[String]) -> Option<NativeSuggestion> {
    // Only map trivial `{print}` / `{print $0}` file dumps.
    let mut script = None;
    let mut path = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-f" {
            return None;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        if script.is_none() {
            script = Some(arg.clone());
            i += 1;
            continue;
        }
        if looks_like_path(arg) {
            path = Some(arg.clone());
        }
        i += 1;
    }
    let script = script?;
    let path = path?;
    let compact: String = script.chars().filter(|c| !c.is_whitespace()).collect();
    if matches!(compact.as_str(), "{print}" | "{print$0}" | "1" | "{print;}") {
        return Some(NativeSuggestion {
            tool: "read_file",
            input: json!({ "path": path }),
        });
    }
    // NR ranges: NR>=10&&NR<=20{print}
    if let Some((start, end)) = parse_awk_nr_range(&compact) {
        return Some(NativeSuggestion {
            tool: "read_file",
            input: json!({
                "path": path,
                "start_line": start,
                "end_line": end,
            }),
        });
    }
    None
}

fn parse_awk_nr_range(compact: &str) -> Option<(u64, u64)> {
    // NR>=10&&NR<=20 or NR==10
    if let Some(rest) = compact.strip_prefix("NR>=") {
        let (a, rest) = rest.split_once("&&NR<=")?;
        let start: u64 = a.parse().ok()?;
        let end_part = rest.split('{').next()?.trim_end_matches('}');
        let end: u64 = end_part.parse().ok()?;
        return Some((start, end));
    }
    None
}

struct NativeSuggestion {
    tool: &'static str,
    input: Value,
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
        tool: "search",
        input: json!({
            "action": "grep",
            "pattern": pattern,
            "path": path,
        }),
    })
}

fn suggest_list(args: &[String]) -> NativeSuggestion {
    let path = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .cloned()
        .unwrap_or_else(|| ".".to_string());
    NativeSuggestion {
        tool: "search",
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
        tool: "search",
        input,
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

impl BashTool {
    async fn run_foreground(
        &self,
        command: &str,
        timeout_ms: u64,
        invocation_id: String,
        sudo_env: Option<SudoAskpassEnv>,
    ) -> Result<ToolResult> {
        let (shell_cmd, _guard) = wrap_command_for_sudo(command, sudo_env.as_ref())?;
        let mut cmd = shell_command(&shell_cmd, &self.project_root);
        let mut child = cmd.spawn().context("failed to spawn shell")?;
        let mut tree_guard = ProcessTreeGuard::for_child(child.id().unwrap_or(0));

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
            Err(_) => {
                // Explicitly kill the process group; do not rely only on Drop.
                kill_timed_out_child(&mut child, &mut tree_guard).await;
                (
                    false,
                    None,
                    Some("command timed out: deadline has elapsed".to_string()),
                )
            }
        };

        // Give readers a moment to drain remaining output.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = _guard;
        // Drop askpass env (deletes password file) after process ends.
        drop(sudo_env);

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

// ── Sudo password (TUI modal + SUDO_ASKPASS; secret never reaches the model) ─

/// Temp files + script for `sudo -A`. Dropped after the command finishes.
struct SudoAskpassEnv {
    dir: PathBuf,
    script_path: PathBuf,
    pass_path: PathBuf,
}

impl Drop for SudoAskpassEnv {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.pass_path);
        let _ = fs::remove_file(&self.script_path);
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn command_likely_needs_sudo(command: &str) -> bool {
    // Match `sudo` only as a *command* word (start of a simple command or after
    // shell control operators), not as a plain argument (`echo sudo is cool`).
    let mut at_command_position = true;
    for raw in command.split_whitespace() {
        let token = raw.trim_matches(|c: char| "\"'`".contains(c));
        if token.is_empty() {
            continue;
        }
        if at_command_position {
            // `env VAR=value sudo …` still leaves us in command position.
            if token.contains('=') && !token.starts_with('-') && !token.starts_with("sudo") {
                continue;
            }
            if is_sudo_token(token) {
                return true;
            }
            at_command_position = false;
        }
        // Next token is a new command after a shell operator.
        if is_shell_command_separator(token) {
            at_command_position = true;
            // `cmd;sudo` or `cmd|sudo` glued without spaces.
            if let Some(rest) = token.find(['|', ';', '&']).map(|i| &token[i + 1..]) {
                let rest = rest.trim_start_matches(['|', '&', ';']);
                if is_sudo_token(rest) {
                    return true;
                }
            }
        }
    }
    false
}

fn is_sudo_token(token: &str) -> bool {
    matches!(token, "sudo" | "/usr/bin/sudo" | "/bin/sudo") || token.ends_with("/sudo")
}

fn is_shell_command_separator(token: &str) -> bool {
    matches!(
        token,
        "|" | "||" | "&&" | ";" | "&" | "(" | ")" | "{" | "}" | "then" | "do" | "else" | "elif"
    ) || token.ends_with('|')
        || token.ends_with(';')
        || token.ends_with("&&")
        || token.ends_with("||")
}

fn summarize_command(command: &str) -> String {
    let one_line = command.lines().next().unwrap_or(command).trim();
    if one_line.chars().count() <= 80 {
        one_line.to_string()
    } else {
        let mut s: String = one_line.chars().take(77).collect();
        s.push('…');
        s
    }
}

async fn request_sudo_password(
    context: &ToolInvocationContext,
    invocation_id: &str,
    command: &str,
) -> Result<Option<String>, String> {
    let Some(resolver) = context.sudo_password_resolver.as_ref() else {
        return Err(
            "sudo requires an interactive TUI password prompt (no password resolver available)"
                .into(),
        );
    };
    let Some(tx) = context.event_tx.as_ref() else {
        return Err("sudo requires an interactive client".into());
    };

    let id = format!("sudo-{invocation_id}");
    let rx = resolver.register(id.clone());
    let _ = tx.send(AgentEvent::SudoPasswordRequested(SudoPasswordRequest {
        id: id.clone(),
        command_summary: summarize_command(command),
    }));

    let response = if let Some(cancel) = context.cancel_token.as_ref() {
        tokio::select! {
            r = rx => r.ok(),
            _ = cancel.notified() => None,
        }
    } else {
        rx.await.ok()
    };

    match response {
        Some(SudoPasswordResponse::Submitted { password, .. }) => Ok(Some(password)),
        Some(SudoPasswordResponse::Cancelled { .. }) | None => Ok(None),
    }
}

fn prepare_sudo_askpass(password: &str) -> Result<SudoAskpassEnv> {
    let dir = std::env::temp_dir().join(format!("navi-sudo-{}", fastrand::u64(..)));
    fs::create_dir_all(&dir).context("create temp dir for sudo askpass")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    let pass_path = dir.join("pass");
    let script_path = dir.join("askpass.sh");
    fs::write(&pass_path, format!("{password}\n")).context("write sudo password file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&pass_path, fs::Permissions::from_mode(0o600));
    }
    // Askpass: print password once, then delete the secret file immediately.
    let script = format!(
        "#!/bin/sh\ncat '{pass}' 2>/dev/null\nrm -f '{pass}'\n",
        pass = pass_path.display()
    );
    fs::write(&script_path, script).context("write sudo askpass script")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&script_path, fs::Permissions::from_mode(0o700));
    }
    Ok(SudoAskpassEnv {
        dir,
        script_path,
        pass_path,
    })
}

/// Wrap user command so every `sudo` becomes `sudo -A` with our askpass.
///
/// No-op on Windows: there is no `sudo` in PowerShell/cmd, and the askpass
/// helper relies on a POSIX shell.
#[cfg(unix)]
fn wrap_command_for_sudo(
    command: &str,
    sudo: Option<&SudoAskpassEnv>,
) -> Result<(String, Option<()>)> {
    let Some(env) = sudo else {
        return Ok((command.to_string(), None));
    };
    let askpass = env.script_path.display().to_string();
    // Function-based sudo wrapper works with bash -lc.
    let wrapped = format!(
        "export SUDO_ASKPASS={askpass:?}; \
         export SUDO_PROMPT=''; \
         sudo() {{ command sudo -A \"$@\"; }}; \
         export -f sudo; \
         {command}"
    );
    Ok((wrapped, Some(())))
}

/// Wrap user command so every `sudo` becomes `sudo -A` with our askpass.
///
/// No-op on Windows: there is no `sudo` in PowerShell/cmd, and the askpass
/// helper relies on a POSIX shell.
#[cfg(windows)]
fn wrap_command_for_sudo(
    command: &str,
    sudo: Option<&SudoAskpassEnv>,
) -> Result<(String, Option<()>)> {
    let _ = sudo;
    Ok((command.to_string(), None))
}

#[cfg(test)]
mod sudo_tests {
    use super::*;

    #[test]
    fn detects_sudo_commands() {
        assert!(command_likely_needs_sudo("sudo pacman -S foo"));
        assert!(command_likely_needs_sudo("sudo -n true"));
        assert!(!command_likely_needs_sudo("echo sudo is cool"));
        assert!(!command_likely_needs_sudo("ls /tmp"));
    }

    #[test]
    fn askpass_script_reads_password_once() {
        let env = prepare_sudo_askpass("secret-pass").unwrap();
        if cfg!(windows) {
            // askpass.sh is a POSIX shell script; on Windows we only verify the
            // file layout (script + password file) since there is no `sudo`.
            assert!(env.script_path.exists());
            assert!(env.pass_path.exists());
            let script = std::fs::read_to_string(&env.script_path).unwrap();
            assert!(script.contains("cat"));
            return;
        }
        let out = std::process::Command::new(&env.script_path)
            .output()
            .expect("run askpass");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "secret-pass");
        // Second run should yield empty (file removed).
        let out2 = std::process::Command::new(&env.script_path)
            .output()
            .expect("run askpass again");
        assert!(String::from_utf8_lossy(&out2.stdout).trim().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_commands_are_not_intercepted() {
        assert!(native_tool_suggestion("git diff -- Cargo.toml").is_none());
        assert!(native_tool_suggestion("git status").is_none());
        assert!(native_tool_suggestion("git log --oneline").is_none());
    }

    #[test]
    fn suggests_grep_for_rg() {
        let suggestion =
            native_tool_suggestion("rg \"fn main\" crates/navi-core/src").expect("suggestion");

        assert_eq!(suggestion["native_tool"], "search");
        assert_eq!(suggestion["native_input"]["action"], "grep");
        assert_eq!(suggestion["native_input"]["pattern"], "fn main");
        assert_eq!(suggestion["native_input"]["path"], "crates/navi-core/src");
    }

    #[test]
    fn suggests_search_for_ls() {
        let suggestion = native_tool_suggestion("ls -la crates").expect("suggestion");

        assert_eq!(suggestion["native_tool"], "search");
        assert_eq!(
            suggestion["native_input"],
            json!({ "action": "list", "path": "crates" })
        );
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

#[cfg(test)]
mod shell_select_tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn unix_prefers_bash() {
        // Find bash on PATH.
        let has_bash = std::process::Command::new("bash")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        assert!(has_bash, "bash should be available on Unix test hosts");
        let argv = shell_argv("echo hi");
        assert_eq!(argv[0], "-lc", "unix shell must use -lc");
    }

    #[cfg(windows)]
    #[test]
    fn windows_produces_known_shell_shape() {
        // The argv shape must be one of the supported Windows shells. We
        // cannot know which shell is present, but each has a recognizable
        // flag signature.
        let argv = shell_argv("echo hi");
        let joined = argv.join(" ");
        let ok = joined.starts_with("-lc ")
            || joined.starts_with("-NoProfile -Command ")
            || joined.starts_with("/C ");
        assert!(
            ok,
            "argv must match bash/powershell/cmd signatures: {joined:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_defaults_to_powershell_without_override() {
        // When NAVI_BASH_SHELL is not set, the default Windows shell must be
        // PowerShell (pwsh or powershell), never WSL/bash. This guards against
        // the regression where `where bash.exe` found WSL's launcher and every
        // command failed with exit 1. Uses the pure helper so no env mutation
        // is needed (safe under parallel test execution).
        let program = shell_program_from_override(None);
        let name = program
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        assert!(
            name == "pwsh.exe"
                || name == "pwsh"
                || name == "powershell.exe"
                || name == "powershell",
            "default Windows shell must be PowerShell, got: {program:?}"
        );
        assert!(
            !use_bash_from_override(None),
            "no override must not select bash"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_bash_override_uses_bash() {
        // Setting NAVI_BASH_SHELL=bash opts into bash execution (Git Bash /
        // MSYS2) and must select a bash program + `bash -lc` argv shape.
        assert!(
            use_bash_from_override(Some("bash")),
            "NAVI_BASH_SHELL=bash must opt into bash"
        );
        let program = shell_program_from_override(Some("bash"));
        // Either a resolved path ending in bash.exe, or the bare "bash" name.
        let name = program
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        assert!(
            name == "bash" || name == "bash.exe",
            "bash override must resolve to bash, got: {program:?}"
        );
        // An explicit path override is used verbatim.
        let explicit = shell_program_from_override(Some(r"C:\tools\sh.exe"));
        assert_eq!(explicit, PathBuf::from(r"C:\tools\sh.exe"));
        // Whitespace-only override falls back to the PowerShell default.
        assert!(!use_bash_from_override(Some("   ")));
        let pwsh = shell_program_from_override(Some("   "));
        let pname = pwsh
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        assert!(
            pname == "pwsh" || pname == "powershell",
            "whitespace override must fall back to PowerShell, got: {pwsh:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_detects_git_bash_when_installed() {
        // This machine has Git for Windows (used by CI/test environments).
        // When found, the path must point at a real bash.exe.
        if let Some(found) = find_windows_bash() {
            let exists = Path::new(&found).is_file() || found == PathBuf::from("bash");
            assert!(
                exists,
                "detected bash must exist or be a PATH name: {found:?}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_scoop_git_bash_is_detected() {
        // The CI/test machine installs Git via scoop; the well-known layout
        // is apps/git/current/bin/bash.exe under either $SCOOP or ~/scoop.
        let mut possible_homes = Vec::new();
        if let Ok(s) = std::env::var("SCOOP") {
            possible_homes.push(PathBuf::from(s));
        }
        if let Some(home) = std::env::var_os("USERPROFILE") {
            possible_homes.push(PathBuf::from(home).join("scoop"));
        }
        let installed_here = possible_homes
            .iter()
            .any(|root| root.join("apps/git/current/bin/bash.exe").is_file());
        if installed_here {
            assert!(
                find_windows_bash().is_some(),
                "scoop git bash is installed but not detected"
            );
        }
    }
}

#[cfg(test)]
mod native_redirect_tests {
    use super::*;

    #[test]
    fn sed_range_dump_redirects_to_read_file() {
        let out = native_tool_suggestion(
            "sed -n '380,560p' crates/navi-core/src/tool/builtin/search_tool.rs",
        )
        .expect("should redirect");
        assert_eq!(out["error"], "native_tool_available");
        assert_eq!(out["native_tool"], "read_file");
        assert_eq!(
            out["native_input"]["path"],
            "crates/navi-core/src/tool/builtin/search_tool.rs"
        );
        assert_eq!(out["native_input"]["start_line"], 380);
        assert_eq!(out["native_input"]["end_line"], 560);
    }

    #[test]
    fn cat_file_redirects_to_read_file() {
        let out = native_tool_suggestion("cat src/main.rs").expect("redirect");
        assert_eq!(out["native_tool"], "read_file");
        assert_eq!(out["native_input"]["path"], "src/main.rs");
    }

    #[test]
    fn head_n_redirects_to_read_file_range() {
        let out =
            native_tool_suggestion("head -n 40 crates/navi-core/src/lib.rs").expect("redirect");
        assert_eq!(out["native_tool"], "read_file");
        assert_eq!(out["native_input"]["start_line"], 1);
        assert_eq!(out["native_input"]["end_line"], 40);
    }

    #[test]
    fn chained_sed_still_redirects() {
        let out = native_tool_suggestion(
            "sed -n '1,120p' crates/navi-core/src/tool/builtin/search_tool.rs; echo '---'; sed -n '380,560p' crates/navi-core/src/tool/builtin/search_tool.rs",
        )
        .expect("redirect");
        assert_eq!(out["native_tool"], "read_file");
    }

    #[test]
    fn rg_redirects_to_search() {
        let out = native_tool_suggestion("rg -n foo src").expect("redirect");
        assert_eq!(out["native_tool"], "search");
        assert_eq!(out["native_input"]["action"], "grep");
        assert_eq!(out["native_input"]["pattern"], "foo");
    }

    #[test]
    fn cargo_test_is_not_redirected() {
        assert!(native_tool_suggestion("cargo test -p navi-core").is_none());
    }

    #[test]
    fn sed_inplace_edit_is_not_redirected_as_read() {
        // In-place edits should not be mapped to read_file.
        assert!(native_tool_suggestion("sed -i 's/old/new/' src/lib.rs").is_none());
    }
}
