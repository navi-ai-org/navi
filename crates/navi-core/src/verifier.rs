//! Core verifier API for NAVI.
//!
//! Provides the types and runner for executing verification commands
//! (build, test, typecheck, lint, or arbitrary commands) and a store
//! that keeps results keyed by feature ID or tool call ID.
//!
//! The `VerifierRunner` uses `tokio::process::Command` so it integrates
//! seamlessly with the async tool system. It captures stdout, stderr,
//! exit code, and timing, and classifies failures into machine-readable
//! error classes with suggested next actions.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

// ═════════════════════════════════════════════════════════════════════════════
// VerifierSpec
// ═════════════════════════════════════════════════════════════════════════════

/// Describes what should be verified and how.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerifierSpec {
    /// The category of verification: "build", "test", "typecheck", "lint", or "command".
    #[serde(rename = "verifier_type")]
    pub verifier_type: String,

    /// The shell command to run (e.g. "cargo build", "cargo test", "cargo check").
    pub command: String,

    /// Optional working directory override. Defaults to the project root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,

    /// Timeout in milliseconds. Defaults to 120_000 (2 minutes) if not set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,

    /// Whether this verification is required for the step to be considered passing.
    #[serde(default = "default_required")]
    pub required: bool,
}

fn default_required() -> bool {
    true
}

impl Default for VerifierSpec {
    fn default() -> Self {
        Self {
            verifier_type: "command".to_string(),
            command: String::new(),
            cwd: None,
            timeout_ms: None,
            required: true,
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// VerifierResult
// ═════════════════════════════════════════════════════════════════════════════

/// The outcome of running a single verification command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerifierResult {
    /// Overall status: "pass", "fail", "error", or "skipped".
    pub status: String,

    /// The command that was executed.
    pub command: String,

    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,

    /// Captured stdout (lossy UTF-8, truncated to 64 KiB).
    pub stdout: String,

    /// Captured stderr (lossy UTF-8, truncated to 64 KiB).
    pub stderr: String,

    /// Exit code from the process, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,

    /// Machine-readable error classification: "compile_error", "test_failure",
    /// "type_error", "lint_violation", "timeout", "io_error", etc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,

    /// Human-readable suggestion for what to do next.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_next_action: Option<String>,
}

impl VerifierResult {
    /// Creates a "pass" result.
    pub fn pass(command: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            status: "pass".to_string(),
            command: command.into(),
            duration_ms,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: Some(0),
            error_class: None,
            suggested_next_action: None,
        }
    }

    /// Creates a "fail" result from a non-zero exit code.
    pub fn fail(
        command: impl Into<String>,
        duration_ms: u64,
        stdout: String,
        stderr: String,
        exit_code: i32,
        error_class: Option<String>,
        suggested_next_action: Option<String>,
    ) -> Self {
        Self {
            status: "fail".to_string(),
            command: command.into(),
            duration_ms,
            stdout: truncate_output(&stdout),
            stderr: truncate_output(&stderr),
            exit_code: Some(exit_code),
            error_class,
            suggested_next_action,
        }
    }

    /// Creates an "error" result for a system-level failure (could not start command, etc.).
    pub fn error(
        command: impl Into<String>,
        error_class: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        let ec: String = error_class.into();
        let msg: String = message.into();
        let suggestion = match ec.as_str() {
            "timeout" => {
                Some("Increase timeout_ms or optimise the command to run faster.".to_string())
            }
            "io_error" => {
                Some("Check that the command binary is installed and accessible.".to_string())
            }
            _ => Some(format!("System error: {msg}")),
        };
        Self {
            status: "error".to_string(),
            command: command.into(),
            duration_ms: 0,
            stdout: String::new(),
            stderr: msg.clone(),
            exit_code: None,
            error_class: Some(ec),
            suggested_next_action: suggestion,
        }
    }

    /// Creates a "skipped" result.
    pub fn skipped(command: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            status: "skipped".to_string(),
            command: command.into(),
            duration_ms: 0,
            stdout: String::new(),
            stderr: reason.into(),
            exit_code: None,
            error_class: None,
            suggested_next_action: None,
        }
    }

    /// Returns true if the verification passed or was skipped.
    pub fn is_ok(&self) -> bool {
        matches!(self.status.as_str(), "pass" | "skipped")
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// VerifierRunner
// ═════════════════════════════════════════════════════════════════════════════

/// Runs verification commands using `tokio::process::Command`.
pub struct VerifierRunner;

impl VerifierRunner {
    /// Run a single verification spec and return the result.
    ///
    /// The command is executed via `sh -c <command>` under the hood, using
    /// the project root (or spec-level `cwd`) as the working directory.
    pub async fn run(spec: &VerifierSpec, project_root: &Path) -> VerifierResult {
        let command = spec.command.trim().to_string();
        if command.is_empty() {
            return VerifierResult::error(
                &command,
                "invalid_spec",
                "verifier command must not be empty",
            );
        }

        let cwd = spec
            .cwd
            .as_deref()
            .map(PathBuf::from)
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    project_root.join(path)
                }
            })
            .unwrap_or_else(|| project_root.to_path_buf());

        let started = std::time::Instant::now();
        let timeout_dur = Duration::from_millis(spec.timeout_ms.unwrap_or(120_000));

        // Use wait_with_output which handles pipe reading natively.
        let output_result = tokio::time::timeout(timeout_dur, async {
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(&command)
                .current_dir(&cwd)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output()
                .await
        })
        .await;

        let elapsed = started.elapsed().as_millis() as u64;

        match output_result {
            Ok(Ok(output)) => {
                let stdout = truncate_output(&String::from_utf8_lossy(&output.stdout));
                let stderr = truncate_output(&String::from_utf8_lossy(&output.stderr));
                let exit_code = output.status.code();

                if output.status.success() {
                    VerifierResult {
                        status: "pass".to_string(),
                        command,
                        duration_ms: elapsed,
                        stdout,
                        stderr,
                        exit_code: Some(0),
                        error_class: None,
                        suggested_next_action: None,
                    }
                } else {
                    let (error_class, suggested_next_action) =
                        classify_failure(&spec.verifier_type, &stderr, &stdout, exit_code);
                    VerifierResult::fail(
                        command,
                        elapsed,
                        stdout,
                        stderr,
                        exit_code.unwrap_or(1),
                        error_class,
                        suggested_next_action,
                    )
                }
            }
            Ok(Err(e)) => {
                // IO error (could not spawn, etc.)
                VerifierResult {
                    status: "error".to_string(),
                    command,
                    duration_ms: elapsed,
                    stdout: String::new(),
                    stderr: format!("failed to spawn command: {e}"),
                    exit_code: None,
                    error_class: Some("io_error".to_string()),
                    suggested_next_action: Some(
                        "Check that `sh` is available and the command is valid.".to_string(),
                    ),
                }
            }
            Err(_) => {
                // Timeout.
                VerifierResult {
                    status: "error".to_string(),
                    command,
                    duration_ms: elapsed,
                    stdout: String::new(),
                    stderr: "Command timed out.".to_string(),
                    exit_code: None,
                    error_class: Some("timeout".to_string()),
                    suggested_next_action: Some(
                        "Increase timeout_ms or optimise the command to run faster.".to_string(),
                    ),
                }
            }
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// VerificationStore
// ═════════════════════════════════════════════════════════════════════════════

/// Thread-safe store that keeps verification results keyed by `feature_id` or
/// `tool_call_id`.
#[derive(Debug, Clone, Default)]
pub struct VerificationStore {
    inner: Arc<RwLock<HashMap<String, VerifierResult>>>,
}

impl VerificationStore {
    /// Create a new, empty store.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Store a result under the given key.
    pub async fn store(&self, key: impl Into<String>, result: VerifierResult) {
        let mut map = self.inner.write().await;
        map.insert(key.into(), result);
    }

    /// Retrieve a result by key.
    pub async fn get(&self, key: &str) -> Option<VerifierResult> {
        let map = self.inner.read().await;
        map.get(key).cloned()
    }

    /// List all stored results, newest first (approximate: insertion order).
    pub async fn list(&self) -> Vec<(String, VerifierResult)> {
        let map = self.inner.read().await;
        let mut items: Vec<(String, VerifierResult)> =
            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        items.reverse();
        items
    }

    /// Remove a result by key.
    pub async fn remove(&self, key: &str) -> Option<VerifierResult> {
        let mut map = self.inner.write().await;
        map.remove(key)
    }

    /// Clear all stored results.
    pub async fn clear(&self) {
        let mut map = self.inner.write().await;
        map.clear();
    }

    /// Number of stored results.
    pub async fn len(&self) -> usize {
        let map = self.inner.read().await;
        map.len()
    }

    /// Returns true if there are no stored results.
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Internal helpers
// ═════════════════════════════════════════════════════════════════════════════

const OUTPUT_LIMIT_BYTES: usize = 64 * 1024;

fn truncate_output(s: &str) -> String {
    if s.len() <= OUTPUT_LIMIT_BYTES {
        s.to_string()
    } else {
        let mut truncated = s[..OUTPUT_LIMIT_BYTES].to_string();
        truncated.push_str("\n... <truncated>");
        truncated
    }
}

/// Classify a failure into error class and suggested next action.
fn classify_failure(
    verifier_type: &str,
    stderr: &str,
    stdout: &str,
    exit_code: Option<i32>,
) -> (Option<String>, Option<String>) {
    let combined = format!("{stdout}\n{stderr}").to_lowercase();

    match verifier_type {
        "build" => {
            if combined.contains("error[E") || combined.contains("error:") {
                (
                    Some("compile_error".to_string()),
                    Some("Fix the compilation error(s) shown above and rebuild.".to_string()),
                )
            } else if exit_code == Some(101) {
                (
                    Some("compile_error".to_string()),
                    Some(
                        "The build failed with a non-zero exit code. Inspect the output."
                            .to_string(),
                    ),
                )
            } else {
                (
                    Some("build_failure".to_string()),
                    Some("The build did not complete successfully. Check the output.".to_string()),
                )
            }
        }
        "test" => (
            Some("test_failure".to_string()),
            Some("Fix the failing test(s) and re-run.".to_string()),
        ),
        "typecheck" => {
            if combined.contains("error[E") {
                (
                    Some("type_error".to_string()),
                    Some("Fix the type errors shown above.".to_string()),
                )
            } else {
                (
                    Some("type_error".to_string()),
                    Some(
                        "Type checking failed. Inspect the output and fix type issues.".to_string(),
                    ),
                )
            }
        }
        "lint" => (
            Some("lint_violation".to_string()),
            Some("Fix the lint violations shown above.".to_string()),
        ),
        _ => {
            if combined.contains("not found")
                || combined.contains("no such file")
                || combined.contains("command not found")
            {
                (
                    Some("command_not_found".to_string()),
                    Some("The command was not found. Check that it is installed.".to_string()),
                )
            } else {
                (
                    Some("execution_failure".to_string()),
                    Some(
                        "The command exited with a non-zero status. Inspect the output."
                            .to_string(),
                    ),
                )
            }
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Tests
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    // ── VerifierSpec serde roundtrip ──────────────────────────────────────

    #[test]
    fn verifier_spec_serde_roundtrip_full() {
        let spec = VerifierSpec {
            verifier_type: "build".to_string(),
            command: "cargo build".to_string(),
            cwd: Some("/tmp".to_string()),
            timeout_ms: Some(300_000),
            required: true,
        };
        let json = serde_json::to_value(&spec).unwrap();
        let restored: VerifierSpec = serde_json::from_value(json).unwrap();
        assert_eq!(restored.verifier_type, "build");
        assert_eq!(restored.command, "cargo build");
        assert_eq!(restored.cwd, Some("/tmp".to_string()));
        assert_eq!(restored.timeout_ms, Some(300_000));
        assert!(restored.required);
    }

    #[test]
    fn verifier_spec_serde_roundtrip_minimal() {
        let spec = VerifierSpec {
            verifier_type: "test".to_string(),
            command: "cargo test".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_value(&spec).unwrap();
        let restored: VerifierSpec = serde_json::from_value(json).unwrap();
        assert_eq!(restored.verifier_type, "test");
        assert_eq!(restored.command, "cargo test");
        assert!(restored.required);
        assert!(restored.cwd.is_none());
        assert!(restored.timeout_ms.is_none());
    }

    #[test]
    fn verifier_spec_default_required_true() {
        assert!(default_required());
    }

    #[test]
    fn verifier_spec_serde_optional_fields_omit_when_none() {
        let spec = VerifierSpec {
            verifier_type: "lint".to_string(),
            command: "cargo clippy".to_string(),
            cwd: None,
            timeout_ms: None,
            required: false,
        };
        let json = serde_json::to_value(&spec).unwrap();
        // cwd and timeout_ms should be absent from serialized form.
        assert!(json.get("cwd").is_none());
        assert!(json.get("timeout_ms").is_none());
        assert_eq!(json["required"], false);
    }

    // ── VerifierResult construction ───────────────────────────────────────

    #[test]
    fn verifier_result_pass() {
        let r = VerifierResult::pass("echo ok", 42);
        assert_eq!(r.status, "pass");
        assert_eq!(r.command, "echo ok");
        assert_eq!(r.duration_ms, 42);
        assert!(r.is_ok());
    }

    #[test]
    fn verifier_result_fail() {
        let r = VerifierResult::fail(
            "cargo build",
            100,
            "stdout".to_string(),
            "error[E0425]".to_string(),
            101,
            Some("compile_error".to_string()),
            Some("Fix errors.".to_string()),
        );
        assert_eq!(r.status, "fail");
        assert_eq!(r.exit_code, Some(101));
        assert!(!r.is_ok());
    }

    #[test]
    fn verifier_result_error() {
        let r = VerifierResult::error("cargo build", "io_error", "command not found");
        assert_eq!(r.status, "error");
        assert!(!r.is_ok());
        assert!(r.suggested_next_action.is_some());
    }

    #[test]
    fn verifier_result_skipped() {
        let r = VerifierResult::skipped("cargo build", "not needed");
        assert_eq!(r.status, "skipped");
        assert!(r.is_ok());
    }

    #[test]
    fn verifier_result_is_ok_returns_false_for_error() {
        let r = VerifierResult::error("cmd", "io_error", "msg");
        assert!(!r.is_ok());
    }

    #[test]
    fn verifier_result_is_ok_returns_false_for_fail() {
        let r = VerifierResult::fail("cmd", 0, String::new(), String::new(), 1, None, None);
        assert!(!r.is_ok());
    }

    // ── VerifierResult serde roundtrip ────────────────────────────────────

    #[test]
    fn verifier_result_serde_roundtrip() {
        let r = VerifierResult::fail(
            "cargo check",
            150,
            "".to_string(),
            "error[E0308]: mismatched types".to_string(),
            101,
            Some("compile_error".to_string()),
            Some("Fix type mismatch.".to_string()),
        );
        let json = serde_json::to_value(&r).unwrap();
        let restored: VerifierResult = serde_json::from_value(json).unwrap();
        assert_eq!(restored.status, "fail");
        assert_eq!(restored.command, "cargo check");
        assert_eq!(restored.exit_code, Some(101));
        assert_eq!(restored.error_class, Some("compile_error".to_string()));
    }

    // ── VerifierRunner runs a simple command ──────────────────────────────

    #[tokio::test]
    async fn verifier_runner_runs_simple_pass() {
        let spec = VerifierSpec {
            verifier_type: "command".to_string(),
            command: "echo hello".to_string(),
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let result = VerifierRunner::run(&spec, dir.path()).await;
        assert_eq!(result.status, "pass", "echo should succeed: {:?}", result);
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("hello"), "stdout: {}", result.stdout);
        assert!(result.duration_ms > 0);
    }

    #[tokio::test]
    async fn verifier_runner_captures_exit_code() {
        let spec = VerifierSpec {
            verifier_type: "command".to_string(),
            command: "exit 42".to_string(),
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let result = VerifierRunner::run(&spec, dir.path()).await;
        assert_eq!(result.status, "fail", "exit 42 should fail");
        assert_eq!(result.exit_code, Some(42));
    }

    #[tokio::test]
    async fn verifier_runner_captures_stderr() {
        let spec = VerifierSpec {
            verifier_type: "command".to_string(),
            command: "echo error >&2 && exit 1".to_string(),
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let result = VerifierRunner::run(&spec, dir.path()).await;
        assert_eq!(result.status, "fail");
        assert!(result.stderr.contains("error"), "stderr: {}", result.stderr);
    }

    #[tokio::test]
    async fn verifier_runner_resolves_relative_cwd_under_project_root() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("checks");
        std::fs::create_dir_all(&subdir).unwrap();
        std::fs::write(subdir.join("marker.txt"), "ok").unwrap();
        let spec = VerifierSpec {
            verifier_type: "command".to_string(),
            command: "cat marker.txt".to_string(),
            cwd: Some("checks".to_string()),
            ..Default::default()
        };

        let result = VerifierRunner::run(&spec, dir.path()).await;
        assert_eq!(result.status, "pass", "{result:?}");
        assert_eq!(result.stdout, "ok");
    }

    #[tokio::test]
    async fn verifier_runner_empty_command_returns_error() {
        let spec = VerifierSpec {
            verifier_type: "command".to_string(),
            command: "".to_string(),
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let result = VerifierRunner::run(&spec, dir.path()).await;
        assert_eq!(result.status, "error");
        assert_eq!(result.error_class, Some("invalid_spec".to_string()));
    }

    #[tokio::test]
    async fn verifier_runner_timeout_returns_error() {
        let spec = VerifierSpec {
            verifier_type: "command".to_string(),
            command: "sleep 10".to_string(),
            timeout_ms: Some(1),
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let result = VerifierRunner::run(&spec, dir.path()).await;
        assert_eq!(
            result.status, "error",
            "sleep should time out: {:?}",
            result
        );
        assert_eq!(result.error_class, Some("timeout".to_string()));
    }

    // ── VerificationStore ─────────────────────────────────────────────────

    #[tokio::test]
    async fn verification_store_store_and_get() {
        let store = VerificationStore::new();
        let result = VerifierResult::pass("echo hi", 10);
        store.store("feature-1", result.clone()).await;
        let retrieved = store.get("feature-1").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().status, "pass");
    }

    #[tokio::test]
    async fn verification_store_get_missing_returns_none() {
        let store = VerificationStore::new();
        let retrieved = store.get("nonexistent").await;
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn verification_store_list_returns_all() {
        let store = VerificationStore::new();
        store.store("a", VerifierResult::pass("cmd1", 1)).await;
        store.store("b", VerifierResult::pass("cmd2", 2)).await;
        let items = store.list().await;
        assert_eq!(items.len(), 2);
    }

    #[tokio::test]
    async fn verification_store_remove() {
        let store = VerificationStore::new();
        store.store("x", VerifierResult::pass("cmd", 1)).await;
        let removed = store.remove("x").await;
        assert!(removed.is_some());
        assert!(store.get("x").await.is_none());
    }

    #[tokio::test]
    async fn verification_store_clear() {
        let store = VerificationStore::new();
        store.store("a", VerifierResult::pass("cmd", 1)).await;
        store.clear().await;
        assert!(store.is_empty().await);
    }

    #[tokio::test]
    async fn verification_store_is_empty() {
        let store = VerificationStore::new();
        assert!(store.is_empty().await);
        store.store("k", VerifierResult::pass("cmd", 1)).await;
        assert!(!store.is_empty().await);
    }

    // ── classify_failure ──────────────────────────────────────────────────

    #[test]
    fn classify_failure_build_compile_error() {
        let (class, _) =
            classify_failure("build", "error[E0425]: cannot find value", "", Some(101));
        assert_eq!(class, Some("compile_error".to_string()));
    }

    #[test]
    fn classify_failure_test_failure() {
        let (class, _) = classify_failure("test", "test result: FAILED", "", Some(1));
        assert_eq!(class, Some("test_failure".to_string()));
    }

    #[test]
    fn classify_failure_typecheck_type_error() {
        let (class, _) =
            classify_failure("typecheck", "error[E0308]: mismatched types", "", Some(1));
        assert_eq!(class, Some("type_error".to_string()));
    }

    #[test]
    fn classify_failure_lint_violation() {
        let (class, _) = classify_failure("lint", "clippy::style", "", Some(1));
        assert_eq!(class, Some("lint_violation".to_string()));
    }

    #[test]
    fn classify_failure_command_not_found() {
        let (class, _) = classify_failure("command", "command not found: foo", "", Some(127));
        assert_eq!(class, Some("command_not_found".to_string()));
    }

    #[test]
    fn classify_failure_generic() {
        let (class, _) = classify_failure("command", "something broke", "", Some(1));
        assert_eq!(class, Some("execution_failure".to_string()));
    }

    #[test]
    fn classify_failure_build_no_rust_error() {
        let (class, _) = classify_failure("build", "linking failed", "", Some(1));
        assert_eq!(class, Some("build_failure".to_string()));
    }
}
