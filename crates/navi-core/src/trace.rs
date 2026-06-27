use crate::security::redact_secrets;
use crate::tool::{ToolInvocation, ToolResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Structured trace for a single turn of agent execution.
///
/// Separates trace data from session events. Traces are focused on harness
/// metrics, tool calls, verifiers, and outcomes — not full conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnTrace {
    /// Schema version for forward compatibility.
    pub version: u32,

    /// Unique turn identifier.
    pub turn_id: String,

    /// Session this turn belongs to.
    pub session_id: String,

    /// Unix timestamp (milliseconds) when the turn started.
    pub started_at: u64,

    /// Unix timestamp (milliseconds) when the turn ended.
    pub ended_at: u64,

    /// Model provider used.
    pub model_provider: String,

    /// Model name used.
    pub model_name: String,

    /// The user task or prompt that started this turn.
    pub task: String,

    /// How many tool definitions were visible to the model.
    pub visible_tool_count: usize,

    /// Names of visible tools.
    #[serde(default)]
    pub visible_tools: Vec<String>,

    /// Names of deferred tools discovered via tool.search.
    #[serde(default)]
    pub deferred_tools_discovered: Vec<String>,

    /// All tool calls made during this turn, in order.
    #[serde(default)]
    pub tool_calls: Vec<ToolCallTrace>,

    /// Approval decisions made during this turn.
    #[serde(default)]
    pub approvals: Vec<ApprovalTrace>,

    /// Verifier results from this turn.
    #[serde(default)]
    pub verifier_results: Vec<VerifierTrace>,

    /// Metrics summary.
    pub metrics: TurnMetrics,

    /// Outcome classification.
    pub outcome: TurnOutcome,

    /// Human-readable final message.
    #[serde(default)]
    pub final_message: String,
}

/// Trace data for a single tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallTrace {
    /// Tool invocation details.
    pub invocation: ToolInvocation,
    /// Tool result.
    pub result: ToolResult,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Whether this was a retry.
    #[serde(default)]
    pub was_retry: bool,
    /// Whether the tool triggered a rollback.
    #[serde(default)]
    pub was_rolled_back: bool,
    /// Error code if the tool failed.
    #[serde(default)]
    pub error_code: Option<String>,
}

/// Trace data for an approval decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalTrace {
    /// Tool name that required approval.
    pub tool_name: String,
    /// Risk level.
    pub risk: String,
    /// Whether approved or denied.
    pub decision: String,
    /// Duration in milliseconds.
    pub duration_ms: u64,
}

/// Trace data for a verifier run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifierTrace {
    /// Verifier name (e.g. "verify.test", "verify.build").
    pub verifier: String,
    /// Command that was run.
    pub command: String,
    /// Pass/fail.
    pub passed: bool,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Exit code.
    pub exit_code: Option<i32>,
}

/// Aggregated turn metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnMetrics {
    /// Total input tokens consumed.
    pub input_tokens: u64,
    /// Total output tokens produced.
    pub output_tokens: u64,
    /// Cache creation tokens.
    pub cache_creation_tokens: u64,
    /// Cache read tokens.
    pub cache_read_tokens: u64,
    /// Total tool calls.
    pub tool_call_count: usize,
    /// Failed tool calls.
    pub failed_tool_calls: usize,
    /// Approval prompts shown.
    pub approval_count: usize,
    /// Verifier runs executed.
    pub verifier_count: usize,
    /// Retries triggered.
    pub retry_count: usize,
    /// Rollbacks executed.
    pub rollback_count: usize,
    /// Wall time in milliseconds.
    pub wall_time_ms: u64,
}

impl Default for TurnMetrics {
    fn default() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            tool_call_count: 0,
            failed_tool_calls: 0,
            approval_count: 0,
            verifier_count: 0,
            retry_count: 0,
            rollback_count: 0,
            wall_time_ms: 0,
        }
    }
}

/// Outcome of a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnOutcome {
    /// Turn completed successfully with a final answer.
    Success,
    /// Turn completed but with one or more tool failures.
    PartialSuccess,
    /// Turn was stopped by the harness.
    Stopped(String),
    /// Turn failed with an unrecoverable error.
    Failed(String),
}

impl TurnTrace {
    /// Current trace schema version.
    pub const CURRENT_VERSION: u32 = 1;

    /// Creates a new turn trace with the given basic fields.
    pub fn new(
        turn_id: impl Into<String>,
        session_id: impl Into<String>,
        model_provider: impl Into<String>,
        model_name: impl Into<String>,
        task: impl Into<String>,
    ) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            turn_id: turn_id.into(),
            session_id: session_id.into(),
            started_at: current_unix_millis(),
            ended_at: 0,
            model_provider: model_provider.into(),
            model_name: model_name.into(),
            task: task.into(),
            visible_tool_count: 0,
            visible_tools: Vec::new(),
            deferred_tools_discovered: Vec::new(),
            tool_calls: Vec::new(),
            approvals: Vec::new(),
            verifier_results: Vec::new(),
            metrics: TurnMetrics::default(),
            outcome: TurnOutcome::Success,
            final_message: String::new(),
        }
    }

    /// Finalizes the trace by setting end time and wall time.
    pub fn finalize(&mut self) {
        self.ended_at = current_unix_millis();
        self.metrics.wall_time_ms = self.ended_at.saturating_sub(self.started_at);
    }

    /// Records a tool call in the trace.
    pub fn record_tool_call(
        &mut self,
        invocation: &ToolInvocation,
        result: &ToolResult,
        duration_ms: u64,
    ) {
        self.metrics.tool_call_count += 1;
        if !result.ok {
            self.metrics.failed_tool_calls += 1;
        }
        self.tool_calls.push(ToolCallTrace {
            invocation: invocation.clone(),
            result: result.clone(),
            duration_ms,
            was_retry: false,
            was_rolled_back: false,
            error_code: result
                .output
                .get("error_code")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        });
    }

    /// Records an approval decision.
    pub fn record_approval(
        &mut self,
        tool_name: &str,
        risk: &str,
        approved: bool,
        duration_ms: u64,
    ) {
        self.metrics.approval_count += 1;
        self.approvals.push(ApprovalTrace {
            tool_name: tool_name.to_string(),
            risk: risk.to_string(),
            decision: if approved {
                "approved".to_string()
            } else {
                "denied".to_string()
            },
            duration_ms,
        });
    }

    /// Records a verifier run.
    pub fn record_verifier(
        &mut self,
        verifier: &str,
        command: &str,
        passed: bool,
        duration_ms: u64,
        exit_code: Option<i32>,
    ) {
        self.metrics.verifier_count += 1;
        self.verifier_results.push(VerifierTrace {
            verifier: verifier.to_string(),
            command: redact_secrets(command),
            passed,
            duration_ms,
            exit_code,
        });
    }

    /// Returns a copy of this trace with secret-like content redacted.
    pub fn redacted(&self) -> Self {
        let mut trace = self.clone();
        trace.task = redact_secrets(&trace.task);
        trace.final_message = redact_secrets(&trace.final_message);
        for call in &mut trace.tool_calls {
            call.invocation.input = redact_json_value(&call.invocation.input);
            call.result.output = redact_json_value(&call.result.output);
        }
        for verifier in &mut trace.verifier_results {
            verifier.command = redact_secrets(&verifier.command);
        }
        trace
    }
}

/// Persists structured traces to disk for replay, debugging, and metrics.
#[derive(Debug, Clone)]
pub struct TraceStore {
    root: PathBuf,
}

impl TraceStore {
    /// Creates a new trace store at `<data_dir>/traces/`.
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("traces"),
        }
    }

    /// Returns the root directory for trace storage.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Saves a turn trace as a JSONL line to the session trace file.
    ///
    /// Returns the path to the trace file.
    pub fn save_trace(&self, trace: &TurnTrace) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(&self.root)?;
        let path = self.root.join(format!("{}.jsonl", trace.session_id));
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let redacted = trace.redacted();
        let line = serde_json::to_string(&redacted)?;
        use std::io::Write;
        writeln!(file, "{line}")?;
        Ok(path)
    }

    /// Loads all turn traces for the given session.
    pub fn load_session_traces(&self, session_id: &str) -> Vec<TurnTrace> {
        let path = self.root.join(format!("{session_id}.jsonl"));
        let Ok(content) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        content
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// Lists all session IDs with stored traces.
    pub fn list_sessions(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
            .filter_map(|e| {
                e.path()
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            })
            .collect()
    }
}

fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn redact_json_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => serde_json::Value::String(redact_secrets(text)),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(redact_json_value).collect())
        }
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), redact_json_value(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn turn_trace_creation_and_finalize() {
        let mut trace = TurnTrace::new("turn-1", "session-1", "openai", "gpt-4", "test task");
        assert_eq!(trace.version, TurnTrace::CURRENT_VERSION);
        assert_eq!(trace.turn_id, "turn-1");
        assert_eq!(trace.ended_at, 0);

        trace.finalize();
        assert!(trace.ended_at >= trace.started_at);
        assert!(trace.metrics.wall_time_ms > 0 || trace.ended_at == trace.started_at);
    }

    #[test]
    fn trace_records_tool_call() {
        let mut trace = TurnTrace::new("t1", "s1", "p", "m", "task");
        let inv = ToolInvocation {
            id: "c1".to_string(),
            tool_name: "read".to_string(),
            input: json!({"path": "file.txt"}),
        };
        let result = ToolResult {
            invocation_id: "c1".to_string(),
            ok: true,
            output: json!("content"),
        };
        trace.record_tool_call(&inv, &result, 100);
        assert_eq!(trace.metrics.tool_call_count, 1);
        assert_eq!(trace.metrics.failed_tool_calls, 0);
    }

    #[test]
    fn trace_records_failed_tool() {
        let mut trace = TurnTrace::new("t1", "s1", "p", "m", "task");
        let inv = ToolInvocation {
            id: "c1".to_string(),
            tool_name: "bash".to_string(),
            input: json!({"command": "false"}),
        };
        let result = ToolResult {
            invocation_id: "c1".to_string(),
            ok: false,
            output: json!({"error_code": "command_failed", "message": "exit 1"}),
        };
        trace.record_tool_call(&inv, &result, 50);
        assert_eq!(trace.metrics.failed_tool_calls, 1);
        assert_eq!(
            trace.tool_calls[0].error_code.as_deref(),
            Some("command_failed")
        );
    }

    #[test]
    fn trace_records_approval_and_verifier() {
        let mut trace = TurnTrace::new("t1", "s1", "p", "m", "task");
        trace.record_approval("write", "write", true, 500);
        trace.record_verifier("verify.test", "cargo test", true, 3000, Some(0));
        assert_eq!(trace.metrics.approval_count, 1);
        assert_eq!(trace.metrics.verifier_count, 1);
    }

    #[test]
    fn trace_store_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::new(dir.path());
        let mut trace = TurnTrace::new("turn-1", "session-test", "openai", "gpt-4", "task");
        trace.finalize();
        store.save_trace(&trace).unwrap();
        let loaded = store.load_session_traces("session-test");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].turn_id, "turn-1");
    }

    #[test]
    fn trace_store_redacts_secrets_before_persisting() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::new(dir.path());
        let mut trace = TurnTrace::new(
            "turn-secret",
            "session-secret",
            "openai",
            "gpt-4",
            "use OPENAI_API_KEY=sk-proj-1234567890abcdef",
        );
        trace.record_tool_call(
            &ToolInvocation {
                id: "c1".to_string(),
                tool_name: "bash".to_string(),
                input: json!({"command": "echo sk-proj-1234567890abcdef"}),
            },
            &ToolResult {
                invocation_id: "c1".to_string(),
                ok: true,
                output: json!({"stdout": "sk-proj-1234567890abcdef"}),
            },
            1,
        );
        trace.finalize();

        let path = store.save_trace(&trace).unwrap();
        let persisted = std::fs::read_to_string(path).unwrap();
        assert!(!persisted.contains("sk-proj-1234567890abcdef"));
        assert!(persisted.contains("<redacted>"));
    }

    #[test]
    fn trace_store_save_appends() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::new(dir.path());
        let mut t1 = TurnTrace::new("turn-1", "session-multi", "p", "m", "task1");
        t1.finalize();
        let mut t2 = TurnTrace::new("turn-2", "session-multi", "p", "m", "task2");
        t2.finalize();
        store.save_trace(&t1).unwrap();
        store.save_trace(&t2).unwrap();
        let loaded = store.load_session_traces("session-multi");
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn trace_store_list_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::new(dir.path());
        let mut t1 = TurnTrace::new("t1", "session-a", "p", "m", "task");
        t1.finalize();
        let mut t2 = TurnTrace::new("t2", "session-b", "p", "m", "task");
        t2.finalize();
        store.save_trace(&t1).unwrap();
        store.save_trace(&t2).unwrap();
        let sessions = store.list_sessions();
        assert!(sessions.contains(&"session-a".to_string()));
        assert!(sessions.contains(&"session-b".to_string()));
    }

    #[test]
    fn trace_store_empty_for_missing_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::new(dir.path());
        let loaded = store.load_session_traces("nonexistent");
        assert!(loaded.is_empty());
    }
}
