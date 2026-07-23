use crate::capability::CapabilityLedgerEntry;
use crate::diagnose::FailureKind;
use crate::event::{AgentEvent, ApprovalDecision};
use crate::security::redact_secrets;
use crate::tool::{ToolInvocation, ToolResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

    /// Capability lifecycle entries recorded during this turn.
    #[serde(default)]
    pub capabilities: Vec<CapabilityLedgerEntry>,

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

    /// Machine-readable failure classification, if the turn did not fully succeed.
    #[serde(default)]
    pub failure_kind: Option<crate::diagnose::FailureKind>,

    /// Number of self-repair attempts applied during this turn.
    #[serde(default)]
    pub recovery_attempts: u32,

    /// Human-readable diagnosis/suggested recovery action.
    #[serde(default)]
    pub diagnosis: Option<String>,

    /// Concise one-line summary of the turn for AI consumption and quick scanning.
    #[serde(default)]
    pub turn_summary: String,
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
    /// Provider stream succeeded but produced no assistant content.
    EmptyResponse,
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
            capabilities: Vec::new(),
            verifier_results: Vec::new(),
            metrics: TurnMetrics::default(),
            outcome: TurnOutcome::Success,
            final_message: String::new(),
            failure_kind: None,
            recovery_attempts: 0,
            diagnosis: None,
            turn_summary: String::new(),
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

    /// Records a capability lifecycle entry.
    pub fn record_capability(&mut self, entry: CapabilityLedgerEntry) {
        self.capabilities.push(entry);
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
        trace.diagnosis = trace.diagnosis.as_ref().map(|s| redact_secrets(s));
        trace.turn_summary = redact_secrets(&trace.turn_summary);
        for call in &mut trace.tool_calls {
            call.invocation.input = redact_json_value(&call.invocation.input);
            call.result.output = redact_json_value(&call.result.output);
        }
        for verifier in &mut trace.verifier_results {
            verifier.command = redact_secrets(&verifier.command);
        }
        for capability in &mut trace.capabilities {
            capability.justification = redact_secrets(&capability.justification);
        }
        trace
    }

    /// Builds a concise, AI-friendly one-line summary of this turn.
    pub fn summarize(&self) -> String {
        let outcome_label = match self.outcome {
            TurnOutcome::Success => "success",
            TurnOutcome::PartialSuccess => "partial_success",
            TurnOutcome::Stopped(_) => "stopped",
            TurnOutcome::Failed(_) => "failed",
            TurnOutcome::EmptyResponse => "empty_response",
        };
        let mut summary = format!(
            "{outcome_label} turn with {}/{} in {}ms; {} tool calls ({} failed); in={} out={} tokens",
            self.model_provider,
            self.model_name,
            self.metrics.wall_time_ms,
            self.metrics.tool_call_count,
            self.metrics.failed_tool_calls,
            self.metrics.input_tokens,
            self.metrics.output_tokens
        );
        if self.recovery_attempts > 0 {
            summary.push_str(&format!(
                "; {} self-repair attempt(s)",
                self.recovery_attempts
            ));
        }
        if let Some(ref diagnosis) = self.diagnosis {
            summary.push_str(&format!("; diagnosis: {diagnosis}"));
        }
        summary
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

    /// Replaces the trace JSONL for a session with the provided traces.
    pub fn save_session_traces(
        &self,
        session_id: &str,
        traces: &[TurnTrace],
    ) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(&self.root)?;
        let path = self.root.join(format!("{session_id}.jsonl"));
        let mut file = std::fs::File::create(&path)?;
        use std::io::Write;
        for trace in traces {
            let redacted = trace.redacted();
            let line = serde_json::to_string(&redacted)?;
            writeln!(file, "{line}")?;
        }
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

pub fn turn_traces_from_events(
    session_id: &str,
    model_provider: &str,
    model_name: &str,
    events: &[AgentEvent],
) -> Vec<TurnTrace> {
    let mut traces = Vec::new();
    let mut current: Option<TurnTrace> = None;
    let mut invocations: HashMap<String, ToolInvocation> = HashMap::new();

    for event in events {
        match event {
            AgentEvent::UserTaskSubmitted { text, .. } => {
                if let Some(mut trace) = current.take() {
                    trace.finalize();
                    traces.push(trace);
                }
                invocations.clear();
                current = Some(TurnTrace::new(
                    format!("{}-trace-{}", session_id, traces.len() + 1),
                    session_id.to_string(),
                    model_provider.to_string(),
                    model_name.to_string(),
                    text.clone(),
                ));
            }
            AgentEvent::ToolRequested(invocation) => {
                invocations.insert(invocation.id.clone(), invocation.clone());
            }
            AgentEvent::ToolCompleted(result) => {
                if let Some(trace) = current.as_mut() {
                    let invocation =
                        invocations
                            .get(&result.invocation_id)
                            .cloned()
                            .unwrap_or(ToolInvocation {
                                id: result.invocation_id.clone(),
                                tool_name: result
                                    .output
                                    .get("tool")
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or("unknown")
                                    .to_string(),
                                input: serde_json::json!({}),
                            });
                    if !result.ok && trace.failure_kind.is_none() {
                        trace.failure_kind = Some(FailureKind::ToolFailure);
                    }
                    trace.record_tool_call(&invocation, result, 0);
                    if invocation.tool_name == "verifier" {
                        if let Some(verifier_trace) = verifier_trace_from_tool(&invocation, result)
                        {
                            trace.metrics.verifier_count += 1;
                            trace.verifier_results.push(verifier_trace);
                        }
                    }
                }
            }
            AgentEvent::ApprovalRequested(request) => {
                if let Some(trace) = current.as_mut() {
                    trace.metrics.approval_count += 1;
                    trace.approvals.push(ApprovalTrace {
                        tool_name: request.id.clone(),
                        risk: format!("{:?}", request.risk).to_lowercase(),
                        decision: "requested".to_string(),
                        duration_ms: 0,
                    });
                }
            }
            AgentEvent::ApprovalResolved(decision) => {
                if let Some(trace) = current.as_mut() {
                    let (id, label) = match decision {
                        ApprovalDecision::Approved { id } => (id, "approved"),
                        ApprovalDecision::Denied { id } => (id, "denied"),
                    };
                    trace.approvals.push(ApprovalTrace {
                        tool_name: id.clone(),
                        risk: String::new(),
                        decision: label.to_string(),
                        duration_ms: 0,
                    });
                }
            }
            AgentEvent::CapabilityRecorded(entry) => {
                if let Some(trace) = current.as_mut() {
                    trace.record_capability(entry.clone());
                }
            }
            AgentEvent::UsageReported {
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            } => {
                if let Some(trace) = current.as_mut() {
                    trace.metrics.input_tokens =
                        trace.metrics.input_tokens.saturating_add(*input_tokens);
                    trace.metrics.output_tokens =
                        trace.metrics.output_tokens.saturating_add(*output_tokens);
                    trace.metrics.cache_creation_tokens = trace
                        .metrics
                        .cache_creation_tokens
                        .saturating_add(*cache_creation_tokens);
                    trace.metrics.cache_read_tokens = trace
                        .metrics
                        .cache_read_tokens
                        .saturating_add(*cache_read_tokens);
                }
            }
            AgentEvent::ModelOutput { text, .. } => {
                if let Some(trace) = current.as_mut() {
                    trace.final_message = text.clone();
                }
            }
            AgentEvent::Error { message } => {
                if let Some(trace) = current.as_mut() {
                    trace.outcome = TurnOutcome::Failed(message.clone());
                    trace.failure_kind = trace.failure_kind.or(Some(FailureKind::ProviderError));
                }
            }
            AgentEvent::HarnessStopped { reason, .. } => {
                if let Some(trace) = current.as_mut() {
                    trace.outcome = TurnOutcome::Stopped(reason.clone());
                    trace.failure_kind = trace.failure_kind.or(Some(FailureKind::HarnessStopped));
                }
            }
            AgentEvent::HarnessTrace(value) => {
                if let Some(trace) = current.as_mut() {
                    handle_harness_trace(trace, value);
                }
            }
            _ => {}
        }
    }

    if let Some(mut trace) = current {
        trace.finalize();
        if trace.outcome == TurnOutcome::Success {
            if trace.final_message.trim().is_empty() {
                trace.outcome = TurnOutcome::EmptyResponse;
                trace.failure_kind = trace.failure_kind.or(Some(FailureKind::EmptyResponse));
            } else if trace.metrics.failed_tool_calls > 0 {
                trace.outcome = TurnOutcome::PartialSuccess;
                trace.failure_kind = trace.failure_kind.or(Some(FailureKind::ToolFailure));
            }
        } else {
            trace.failure_kind = trace.failure_kind.or(match trace.outcome {
                TurnOutcome::Failed(_) => Some(FailureKind::ProviderError),
                TurnOutcome::Stopped(_) => Some(FailureKind::HarnessStopped),
                TurnOutcome::PartialSuccess => Some(FailureKind::ToolFailure),
                TurnOutcome::EmptyResponse => Some(FailureKind::EmptyResponse),
                TurnOutcome::Success => None,
            });
        }
        trace.turn_summary = trace.summarize();
        traces.push(trace);
    }

    traces
}

fn handle_harness_trace(trace: &mut TurnTrace, value: &serde_json::Value) {
    let Some(kind) = value.get("kind").and_then(serde_json::Value::as_str) else {
        // Legacy request summary (no `kind`). Use visible tool count if present.
        if let Some(tools) = value.get("tools").and_then(serde_json::Value::as_u64) {
            trace.visible_tool_count = tools as usize;
        }
        return;
    };

    match kind {
        "request" => {
            if let Some(tools) = value.get("tools").and_then(serde_json::Value::as_u64) {
                trace.visible_tool_count = tools as usize;
            }
        }
        "repair_attempt" => {
            trace.recovery_attempts += 1;
        }
        "diagnosis" => {
            if let Some(summary) = value.get("summary").and_then(serde_json::Value::as_str) {
                trace.diagnosis = Some(summary.to_string());
            }
            if trace.failure_kind.is_none() {
                if let Some(fk) = value
                    .get("failure_kind")
                    .and_then(|v| serde_json::from_value::<FailureKind>(v.clone()).ok())
                {
                    trace.failure_kind = Some(fk);
                }
            }
        }
        _ => {}
    }
}

fn verifier_trace_from_tool(
    invocation: &ToolInvocation,
    result: &ToolResult,
) -> Option<VerifierTrace> {
    let command = result
        .output
        .get("command")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            invocation
                .input
                .get("command")
                .and_then(serde_json::Value::as_str)
        })?;
    let status = result
        .output
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(if result.ok { "pass" } else { "fail" });
    Some(VerifierTrace {
        verifier: invocation
            .input
            .get("verifier")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("command")
            .to_string(),
        command: redact_secrets(command),
        passed: matches!(status, "pass" | "skipped"),
        duration_ms: result
            .output
            .get("duration_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default(),
        exit_code: result
            .output
            .get("exit_code")
            .and_then(serde_json::Value::as_i64)
            .map(|value| value as i32),
    })
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
    fn trace_records_and_redacts_capability_entries() {
        let mut trace = TurnTrace::new("t1", "s1", "p", "m", "task");
        trace.record_capability(CapabilityLedgerEntry {
            capability: crate::capability::Capability::RepoRead,
            scope: crate::capability::CapabilityScope::Turn("t1".to_string()),
            decision: crate::capability::CapabilityDecision::Granted,
            at_ms: 1,
            justification: "read token=sk-proj-1234567890abcdef".to_string(),
        });

        let redacted = trace.redacted();
        assert_eq!(redacted.capabilities.len(), 1);
        assert!(!redacted.capabilities[0].justification.contains("sk-proj"));
        assert!(
            redacted.capabilities[0]
                .justification
                .contains("<redacted>")
        );
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
    fn trace_store_save_session_traces_replaces_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::new(dir.path());
        let mut first = TurnTrace::new("t1", "session-replace", "p", "m", "task1");
        first.finalize();
        store.save_trace(&first).unwrap();

        let mut second = TurnTrace::new("t2", "session-replace", "p", "m", "task2");
        second.finalize();
        store
            .save_session_traces("session-replace", &[second])
            .unwrap();

        let loaded = store.load_session_traces("session-replace");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].turn_id, "t2");
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

    #[test]
    fn session_events_generate_turn_trace_with_capabilities() {
        let events = vec![
            AgentEvent::UserTaskSubmitted {
                text: "verify".to_string(),
                content_parts: Vec::new(),
                submitted_at: None,
            },
            AgentEvent::CapabilityRecorded(CapabilityLedgerEntry {
                capability: crate::capability::Capability::RepoRead,
                scope: crate::capability::CapabilityScope::SingleCall("read".to_string()),
                decision: crate::capability::CapabilityDecision::Requested,
                at_ms: 1,
                justification: "read".to_string(),
            }),
            AgentEvent::ToolRequested(ToolInvocation {
                id: "verify".to_string(),
                tool_name: "verifier".to_string(),
                input: json!({"verifier": "test", "command": "true"}),
            }),
            AgentEvent::ToolCompleted(ToolResult {
                invocation_id: "verify".to_string(),
                ok: true,
                output: json!({"status": "pass", "command": "true", "duration_ms": 3, "exit_code": 0}),
            }),
            AgentEvent::UsageReported {
                input_tokens: 100,
                output_tokens: 20,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
            AgentEvent::ModelOutput {
                text: "done".to_string(),
                thinking: None,
            },
        ];

        let traces = turn_traces_from_events("s", "p", "m", &events);

        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].task, "verify");
        assert_eq!(traces[0].tool_calls.len(), 1);
        assert_eq!(traces[0].verifier_results.len(), 1);
        assert_eq!(traces[0].capabilities.len(), 1);
        assert_eq!(traces[0].metrics.input_tokens, 100);
    }

    #[test]
    fn empty_model_output_classified_as_empty_response() {
        let events = vec![
            AgentEvent::UserTaskSubmitted {
                text: "hello".to_string(),
                content_parts: Vec::new(),
                submitted_at: None,
            },
            AgentEvent::UsageReported {
                input_tokens: 50,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
            AgentEvent::ModelOutput {
                text: "".to_string(),
                thinking: None,
            },
        ];

        let traces = turn_traces_from_events("s", "p", "m", &events);

        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].outcome, TurnOutcome::EmptyResponse);
        assert_eq!(traces[0].failure_kind, Some(FailureKind::EmptyResponse));
        assert!(traces[0].turn_summary.contains("empty_response"));
    }
}
