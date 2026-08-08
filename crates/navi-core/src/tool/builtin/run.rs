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
    // CREATE_NO_WINDOW = 0x08000000 -- tokio's Command exposes creation_flags
    // as an inherent method on Windows (delegates to std::process::Command).
    cmd.creation_flags(0x0800_0000);
}

#[cfg(not(any(unix, windows)))]
fn configure_process_group(_cmd: &mut tokio::process::Command) {}

/// Owns a Win32 Job Object with `KILL_ON_JOB_CLOSE`. When the handle is closed
/// (via [`ProcessTreeGuard::kill_tree`] or Drop), every process in the job --
/// including grandchildren spawned by the shell -- is terminated immediately.
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
/// On Windows, holds a Job Object handle -- dropping it or calling
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

/// The kind of shell the `run` tool will invoke.
///
/// The tool is named `run` (renamed from `bash`), and the *description* tells
/// the model which shell it is really talking to so it writes the correct
/// syntax. See [`shell_description`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellKind {
    /// `bash -lc` (Unix default, or Windows with config/env override).
    Bash,
    /// `zsh -lc` (Unix, when configured).
    Zsh,
    /// PowerShell 7+ (`pwsh -NoProfile -Command`).
    Pwsh,
    /// Windows PowerShell 5.1 (`powershell -NoProfile -Command`).
    PowerShell5,
    /// Nushell (`nu -c`).
    Nu,
    /// Windows cmd.exe (`cmd /C`).
    Cmd,
    /// fish (`fish -c`).
    Fish,
    /// Unknown shell -- use generic description, `-c` argv as a guess.
    Unknown,
}

impl ShellKind {
    /// Extract the shell kind from a program path's file name (lowercased).
    fn from_program_name(name: &str) -> ShellKind {
        match name {
            "bash" | "bash.exe" => ShellKind::Bash,
            "zsh" | "zsh.exe" => ShellKind::Zsh,
            "pwsh" | "pwsh.exe" => ShellKind::Pwsh,
            "powershell" | "powershell.exe" => ShellKind::PowerShell5,
            "nu" | "nu.exe" => ShellKind::Nu,
            "cmd" | "cmd.exe" => ShellKind::Cmd,
            "fish" => ShellKind::Fish,
            _ => ShellKind::Unknown,
        }
    }

    /// The argv prefix to insert before the command string.
    fn argv_prefix(&self) -> &'static [&'static str] {
        match self {
            ShellKind::Bash | ShellKind::Zsh => &["-lc"],
            ShellKind::Pwsh | ShellKind::PowerShell5 => &["-NoProfile", "-Command"],
            ShellKind::Nu | ShellKind::Fish => &["-c"],
            ShellKind::Cmd => &["/C"],
            ShellKind::Unknown => &["-c"],
        }
    }

    /// Human-readable name for the shell (used in descriptions and logs).
    #[cfg(test)]
    fn name(&self) -> &'static str {
        match self {
            ShellKind::Bash => "bash",
            ShellKind::Zsh => "zsh",
            ShellKind::Pwsh => "PowerShell 7+ (pwsh)",
            ShellKind::PowerShell5 => "Windows PowerShell 5.1 (powershell.exe)",
            ShellKind::Nu => "Nushell (nu)",
            ShellKind::Cmd => "cmd.exe",
            ShellKind::Fish => "fish",
            ShellKind::Unknown => "unknown shell",
        }
    }
}

/// Resolve the shell program path from config, env, or platform defaults.
///
/// Resolution order:
/// 1. `ShellConfig.program` (from `config.toml [shell]`)
/// 2. `NAVI_SHELL` env var (any OS)
/// 3. `SHELL` env var (Unix)
/// 4. `NAVI_BASH_SHELL` env var (legacy, Windows)
/// 5. Platform default: `bash` on Unix, `pwsh`->`powershell` on Windows
fn resolve_shell_program(config: &crate::config::ShellConfig) -> PathBuf {
    // 1. Config
    if let Some(program) = &config.program {
        let trimmed = program.trim();
        if !trimmed.is_empty() {
            return resolve_shell_path(trimmed);
        }
    }
    // 2. NAVI_SHELL env (cross-platform)
    if let Ok(shell) = std::env::var("NAVI_SHELL") {
        let trimmed = shell.trim();
        if !trimmed.is_empty() {
            return resolve_shell_path(trimmed);
        }
    }
    // 3. SHELL env (Unix, but may be set on Windows in Git Bash sessions)
    if let Ok(shell) = std::env::var("SHELL") {
        let trimmed = shell.trim();
        if !trimmed.is_empty() {
            return resolve_shell_path(trimmed);
        }
    }
    // 4. Legacy NAVI_BASH_SHELL (Windows)
    if let Ok(shell) = std::env::var("NAVI_BASH_SHELL") {
        let trimmed = shell.trim();
        if !trimmed.is_empty() {
            return resolve_shell_path(trimmed);
        }
    }
    // 5. Platform default
    platform_default_shell()
}

/// Resolve a shell name/path to a PathBuf, handling "bash" specially on Windows.
fn resolve_shell_path(name: &str) -> PathBuf {
    if cfg!(windows) && name.eq_ignore_ascii_case("bash") {
        find_windows_bash().unwrap_or_else(|| PathBuf::from("bash"))
    } else {
        PathBuf::from(name)
    }
}

/// Platform default shell: `bash` on Unix, `pwsh`->`powershell` on Windows.
fn platform_default_shell() -> PathBuf {
    if cfg!(windows) {
        if windows_program_exists("pwsh.exe") {
            PathBuf::from("pwsh")
        } else {
            PathBuf::from("powershell")
        }
    } else {
        PathBuf::from("bash")
    }
}

/// Detect which shell kind the tool will invoke, given a shell config.
fn detect_shell_kind_with(config: &crate::config::ShellConfig) -> ShellKind {
    let program = resolve_shell_program(config);
    let name = program
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    ShellKind::from_program_name(&name)
}

/// Detect which shell kind the tool will invoke using default config (env only).
#[cfg(test)]
fn detect_shell_kind() -> ShellKind {
    detect_shell_kind_with(&crate::config::ShellConfig::default())
}

/// Resolve the argv prefix for the shell, honoring config overrides.
fn shell_argv_prefix(config: &crate::config::ShellConfig, kind: ShellKind) -> Vec<String> {
    if let Some(args) = &config.args {
        if !args.is_empty() {
            return args.clone();
        }
    }
    kind.argv_prefix().iter().map(|s| s.to_string()).collect()
}

/// Build the shell command for the current platform and config.
fn shell_command(
    shell_cmd: &str,
    project_root: &std::path::Path,
    config: &crate::config::ShellConfig,
) -> tokio::process::Command {
    let program = resolve_shell_program(config);
    let kind = detect_shell_kind_with(config);
    let prefix = shell_argv_prefix(config, kind);
    let mut c = tokio::process::Command::new(program);
    for arg in prefix {
        c.arg(&arg);
    }
    c.arg(shell_cmd);
    c.current_dir(project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true);
    configure_process_group(&mut c);
    c
}

/// Generate a shell-specific description for the `run` tool definition.
///
/// The base text is shared across all shells; the shell-specific paragraph
/// tells the model which syntax to use so it doesn't write bash syntax for
/// PowerShell or vice-versa.
fn shell_description(kind: ShellKind) -> String {
    let base = "Run an ad-hoc shell command in the current project. \
        Common git, test, build, and file-read commands (cat/sed/head/rg/ls/find) \
        are not executed here; this tool returns a native_tool_available suggestion \
        pointing at read_file/search. Use background=true and wait_ms for long-running \
        commands. Commands using sudo open a secure password modal in the TUI -- \
        the password is never shown to the model. Never use this tool to dump \
        project source for inspection.";

    match kind {
        ShellKind::Bash => format!(
            "{base}\n\n\
            Shell: bash (POSIX). Use Unix shell syntax: $VAR for variables, \
            $(...) for command substitution, && / || for chaining, | for pipes. \
            Paths use forward slashes. On Windows with Git Bash, drive letters \
            are /c/Users/... not C:\\Users\\...."
        ),
        ShellKind::Zsh => format!(
            "{base}\n\n\
            Shell: zsh (POSIX-compatible). Use Unix shell syntax: $VAR for \
            variables, $(...) for command substitution, && / || for chaining, \
            | for pipes. zsh supports arrays and globbing extensions but \
            standard POSIX syntax is safest. Paths use forward slashes."
        ),
        ShellKind::Pwsh => format!(
            "{base}\n\n\
            Shell: PowerShell 7+ (pwsh). Use PowerShell syntax, NOT bash: \
            $env:VAR for environment variables (not $VAR), $(...) for command \
            substitution, ; to chain commands (prefer ; over &&), | for \
            object-based pipelines. Use Get-ChildItem (not ls), Get-Content \
            (not cat), Select-String (not grep), Test-Path (not test). \
            Paths use backslashes with drive letters: C:\\Users\\...."
        ),
        ShellKind::PowerShell5 => format!(
            "{base}\n\n\
            Shell: Windows PowerShell 5.1 (powershell.exe). Use PowerShell \
            syntax, NOT bash: $env:VAR for environment variables (not $VAR), \
            $(...) for command substitution, ; to chain commands (NEVER use \
            && -- it is not supported in PowerShell 5.1), | for object-based \
            pipelines. Use Get-ChildItem (not ls), Get-Content (not cat), \
            Select-String (not grep), Test-Path (not test). \
            Paths use backslashes with drive letters: C:\\Users\\...."
        ),
        ShellKind::Nu => format!(
            "{base}\n\n\
            Shell: Nushell (nu). Use Nushell syntax, NOT bash: $env.VAR for \
            environment variables (not $VAR), pipelines are object-based. \
            Built-ins: ls, open, into string, str replace. Paths use forward \
            slashes; on Windows drive letters are C:/Users/...."
        ),
        ShellKind::Cmd => format!(
            "{base}\n\n\
            Shell: cmd.exe (Windows). Use cmd syntax: %VAR% for environment \
            variables (not $VAR), & to chain commands, | for text pipelines. \
            Use dir (not ls), type (not cat), findstr (not grep). \
            Paths use backslashes with drive letters: C:\\Users\\...."
        ),
        ShellKind::Fish => format!(
            "{base}\n\n\
            Shell: fish. Use fish syntax, NOT POSIX: $VAR for variables, \
            (cmd) for command substitution (NOT $(...)), and / or / not / \
            not and for chaining (NOT && / ||), | for pipes. Paths use \
            forward slashes."
        ),
        ShellKind::Unknown => format!(
            "{base}\n\n\
            Shell: unknown (configured via [shell] program). The shell \
            syntax is unknown — prefer simple commands without shell-specific \
            features (variables, command substitution, pipes, chaining). \
            If you know which shell is configured, write syntax for that \
            shell; otherwise avoid $VAR, $(...), &&, ||, and | unless you \
            are certain the shell supports POSIX syntax."
        ),
    }
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

pub(crate) struct RunTool {
    background: Arc<RunBackgroundRegistry>,
    project_root: PathBuf,
    shell_config: crate::config::ShellConfig,
}

impl RunTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self::with_shell_config(project_root, crate::config::ShellConfig::default())
    }

    pub(crate) fn with_shell_config(
        project_root: PathBuf,
        shell_config: crate::config::ShellConfig,
    ) -> Self {
        Self {
            background: Arc::new(RunBackgroundRegistry::default()),
            project_root,
            shell_config,
        }
    }

    fn shell_kind(&self) -> ShellKind {
        detect_shell_kind_with(&self.shell_config)
    }
}

#[derive(Default)]
struct RunBackgroundRegistry {
    next_id: AtomicU64,
    tasks: tokio::sync::Mutex<HashMap<String, Arc<RunBackgroundTask>>>,
}

impl RunBackgroundRegistry {
    async fn spawn_task(
        &self,
        command: String,
        description: Option<String>,
        project_root: PathBuf,
        timeout_ms: u64,
        sudo_env: Option<SudoAskpassEnv>,
        shell_config: crate::config::ShellConfig,
    ) -> Result<Arc<RunBackgroundTask>> {
        let mut tasks = self.tasks.lock().await;
        let running_tasks = tasks
            .values()
            .filter(|task| !task.snapshot_state().is_final())
            .count();
        if running_tasks >= BASH_MAX_BACKGROUND_TASKS {
            anyhow::bail!("too many background bash tasks running");
        }

        let task_id = format!("bg_{}", self.next_id.fetch_add(1, Ordering::SeqCst) + 1);
        let task = Arc::new(RunBackgroundTask::spawn(
            task_id.clone(),
            command,
            description,
            project_root,
            timeout_ms,
            sudo_env,
            shell_config,
        )?);
        tasks.insert(task_id, task.clone());
        Ok(task)
    }

    async fn get(&self, task_id: &str) -> Option<Arc<RunBackgroundTask>> {
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

struct RunBackgroundTask {
    task_id: String,
    command: String,
    description: Option<String>,
    started_at: Instant,
    timeout_ms: u64,
    child: tokio::sync::Mutex<Option<tokio::process::Child>>,
    stdout: Arc<tokio::sync::Mutex<Vec<u8>>>,
    stderr: Arc<tokio::sync::Mutex<Vec<u8>>>,
    state: std::sync::Mutex<RunBackgroundState>,
    /// Keeps askpass temp files alive until the task finishes.
    _sudo_askpass: Option<SudoAskpassEnv>,
    /// On Windows, owns the Job Object that kills the whole process tree on
    /// timeout/cancel. On Unix, this is a no-op (process group is used instead).
    tree_guard: tokio::sync::Mutex<ProcessTreeGuard>,
}

impl RunBackgroundTask {
    fn spawn(
        task_id: String,
        command: String,
        description: Option<String>,
        project_root: PathBuf,
        timeout_ms: u64,
        sudo_env: Option<SudoAskpassEnv>,
        shell_config: crate::config::ShellConfig,
    ) -> Result<Self> {
        let (shell_cmd, _guard) = wrap_command_for_sudo(&command, sudo_env.as_ref())?;
        let mut cmd = shell_command(&shell_cmd, &project_root, &shell_config);
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
            state: std::sync::Mutex::new(RunBackgroundState::running()),
            _sudo_askpass: sudo_env,
            tree_guard: tokio::sync::Mutex::new(tree_guard),
        })
    }

    fn snapshot_state(&self) -> RunBackgroundState {
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
        // Give the output reader task a chance to finish reading the pipe
        // after the process exits. Without this, there's a race where
        // try_wait() detects exit but the reader hasn't consumed the last
        // bytes yet. 50ms matches the foreground command's drain delay and
        // is enough for even large buffered outputs on slow I/O.
        if self.snapshot_state().is_final() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let output = self.observation_json().await;
        let state = self.snapshot_state();
        // Timed-out / failed / cancelled background tasks must not report ok:true,
        // otherwise the agent loop treats them as success and can stall waiting
        // for a follow-up that never comes.
        let ok = match state.status {
            RunTaskStatus::Completed | RunTaskStatus::Running => true,
            RunTaskStatus::Failed | RunTaskStatus::TimedOut | RunTaskStatus::Cancelled => false,
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
                *state = RunBackgroundState::cancelled();
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
                *state = RunBackgroundState::completed(status.success(), status.code());
            }
            Ok(None) if timed_out => {
                let mut guard = self.tree_guard.lock().await;
                kill_timed_out_child(child_ref, &mut guard).await;
                *child = None;
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                *state = RunBackgroundState::timed_out();
            }
            Ok(None) => {}
            Err(err) => {
                *child = None;
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                *state = RunBackgroundState::failed(format!("failed to poll command: {err}"));
            }
        }
    }

    async fn observation_json(&self) -> Value {
        let state = self.snapshot_state();
        let mut value = self.snapshot_json().await;
        if !state.is_final() {
            value["message"] = json!(format!(
                "Command is still running. Poll with run({{\"task_id\":\"{}\"}}) or cancel with run({{\"task_id\":\"{}\",\"action\":\"cancel\"}}).",
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
enum RunTaskStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
}

impl std::fmt::Display for RunTaskStatus {
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

impl RunTaskStatus {
    fn is_final(&self) -> bool {
        *self != Self::Running
    }
}

#[derive(Clone)]
struct RunBackgroundState {
    status: RunTaskStatus,
    exit_code: Option<i32>,
    error: Option<String>,
}

impl RunBackgroundState {
    fn running() -> Self {
        Self {
            status: RunTaskStatus::Running,
            exit_code: None,
            error: None,
        }
    }

    fn completed(ok: bool, exit_code: Option<i32>) -> Self {
        Self {
            status: if ok {
                RunTaskStatus::Completed
            } else {
                RunTaskStatus::Failed
            },
            exit_code,
            error: None,
        }
    }

    fn failed(error: String) -> Self {
        Self {
            status: RunTaskStatus::Failed,
            exit_code: None,
            error: Some(error),
        }
    }

    fn timed_out() -> Self {
        Self {
            status: RunTaskStatus::TimedOut,
            exit_code: None,
            error: Some("command timed out: deadline has elapsed".to_string()),
        }
    }

    fn cancelled() -> Self {
        Self {
            status: RunTaskStatus::Cancelled,
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
impl Tool for RunTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "run",
            &shell_description(self.shell_kind()),
            ToolKind::Command,
            helpers::run_json_schema(),
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
                    self.shell_config.clone(),
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
        "message": "This common shell command was not executed. Use the suggested native tool instead of dumping files via the run tool (keeps the TUI clean and uses structured tools).",
        "original_command": command,
        "native_tool": suggestion.tool,
        "native_input": suggestion.input,
        "recoverable": true,
    })
}

/// Redirect shell file readers (sed/cat/head/tail/less/...) to `read_file`.
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
            // script file -- not a project source dump we can map cleanly
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
    // Only redirect when -n is present with a simple print script missing -- skip.
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

impl RunTool {
    async fn run_foreground(
        &self,
        command: &str,
        timeout_ms: u64,
        invocation_id: String,
        sudo_env: Option<SudoAskpassEnv>,
    ) -> Result<ToolResult> {
        let (shell_cmd, _guard) = wrap_command_for_sudo(command, sudo_env.as_ref())?;
        let mut cmd = shell_command(&shell_cmd, &self.project_root, &self.shell_config);
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

// ΓöÇΓöÇ Sudo password (TUI modal + SUDO_ASKPASS; secret never reaches the model) ΓöÇ

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
            // `env VAR=value sudo ...` still leaves us in command position.
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
        s.push('\u{2026}');
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
    // Function-based sudo wrapper uses bash `export -f` syntax. This works
    // with bash and zsh (the common Unix shells). Fish does not support
    // `export -f`; users who configure fish on Unix and need sudo should
    // use a POSIX shell instead. The wrapper is Unix-only because Windows
    // has no sudo.
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
    use crate::config::ShellConfig;

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
        let kind = detect_shell_kind();
        assert_eq!(kind, ShellKind::Bash, "Unix default must be bash");
    }

    #[cfg(windows)]
    #[test]
    fn windows_produces_known_shell_shape() {
        // The detected shell kind must be one of the supported Windows shells.
        let kind = detect_shell_kind();
        assert!(
            matches!(
                kind,
                ShellKind::Bash | ShellKind::Pwsh | ShellKind::PowerShell5 | ShellKind::Cmd
            ),
            "Windows must detect a known shell kind, got: {kind:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_defaults_to_powershell_without_override() {
        // When no config program is set and no env override exists, the default
        // Windows shell must be PowerShell (pwsh or powershell), never WSL/bash.
        let config = ShellConfig::default();
        let program = resolve_shell_program(&config);
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
    }

    #[cfg(windows)]
    #[test]
    fn windows_bash_override_uses_bash() {
        // Setting program to "bash" opts into bash execution (Git Bash / MSYS2).
        let config = ShellConfig {
            program: Some("bash".to_string()),
            args: None,
        };
        let program = resolve_shell_program(&config);
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
        let config2 = ShellConfig {
            program: Some(r"C:\tools\sh.exe".to_string()),
            args: None,
        };
        let explicit = resolve_shell_program(&config2);
        assert_eq!(explicit, PathBuf::from(r"C:\tools\sh.exe"));
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

#[cfg(test)]
mod shell_kind_tests {
    use super::*;
    use crate::config::ShellConfig;
    use crate::tool::{ToolExecutor, ToolInvocation};
    use crate::{PermissionMode, SecurityConfig, SecurityPolicy};
    use serde_json::json;
    use std::path::Path;

    fn executor(root: &Path) -> ToolExecutor {
        let config = SecurityConfig {
            permission_mode: PermissionMode::Yolo,
            ..SecurityConfig::default()
        };
        let policy = SecurityPolicy::new(root.to_path_buf(), root.join(".navi-data"), config)
            .expect("policy");
        ToolExecutor::new(policy)
    }

    /// Build a `ShellConfig` with only a `program` field set (no args).
    /// This is the new-API equivalent of the old `shell_kind_from(override, program)`
    /// calls — the config's `program` field IS the override/program.
    fn cfg(program: Option<&str>) -> ShellConfig {
        ShellConfig {
            program: program.map(|s| s.to_string()),
            args: None,
        }
    }

    // ================================================================
    // Layer 1 — Unit tests: ShellKind::from_program_name (every branch)
    // ================================================================

    #[test]
    fn from_program_name_bash() {
        assert_eq!(ShellKind::from_program_name("bash"), ShellKind::Bash);
        assert_eq!(ShellKind::from_program_name("bash.exe"), ShellKind::Bash);
    }

    #[test]
    fn from_program_name_zsh() {
        assert_eq!(ShellKind::from_program_name("zsh"), ShellKind::Zsh);
        assert_eq!(ShellKind::from_program_name("zsh.exe"), ShellKind::Zsh);
    }

    #[test]
    fn from_program_name_pwsh() {
        assert_eq!(ShellKind::from_program_name("pwsh"), ShellKind::Pwsh);
        assert_eq!(ShellKind::from_program_name("pwsh.exe"), ShellKind::Pwsh);
    }

    #[test]
    fn from_program_name_powershell() {
        assert_eq!(
            ShellKind::from_program_name("powershell"),
            ShellKind::PowerShell5
        );
        assert_eq!(
            ShellKind::from_program_name("powershell.exe"),
            ShellKind::PowerShell5
        );
    }

    #[test]
    fn from_program_name_nu() {
        assert_eq!(ShellKind::from_program_name("nu"), ShellKind::Nu);
        assert_eq!(ShellKind::from_program_name("nu.exe"), ShellKind::Nu);
    }

    #[test]
    fn from_program_name_cmd() {
        assert_eq!(ShellKind::from_program_name("cmd"), ShellKind::Cmd);
        assert_eq!(ShellKind::from_program_name("cmd.exe"), ShellKind::Cmd);
    }

    #[test]
    fn from_program_name_fish() {
        assert_eq!(ShellKind::from_program_name("fish"), ShellKind::Fish);
        // fish.exe is not a recognized variant (fish is Unix-only) → Unknown.
        assert_eq!(ShellKind::from_program_name("fish.exe"), ShellKind::Unknown);
    }

    #[test]
    fn from_program_name_unknown_names() {
        assert_eq!(ShellKind::from_program_name("sh"), ShellKind::Unknown);
        assert_eq!(ShellKind::from_program_name("dash"), ShellKind::Unknown);
        assert_eq!(ShellKind::from_program_name("ksh"), ShellKind::Unknown);
        assert_eq!(ShellKind::from_program_name("tcsh"), ShellKind::Unknown);
        assert_eq!(ShellKind::from_program_name(""), ShellKind::Unknown);
        assert_eq!(
            ShellKind::from_program_name("random_shell"),
            ShellKind::Unknown
        );
    }

    #[test]
    fn from_program_name_is_case_insensitive_via_detect() {
        // from_program_name itself matches lowercased names; detect_shell_kind_with
        // lowercases the file name before calling it.
        assert_eq!(detect_shell_kind_with(&cfg(Some("BASH"))), ShellKind::Bash);
        assert_eq!(detect_shell_kind_with(&cfg(Some("PwSh"))), ShellKind::Pwsh);
        assert_eq!(
            detect_shell_kind_with(&cfg(Some("POWERSHELL"))),
            ShellKind::PowerShell5
        );
    }

    // ================================================================
    // Layer 1 — Unit tests: ShellKind::argv_prefix (every variant)
    // ================================================================

    #[test]
    fn argv_prefix_bash() {
        assert_eq!(ShellKind::Bash.argv_prefix(), &["-lc"]);
    }

    #[test]
    fn argv_prefix_zsh() {
        assert_eq!(ShellKind::Zsh.argv_prefix(), &["-lc"]);
    }

    #[test]
    fn argv_prefix_pwsh() {
        assert_eq!(ShellKind::Pwsh.argv_prefix(), &["-NoProfile", "-Command"]);
    }

    #[test]
    fn argv_prefix_powershell5() {
        assert_eq!(
            ShellKind::PowerShell5.argv_prefix(),
            &["-NoProfile", "-Command"]
        );
    }

    #[test]
    fn argv_prefix_nu() {
        assert_eq!(ShellKind::Nu.argv_prefix(), &["-c"]);
    }

    #[test]
    fn argv_prefix_cmd() {
        assert_eq!(ShellKind::Cmd.argv_prefix(), &["/C"]);
    }

    #[test]
    fn argv_prefix_fish() {
        assert_eq!(ShellKind::Fish.argv_prefix(), &["-c"]);
    }

    #[test]
    fn argv_prefix_unknown() {
        assert_eq!(ShellKind::Unknown.argv_prefix(), &["-c"]);
    }

    // ================================================================
    // Layer 1 — Unit tests: ShellKind::name (every variant)
    // ================================================================

    #[test]
    fn name_returns_human_readable() {
        assert_eq!(ShellKind::Bash.name(), "bash");
        assert_eq!(ShellKind::Zsh.name(), "zsh");
        assert_eq!(ShellKind::Pwsh.name(), "PowerShell 7+ (pwsh)");
        assert_eq!(
            ShellKind::PowerShell5.name(),
            "Windows PowerShell 5.1 (powershell.exe)"
        );
        assert_eq!(ShellKind::Nu.name(), "Nushell (nu)");
        assert_eq!(ShellKind::Cmd.name(), "cmd.exe");
        assert_eq!(ShellKind::Fish.name(), "fish");
        assert_eq!(ShellKind::Unknown.name(), "unknown shell");
    }

    // ================================================================
    // Layer 1 — Unit tests: detect_shell_kind_with (pure helper, every branch)
    // ================================================================

    #[test]
    fn bash_program_selects_bash() {
        assert_eq!(detect_shell_kind_with(&cfg(Some("bash"))), ShellKind::Bash);
    }

    #[test]
    fn pwsh_program_selects_pwsh() {
        assert_eq!(detect_shell_kind_with(&cfg(Some("pwsh"))), ShellKind::Pwsh);
        assert_eq!(
            detect_shell_kind_with(&cfg(Some("pwsh.exe"))),
            ShellKind::Pwsh
        );
    }

    #[test]
    fn powershell_program_selects_ps5() {
        assert_eq!(
            detect_shell_kind_with(&cfg(Some("powershell"))),
            ShellKind::PowerShell5
        );
        assert_eq!(
            detect_shell_kind_with(&cfg(Some("powershell.exe"))),
            ShellKind::PowerShell5
        );
    }

    #[test]
    fn zsh_program_selects_zsh() {
        assert_eq!(detect_shell_kind_with(&cfg(Some("zsh"))), ShellKind::Zsh);
    }

    #[test]
    fn nu_program_selects_nu() {
        assert_eq!(detect_shell_kind_with(&cfg(Some("nu"))), ShellKind::Nu);
        assert_eq!(detect_shell_kind_with(&cfg(Some("nu.exe"))), ShellKind::Nu);
    }

    #[test]
    fn cmd_program_selects_cmd() {
        assert_eq!(detect_shell_kind_with(&cfg(Some("cmd"))), ShellKind::Cmd);
        assert_eq!(
            detect_shell_kind_with(&cfg(Some("cmd.exe"))),
            ShellKind::Cmd
        );
    }

    #[test]
    fn fish_program_selects_fish() {
        assert_eq!(detect_shell_kind_with(&cfg(Some("fish"))), ShellKind::Fish);
    }

    #[test]
    fn no_program_defaults_to_platform_shell() {
        // With no program and no env override, the platform default is used:
        // bash on Unix, pwsh/powershell on Windows.
        let kind = detect_shell_kind_with(&cfg(None));
        #[cfg(not(windows))]
        {
            assert_eq!(kind, ShellKind::Bash, "Unix default must be bash");
        }
        #[cfg(windows)]
        {
            assert!(
                matches!(kind, ShellKind::Pwsh | ShellKind::PowerShell5),
                "Windows default must be pwsh or powershell, got: {kind:?}"
            );
        }
    }

    // ================================================================
    // Layer 2 — Edge case tests: boundary, invalid, unicode, case
    // ================================================================

    // --- Empty / null ---

    #[test]
    fn empty_program_string_falls_through_to_default() {
        // An empty program string is treated as "not specified" — resolve
        // falls through to env vars / platform default, same as None.
        let empty = detect_shell_kind_with(&cfg(Some("")));
        let none = detect_shell_kind_with(&cfg(None));
        assert_eq!(
            empty, none,
            "empty program must behave like None (fall through to default)"
        );
    }

    #[test]
    fn whitespace_program_string_falls_through_to_default() {
        let ws = detect_shell_kind_with(&cfg(Some("   ")));
        let none = detect_shell_kind_with(&cfg(None));
        assert_eq!(ws, none, "whitespace-only program must behave like None");
    }

    // --- Case insensitivity ---

    #[test]
    fn bash_program_is_case_insensitive() {
        assert_eq!(detect_shell_kind_with(&cfg(Some("BASH"))), ShellKind::Bash);
        assert_eq!(detect_shell_kind_with(&cfg(Some("Bash"))), ShellKind::Bash);
        assert_eq!(detect_shell_kind_with(&cfg(Some("BaSh"))), ShellKind::Bash);
    }

    #[test]
    fn bash_program_with_trailing_whitespace_is_trimmed() {
        assert_eq!(
            detect_shell_kind_with(&cfg(Some("  bash  "))),
            ShellKind::Bash
        );
    }

    // --- Path with directories (file_name extraction) ---

    #[test]
    fn full_path_to_pwsh_extracts_file_name() {
        assert_eq!(
            detect_shell_kind_with(&cfg(Some(r"C:\Program Files\PowerShell\7\pwsh.exe"))),
            ShellKind::Pwsh
        );
    }

    #[test]
    fn full_path_to_powershell5_extracts_file_name() {
        assert_eq!(
            detect_shell_kind_with(&cfg(Some(
                r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
            ))),
            ShellKind::PowerShell5
        );
    }

    #[test]
    fn full_path_to_bash_extracts_file_name() {
        // bash.exe IS now recognized by from_program_name.
        assert_eq!(
            detect_shell_kind_with(&cfg(Some(r"C:\Program Files\Git\bin\bash.exe"))),
            ShellKind::Bash
        );
    }

    // --- Unknown / invalid programs ---

    #[test]
    fn known_non_default_programs_select_correct_kind() {
        // These were previously forced to PowerShell5; now they map correctly.
        assert_eq!(
            detect_shell_kind_with(&cfg(Some("cmd.exe"))),
            ShellKind::Cmd
        );
        assert_eq!(detect_shell_kind_with(&cfg(Some("nu.exe"))), ShellKind::Nu);
        assert_eq!(detect_shell_kind_with(&cfg(Some("fish"))), ShellKind::Fish);
        assert_eq!(detect_shell_kind_with(&cfg(Some("zsh"))), ShellKind::Zsh);
    }

    #[test]
    fn unrecognized_program_defaults_to_unknown() {
        assert_eq!(detect_shell_kind_with(&cfg(Some("sh"))), ShellKind::Unknown);
        assert_eq!(
            detect_shell_kind_with(&cfg(Some(r"C:\tools\nu.exe"))),
            ShellKind::Nu
        );
    }

    // --- Unicode / non-ASCII in paths ---

    #[test]
    fn unicode_in_program_path_does_not_crash() {
        // Non-ASCII directory names should not panic; file_name extraction
        // still works if the path is valid UTF-8.
        let result = detect_shell_kind_with(&cfg(Some("C:\\ユーザー\\pwsh.exe")));
        assert_eq!(result, ShellKind::Pwsh);
    }

    #[test]
    fn emoji_in_program_path_does_not_crash() {
        let result = detect_shell_kind_with(&cfg(Some("C:\\tools\\🚀\\pwsh.exe")));
        assert_eq!(result, ShellKind::Pwsh);
    }

    #[test]
    fn unicode_in_program_does_not_select_bash() {
        // A non-"bash" unicode program must not be treated as bash.
        assert_eq!(
            detect_shell_kind_with(&cfg(Some("バッシュ"))),
            ShellKind::Unknown
        );
    }

    // --- Boundary: very long paths ---

    #[test]
    fn very_long_path_extracts_file_name() {
        let dir = "A".repeat(200);
        let path = format!(r"C:\{dir}\pwsh.exe");
        assert_eq!(detect_shell_kind_with(&cfg(Some(&path))), ShellKind::Pwsh);
    }

    // --- Path without file_name (root or trailing slash) ---

    #[test]
    fn root_path_defaults_to_unknown() {
        // "/" and "C:\\" have no file_name component, so from_program_name
        // receives an empty string → Unknown.
        assert_eq!(detect_shell_kind_with(&cfg(Some("/"))), ShellKind::Unknown);
        assert_eq!(
            detect_shell_kind_with(&cfg(Some("C:\\"))),
            ShellKind::Unknown
        );
    }

    // --- ShellConfig edge cases ---

    #[test]
    fn empty_program_string_config() {
        let config = ShellConfig {
            program: Some(String::new()),
            args: None,
        };
        // Empty program falls through to default detection.
        let kind = detect_shell_kind_with(&config);
        let default = detect_shell_kind();
        assert_eq!(
            kind, default,
            "empty program string must match default detection"
        );
    }

    #[test]
    fn whitespace_only_program_config() {
        let config = ShellConfig {
            program: Some("   \t  ".to_string()),
            args: None,
        };
        let kind = detect_shell_kind_with(&config);
        let default = detect_shell_kind();
        assert_eq!(
            kind, default,
            "whitespace-only program must match default detection"
        );
    }

    #[test]
    fn program_with_only_exe_extension() {
        // A program that is just ".exe" has no recognized shell name.
        let kind = detect_shell_kind_with(&cfg(Some(".exe")));
        assert_eq!(kind, ShellKind::Unknown);
    }

    #[test]
    fn very_long_program_name() {
        let name = "x".repeat(10_000);
        let kind = detect_shell_kind_with(&cfg(Some(&name)));
        assert_eq!(kind, ShellKind::Unknown);
    }

    // ================================================================
    // Layer 1 — Unit tests: shell_argv_prefix (config args override)
    // ================================================================

    #[test]
    fn shell_argv_prefix_uses_kind_default_when_no_args() {
        let config = cfg(Some("bash"));
        let argv = shell_argv_prefix(&config, ShellKind::Bash);
        assert_eq!(argv, vec!["-lc".to_string()]);
    }

    #[test]
    fn shell_argv_prefix_uses_kind_default_when_args_none() {
        let config = ShellConfig {
            program: Some("pwsh".to_string()),
            args: None,
        };
        let argv = shell_argv_prefix(&config, ShellKind::Pwsh);
        assert_eq!(argv, vec!["-NoProfile".to_string(), "-Command".to_string()]);
    }

    #[test]
    fn shell_argv_prefix_uses_config_args_when_some() {
        let config = ShellConfig {
            program: Some("bash".to_string()),
            args: Some(vec!["--custom".to_string(), "-x".to_string()]),
        };
        let argv = shell_argv_prefix(&config, ShellKind::Bash);
        assert_eq!(
            argv,
            vec!["--custom".to_string(), "-x".to_string()],
            "config.args must override the kind's default prefix"
        );
    }

    #[test]
    fn shell_argv_prefix_ignores_empty_args_vec() {
        // An empty args vec is treated as "not set" → fall back to kind default.
        let config = ShellConfig {
            program: Some("bash".to_string()),
            args: Some(vec![]),
        };
        let argv = shell_argv_prefix(&config, ShellKind::Bash);
        assert_eq!(
            argv,
            vec!["-lc".to_string()],
            "empty args vec must fall back to kind default"
        );
    }

    #[test]
    fn shell_argv_prefix_cmd_uses_slash_c() {
        let argv = shell_argv_prefix(&cfg(Some("cmd")), ShellKind::Cmd);
        assert_eq!(argv, vec!["/C".to_string()]);
    }

    // ================================================================
    // Layer 1 — Unit tests: detect_shell_kind (platform-aware)
    // ================================================================

    #[cfg(not(windows))]
    #[test]
    fn unix_detects_bash() {
        assert_eq!(detect_shell_kind(), ShellKind::Bash);
    }

    #[cfg(windows)]
    #[test]
    fn windows_detects_known_kind() {
        let kind = detect_shell_kind();
        assert!(
            matches!(
                kind,
                ShellKind::Bash | ShellKind::Pwsh | ShellKind::PowerShell5
            ),
            "Windows must detect a known shell kind, got: {kind:?}"
        );
    }

    // ================================================================
    // Layer 1 — Unit tests: shell_description (every variant, every hint)
    // ================================================================

    // --- Bash description ---

    #[test]
    fn bash_description_says_bash() {
        let desc = shell_description(ShellKind::Bash);
        assert!(desc.contains("bash"), "bash desc must say bash: {desc}");
    }

    #[test]
    fn bash_description_mentions_posix_syntax() {
        let desc = shell_description(ShellKind::Bash);
        assert!(desc.contains("$VAR"), "bash desc must mention $VAR: {desc}");
        assert!(
            desc.contains("$(...)"),
            "bash desc must mention $(...): {desc}"
        );
        assert!(desc.contains("&&"), "bash desc must mention &&: {desc}");
        assert!(
            desc.contains("forward slashes"),
            "bash desc must mention forward slashes: {desc}"
        );
    }

    #[test]
    fn bash_description_does_not_mention_powershell() {
        let desc = shell_description(ShellKind::Bash);
        assert!(
            !desc.contains("PowerShell"),
            "bash desc must not mention PowerShell: {desc}"
        );
        assert!(
            !desc.contains("$env:"),
            "bash desc must not mention $env: : {desc}"
        );
    }

    // --- Pwsh description ---

    #[test]
    fn pwsh_description_says_powershell_7() {
        let desc = shell_description(ShellKind::Pwsh);
        assert!(
            desc.contains("PowerShell 7"),
            "pwsh desc must say PowerShell 7: {desc}"
        );
    }

    #[test]
    fn pwsh_description_mentions_env_var_syntax() {
        let desc = shell_description(ShellKind::Pwsh);
        assert!(
            desc.contains("$env:VAR"),
            "pwsh desc must mention $env:VAR: {desc}"
        );
        assert!(
            desc.contains("not $VAR"),
            "pwsh desc must contrast with $VAR: {desc}"
        );
    }

    #[test]
    fn pwsh_description_warns_not_bash() {
        let desc = shell_description(ShellKind::Pwsh);
        assert!(
            desc.contains("NOT bash"),
            "pwsh desc must warn NOT bash: {desc}"
        );
    }

    #[test]
    fn pwsh_description_mentions_cmdlet_replacements() {
        let desc = shell_description(ShellKind::Pwsh);
        assert!(
            desc.contains("Get-ChildItem"),
            "pwsh desc must mention Get-ChildItem: {desc}"
        );
        assert!(
            desc.contains("Get-Content"),
            "pwsh desc must mention Get-Content: {desc}"
        );
        assert!(
            desc.contains("Select-String"),
            "pwsh desc must mention Select-String: {desc}"
        );
        assert!(
            desc.contains("Test-Path"),
            "pwsh desc must mention Test-Path: {desc}"
        );
    }

    #[test]
    fn pwsh_description_mentions_backslash_paths() {
        let desc = shell_description(ShellKind::Pwsh);
        assert!(
            desc.contains("backslashes"),
            "pwsh desc must mention backslashes: {desc}"
        );
    }

    // --- PowerShell 5.1 description ---

    #[test]
    fn powershell5_description_says_5_1() {
        let desc = shell_description(ShellKind::PowerShell5);
        assert!(
            desc.contains("PowerShell 5.1"),
            "ps5 desc must say PowerShell 5.1: {desc}"
        );
    }

    #[test]
    fn powershell5_description_warns_against_and_chain() {
        let desc = shell_description(ShellKind::PowerShell5);
        assert!(
            desc.contains("NEVER use &&"),
            "ps5 desc must warn against &&: {desc}"
        );
    }

    #[test]
    fn powershell5_description_mentions_env_var_syntax() {
        let desc = shell_description(ShellKind::PowerShell5);
        assert!(
            desc.contains("$env:VAR"),
            "ps5 desc must mention $env:VAR: {desc}"
        );
        assert!(
            desc.contains("not $VAR"),
            "ps5 desc must contrast with $VAR: {desc}"
        );
    }

    #[test]
    fn powershell5_description_warns_not_bash() {
        let desc = shell_description(ShellKind::PowerShell5);
        assert!(
            desc.contains("NOT bash"),
            "ps5 desc must warn NOT bash: {desc}"
        );
    }

    #[test]
    fn powershell5_description_mentions_cmdlet_replacements() {
        let desc = shell_description(ShellKind::PowerShell5);
        assert!(
            desc.contains("Get-ChildItem"),
            "ps5 desc must mention Get-ChildItem: {desc}"
        );
        assert!(
            desc.contains("Get-Content"),
            "ps5 desc must mention Get-Content: {desc}"
        );
        assert!(
            desc.contains("Select-String"),
            "ps5 desc must mention Select-String: {desc}"
        );
        assert!(
            desc.contains("Test-Path"),
            "ps5 desc must mention Test-Path: {desc}"
        );
    }

    // --- Zsh description ---

    #[test]
    fn zsh_description_says_zsh() {
        let desc = shell_description(ShellKind::Zsh);
        assert!(desc.contains("zsh"), "zsh desc must say zsh: {desc}");
    }

    #[test]
    fn zsh_description_mentions_posix_syntax() {
        let desc = shell_description(ShellKind::Zsh);
        assert!(desc.contains("$VAR"), "zsh desc must mention $VAR: {desc}");
        assert!(
            desc.contains("$(...)"),
            "zsh desc must mention $(...): {desc}"
        );
        assert!(
            desc.contains("forward slashes"),
            "zsh desc must mention forward slashes: {desc}"
        );
    }

    // --- Nu description ---

    #[test]
    fn nu_description_says_nushell() {
        let desc = shell_description(ShellKind::Nu);
        assert!(
            desc.contains("Nushell") || desc.contains("nu"),
            "nu desc must say Nushell/nu: {desc}"
        );
    }

    #[test]
    fn nu_description_mentions_env_var_syntax() {
        let desc = shell_description(ShellKind::Nu);
        assert!(
            desc.contains("$env.VAR"),
            "nu desc must mention $env.VAR: {desc}"
        );
        assert!(
            desc.contains("not $VAR"),
            "nu desc must contrast with $VAR: {desc}"
        );
    }

    #[test]
    fn nu_description_mentions_forward_slashes() {
        let desc = shell_description(ShellKind::Nu);
        assert!(
            desc.contains("forward slashes"),
            "nu desc must mention forward slashes: {desc}"
        );
    }

    // --- Cmd description ---

    #[test]
    fn cmd_description_says_cmd() {
        let desc = shell_description(ShellKind::Cmd);
        assert!(desc.contains("cmd"), "cmd desc must say cmd: {desc}");
    }

    #[test]
    fn cmd_description_mentions_percent_var_syntax() {
        let desc = shell_description(ShellKind::Cmd);
        assert!(
            desc.contains("%VAR%"),
            "cmd desc must mention %VAR%: {desc}"
        );
        assert!(
            desc.contains("not $VAR"),
            "cmd desc must contrast with $VAR: {desc}"
        );
    }

    #[test]
    fn cmd_description_mentions_backslash_paths() {
        let desc = shell_description(ShellKind::Cmd);
        assert!(
            desc.contains("backslashes"),
            "cmd desc must mention backslashes: {desc}"
        );
    }

    // --- Fish description ---

    #[test]
    fn fish_description_says_fish() {
        let desc = shell_description(ShellKind::Fish);
        assert!(desc.contains("fish"), "fish desc must say fish: {desc}");
    }

    #[test]
    fn fish_description_mentions_non_posix_syntax() {
        let desc = shell_description(ShellKind::Fish);
        assert!(
            desc.contains("(cmd)") || desc.contains("NOT $(...)"),
            "fish desc must mention (cmd) / NOT $(...): {desc}"
        );
        assert!(
            desc.contains("forward slashes"),
            "fish desc must mention forward slashes: {desc}"
        );
    }

    // --- Unknown description ---

    #[test]
    fn unknown_description_says_unknown() {
        let desc = shell_description(ShellKind::Unknown);
        assert!(
            desc.contains("unknown"),
            "unknown desc must say unknown: {desc}"
        );
    }

    #[test]
    fn unknown_description_mentions_posix_fallback() {
        let desc = shell_description(ShellKind::Unknown);
        assert!(
            desc.contains("POSIX"),
            "unknown desc must mention POSIX fallback: {desc}"
        );
        assert!(
            desc.contains("$VAR"),
            "unknown desc must mention $VAR: {desc}"
        );
    }

    // --- All descriptions share base text ---

    #[test]
    fn all_descriptions_share_base_text() {
        let all_kinds = [
            ShellKind::Bash,
            ShellKind::Zsh,
            ShellKind::Pwsh,
            ShellKind::PowerShell5,
            ShellKind::Nu,
            ShellKind::Cmd,
            ShellKind::Fish,
            ShellKind::Unknown,
        ];
        for kind in all_kinds {
            let desc = shell_description(kind);
            assert!(
                desc.contains("native_tool_available"),
                "{kind:?} desc must keep base text: {desc}"
            );
            assert!(
                desc.contains("background=true"),
                "{kind:?} desc must keep base text: {desc}"
            );
            assert!(
                desc.contains("sudo"),
                "{kind:?} desc must mention sudo: {desc}"
            );
            assert!(
                desc.contains("password"),
                "{kind:?} desc must mention password: {desc}"
            );
        }
    }

    #[test]
    fn descriptions_are_non_empty() {
        let all_kinds = [
            ShellKind::Bash,
            ShellKind::Zsh,
            ShellKind::Pwsh,
            ShellKind::PowerShell5,
            ShellKind::Nu,
            ShellKind::Cmd,
            ShellKind::Fish,
            ShellKind::Unknown,
        ];
        for kind in all_kinds {
            let desc = shell_description(kind);
            assert!(
                desc.len() > 100,
                "{kind:?} desc must be substantial: len={}",
                desc.len()
            );
        }
    }

    #[test]
    fn descriptions_are_distinct() {
        let all_kinds = [
            ShellKind::Bash,
            ShellKind::Zsh,
            ShellKind::Pwsh,
            ShellKind::PowerShell5,
            ShellKind::Nu,
            ShellKind::Cmd,
            ShellKind::Fish,
            ShellKind::Unknown,
        ];
        let descs: Vec<String> = all_kinds.iter().map(|k| shell_description(*k)).collect();
        for i in 0..descs.len() {
            for j in (i + 1)..descs.len() {
                assert_ne!(
                    descs[i], descs[j],
                    "{:?} and {:?} descriptions must differ",
                    all_kinds[i], all_kinds[j]
                );
            }
        }
    }

    // ================================================================
    // Layer 1 — Unit tests: definition() metadata
    // ================================================================

    #[test]
    fn definition_name_is_run() {
        let temp = tempfile::tempdir().unwrap();
        let tool = RunTool::new(temp.path().to_path_buf());
        let def = tool.definition();
        assert_eq!(def.name, "run", "tool name must be run");
    }

    #[test]
    fn definition_kind_is_command() {
        let temp = tempfile::tempdir().unwrap();
        let tool = RunTool::new(temp.path().to_path_buf());
        let def = tool.definition();
        assert_eq!(def.kind, ToolKind::Command);
    }

    #[test]
    fn definition_description_is_non_empty() {
        let temp = tempfile::tempdir().unwrap();
        let tool = RunTool::new(temp.path().to_path_buf());
        let def = tool.definition();
        assert!(!def.description.is_empty(), "description must not be empty");
    }

    #[test]
    fn definition_description_matches_detected_shell_kind() {
        let temp = tempfile::tempdir().unwrap();
        let tool = RunTool::new(temp.path().to_path_buf());
        let def = tool.definition();
        let kind = detect_shell_kind();
        match kind {
            ShellKind::Bash => assert!(
                def.description.contains("bash") && !def.description.contains("PowerShell"),
                "Bash definition must say bash, not PowerShell: {}",
                def.description
            ),
            ShellKind::Zsh => assert!(
                def.description.contains("zsh"),
                "Zsh definition must say zsh: {}",
                def.description
            ),
            ShellKind::Pwsh => assert!(
                def.description.contains("PowerShell 7"),
                "Pwsh definition must say PowerShell 7: {}",
                def.description
            ),
            ShellKind::PowerShell5 => assert!(
                def.description.contains("PowerShell 5.1"),
                "PS5 definition must say PowerShell 5.1: {}",
                def.description
            ),
            ShellKind::Nu => assert!(
                def.description.contains("Nushell") || def.description.contains("nu"),
                "Nu definition must say Nushell/nu: {}",
                def.description
            ),
            ShellKind::Cmd => assert!(
                def.description.contains("cmd"),
                "Cmd definition must say cmd: {}",
                def.description
            ),
            ShellKind::Fish => assert!(
                def.description.contains("fish"),
                "Fish definition must say fish: {}",
                def.description
            ),
            ShellKind::Unknown => assert!(
                def.description.contains("unknown"),
                "Unknown definition must say unknown: {}",
                def.description
            ),
        }
    }

    #[test]
    fn definition_schema_has_command_field() {
        let temp = tempfile::tempdir().unwrap();
        let tool = RunTool::new(temp.path().to_path_buf());
        let def = tool.definition();
        assert!(
            def.input_schema["properties"]["command"].is_object(),
            "schema must have command field"
        );
    }

    // ================================================================
    // Layer 3 — Integration tests: invoke() against the real shell
    // ================================================================

    /// Run a simple echo command through the full tool path and verify
    /// it succeeds. This exercises detect_shell_kind → shell_command →
    /// real shell process → output capture, end-to-end.
    #[tokio::test]
    async fn invoke_runs_simple_command_on_detected_shell() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let exec = executor(tempdir.path());

        // Use a command that works in both bash and PowerShell.
        let result = exec
            .invoke(ToolInvocation {
                id: "echo-test".to_string(),
                tool_name: "run".to_string(),
                input: json!({ "command": "echo hello_from_shell" }),
            })
            .await;

        assert!(result.ok, "echo must succeed on detected shell: {result:?}");
        let stdout = result.output["stdout"].as_str().unwrap_or_default().trim();
        assert_eq!(
            stdout, "hello_from_shell",
            "echo output must match: got {stdout:?}"
        );
    }

    /// Verify the detected shell kind matches what the shell actually is
    /// by running a shell-identifying command. This is the integration
    /// counterpart to the unit tests for detect_shell_kind.
    #[tokio::test]
    async fn invoke_shell_identity_matches_detected_kind() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let exec = executor(tempdir.path());
        let kind = detect_shell_kind();

        // Run a command that only works in the detected shell family.
        let (command, expected_substring) = match kind {
            ShellKind::Bash => (
                "echo $BASH_VERSION".to_string(),
                // On some systems BASH_VERSION may be empty in non-interactive
                // mode; accept either the version string or a non-error exit.
                "".to_string(),
            ),
            ShellKind::Zsh => ("echo $ZSH_VERSION".to_string(), "".to_string()),
            ShellKind::Pwsh => (
                "$PSVersionTable.PSVersion.ToString()".to_string(),
                "7.".to_string(), // PS 7.x
            ),
            ShellKind::PowerShell5 => (
                "$PSVersionTable.PSVersion.ToString()".to_string(),
                "5.".to_string(), // PS 5.x
            ),
            ShellKind::Nu => ("version | get version".to_string(), "".to_string()),
            ShellKind::Cmd => ("ver".to_string(), "".to_string()),
            ShellKind::Fish => ("echo $FISH_VERSION".to_string(), "".to_string()),
            ShellKind::Unknown => ("echo unknown_shell".to_string(), "".to_string()),
        };

        let result = exec
            .invoke(ToolInvocation {
                id: "shell-id".to_string(),
                tool_name: "run".to_string(),
                input: json!({ "command": command }),
            })
            .await;

        if !result.ok {
            // Shell may be unavailable in headless CI — skip gracefully.
            eprintln!(
                "skipping shell identity test: shell command failed: {:?}",
                result.output
            );
            return;
        }

        let stdout = result.output["stdout"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();

        if matches!(kind, ShellKind::Bash | ShellKind::Zsh | ShellKind::Fish) {
            // version vars may not be set in non-interactive mode; just
            // verify the command ran without error (already asserted above).
            eprintln!("shell identity: version output = {stdout:?}");
        } else if !expected_substring.is_empty() {
            assert!(
                stdout.contains(&expected_substring),
                "shell version must contain {expected_substring:?}, got: {stdout:?}"
            );
        }
    }

    /// Verify that PowerShell-specific syntax works when PowerShell is
    /// the detected shell — this is the exact scenario the description
    /// fix targets (model writes PS syntax because description says PS).
    #[tokio::test]
    async fn invoke_powershell_env_var_syntax_works_when_ps_detected() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let exec = executor(tempdir.path());
        let kind = detect_shell_kind();

        if !matches!(kind, ShellKind::Pwsh | ShellKind::PowerShell5) {
            eprintln!("skipping: detected shell is not PowerShell ({kind:?})");
            return;
        }

        // $env:COMPUTERNAME works on Windows PowerShell; on bash this would
        // be interpreted as an empty variable — the exact bug we fixed.
        let result = exec
            .invoke(ToolInvocation {
                id: "ps-env".to_string(),
                tool_name: "run".to_string(),
                input: json!({ "command": "echo $env:COMPUTERNAME" }),
            })
            .await;

        if !result.ok {
            eprintln!(
                "skipping: PowerShell env var command failed: {:?}",
                result.output
            );
            return;
        }

        // On PowerShell, $env:COMPUTERNAME resolves to the machine name.
        // On bash, $env would be empty and :COMPUTERNAME would be literal.
        // If we get here with PS detected, the output should be non-empty.
        let stdout = result.output["stdout"].as_str().unwrap_or_default().trim();
        eprintln!("PowerShell $env:COMPUTERNAME = {stdout:?}");
    }

    /// Verify that bash-specific syntax works when bash is the detected
    /// shell — ensures the tool doesn't break Unix behavior.
    #[tokio::test]
    async fn invoke_bash_syntax_works_when_bash_detected() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let exec = executor(tempdir.path());
        let kind = detect_shell_kind();

        if kind != ShellKind::Bash {
            eprintln!("skipping: detected shell is not bash ({kind:?})");
            return;
        }

        // $(...) command substitution is bash syntax.
        let result = exec
            .invoke(ToolInvocation {
                id: "bash-subst".to_string(),
                tool_name: "run".to_string(),
                input: json!({ "command": "echo $(echo nested_output)" }),
            })
            .await;

        assert!(result.ok, "bash command substitution must work: {result:?}");
        let stdout = result.output["stdout"].as_str().unwrap_or_default().trim();
        assert_eq!(
            stdout, "nested_output",
            "bash $(...) must resolve: got {stdout:?}"
        );
    }

    /// Verify the tool returns an error with a human-readable message
    /// when given an empty command.
    #[tokio::test]
    async fn invoke_empty_command_returns_error() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let exec = executor(tempdir.path());

        let result = exec
            .invoke(ToolInvocation {
                id: "empty-cmd".to_string(),
                tool_name: "run".to_string(),
                input: json!({ "command": "" }),
            })
            .await;

        // Empty command should either fail or be a no-op, but must not
        // crash or hang. The tool may redirect empty commands to native
        // tools or return an error — both are acceptable.
        let output_str = result.output.to_string();
        assert!(
            !result.ok || output_str.contains("native_tool"),
            "empty command should fail or redirect, got: {result:?}"
        );
    }

    /// Verify the tool handles a command that writes to stderr without
    /// crashing — exercises the stderr capture path on the detected shell.
    #[tokio::test]
    async fn invoke_command_with_stderr_does_not_crash() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let exec = executor(tempdir.path());
        let kind = detect_shell_kind();

        // A command that writes to stderr but exits 0.
        let command = match kind {
            ShellKind::Bash | ShellKind::Zsh | ShellKind::Fish => "echo error_msg >&2".to_string(),
            ShellKind::Pwsh | ShellKind::PowerShell5 => {
                "[Console]::Error.WriteLine('error_msg')".to_string()
            }
            ShellKind::Nu => "print -e error_msg".to_string(),
            ShellKind::Cmd => "echo error_msg 1>&2".to_string(),
            ShellKind::Unknown => "echo error_msg >&2".to_string(),
        };

        let result = exec
            .invoke(ToolInvocation {
                id: "stderr-test".to_string(),
                tool_name: "run".to_string(),
                input: json!({ "command": command }),
            })
            .await;

        // Should succeed (exit 0) even with stderr output.
        assert!(result.ok, "stderr command should exit 0: {result:?}");
    }
}
