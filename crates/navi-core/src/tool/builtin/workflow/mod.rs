//! Built-in `workflow` tool — sandboxed Lua 5.4 multi-agent orchestration.

mod backends;
mod journal;
mod policy;
mod runtime;
mod types;

#[cfg(test)]
mod tests;

use self::policy::{RunPolicy, clamp_max_agents, clamp_max_parallel, default_run_policy};
pub use backends::{SubagentBridgeBackend, WorkerProbeBackend};

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use self::journal::WorkflowJournal;
use self::runtime::{LuaRunInput, run_lua_workflow};
use self::types::*;
use super::helpers;
use crate::config::WorkflowConfig;
use crate::security::{SecurityPolicy, redact_secrets};
use crate::tool::{
    Tool, ToolDefinition, ToolInvocation, ToolInvocationContext, ToolKind, ToolResult,
};

static RUN_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Pluggable worker backend.
#[async_trait]
pub trait AgentBackend: Send + Sync {
    async fn run_agent(&self, request: AgentRequest) -> AgentBackendResult;
}

/// Mock backend for unit tests (no live model).
#[derive(Default)]
pub struct MockAgentBackend {
    pub calls: std::sync::Mutex<Vec<AgentRequest>>,
    pub delay_ms: u64,
    pub in_flight: Option<Arc<AtomicUsize>>,
    pub peak_in_flight: Option<Arc<AtomicUsize>>,
}

#[async_trait]
impl AgentBackend for MockAgentBackend {
    async fn run_agent(&self, request: AgentRequest) -> AgentBackendResult {
        if let Some(ref inflight) = self.in_flight {
            let n = inflight.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some(ref peak) = self.peak_in_flight {
                peak.fetch_max(n, Ordering::SeqCst);
            }
        }
        {
            let mut guard = self.calls.lock().unwrap_or_else(|e| e.into_inner());
            guard.push(request.clone());
        }

        if self.delay_ms > 0 {
            let delay = std::time::Duration::from_millis(self.delay_ms);
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = request.cancel_token.notified() => {
                    if let Some(ref inflight) = self.in_flight {
                        inflight.fetch_sub(1, Ordering::SeqCst);
                    }
                    return AgentBackendResult {
                        ok: false,
                        output: json!({"error": "cancelled"}),
                        error: Some("cancelled".into()),
                    };
                }
            }
        }

        if request.cancel_token.is_requested() {
            if let Some(ref inflight) = self.in_flight {
                inflight.fetch_sub(1, Ordering::SeqCst);
            }
            return AgentBackendResult {
                ok: false,
                output: json!({"error": "cancelled"}),
                error: Some("cancelled".into()),
            };
        }

        if let Some(ref inflight) = self.in_flight {
            inflight.fetch_sub(1, Ordering::SeqCst);
        }

        AgentBackendResult {
            ok: true,
            output: json!({
                "ok": true,
                "prompt": request.prompt,
                "label": request.label,
                "agent_index": request.agent_index,
                "profile": request.effective.profile,
                "tools": request.effective.tools,
                "create_files": request.effective.create_files,
                "create_dirs": request.effective.create_dirs,
                "write_allow": request.effective.write_allow,
                "path_allow": request.effective.path_allow,
                "path_deny": request.effective.path_deny,
            }),
            error: None,
        }
    }
}

/// Strips nested orchestration tools and delegates.
pub struct PolicyAgentBackend {
    pub inner: Arc<dyn AgentBackend>,
}

#[async_trait]
impl AgentBackend for PolicyAgentBackend {
    async fn run_agent(&self, request: AgentRequest) -> AgentBackendResult {
        for banned in NESTED_WORKFLOW_TOOLS {
            if request.effective.tools.iter().any(|t| t == *banned) {
                return AgentBackendResult {
                    ok: false,
                    output: json!({"error": "policy_denied", "tool": banned}),
                    error: Some(format!(
                        "worker must not receive orchestration tool {banned}"
                    )),
                };
            }
        }
        self.inner.run_agent(request).await
    }
}

/// Built-in workflow tool.
pub struct WorkflowTool {
    policy: SecurityPolicy,
    config: WorkflowConfig,
    backend: Arc<dyn AgentBackend>,
}

impl WorkflowTool {
    /// Default constructor: [`WorkerProbeBackend`] (real SecurityPolicy tool
    /// filtering, no live model). Production runtimes should prefer
    /// [`Self::with_subagent_bridge`] once a `ToolExecutor` weak handle exists.
    pub fn new(policy: SecurityPolicy, config: WorkflowConfig) -> Self {
        let backend = Arc::new(WorkerProbeBackend::new(policy.clone()));
        Self {
            policy,
            config,
            backend,
        }
    }

    /// Production constructor: each `agent()` runs a real nested `subagent` turn.
    pub fn with_subagent_bridge(
        policy: SecurityPolicy,
        config: WorkflowConfig,
        tool_executor: std::sync::Weak<crate::tool::ToolExecutor>,
    ) -> Self {
        Self {
            policy,
            config,
            backend: Arc::new(SubagentBridgeBackend::new(tool_executor)),
        }
    }

    pub fn with_backend(
        policy: SecurityPolicy,
        config: WorkflowConfig,
        backend: Arc<dyn AgentBackend>,
    ) -> Self {
        Self {
            policy,
            config,
            backend,
        }
    }

    pub fn with_mock(
        policy: SecurityPolicy,
        config: WorkflowConfig,
        mock: MockAgentBackend,
    ) -> Self {
        Self {
            policy,
            config,
            backend: Arc::new(mock),
        }
    }

    /// Integration tests: SecurityPolicy-probing backend with optional delay.
    pub fn with_probe(
        policy: SecurityPolicy,
        config: WorkflowConfig,
        probe: WorkerProbeBackend,
    ) -> Self {
        Self {
            policy,
            config,
            backend: Arc::new(probe),
        }
    }
}

pub(crate) struct AgentJob {
    pub request: AgentRequest,
    pub response: std::sync::mpsc::Sender<AgentBackendResult>,
}

pub(crate) struct WorkflowHostError {
    pub code: WorkflowErrorCode,
    pub message: String,
    pub hint: Option<String>,
}

const TOOL_DESCRIPTION: &str = "\
Run a multi-agent workflow authored as a sandboxed Lua 5.4 script. \
The script only orchestrates workers; workers perform all filesystem/shell IO.

Entrypoint (primary): define `function workflow() ... end` and return a value.

Host builtins (only these — no require/io/os/debug/JSON.parse):
  agent(prompt, opts?)  — spawn one worker; blocks until done; returns a table
  parallel(thunks)      — array of zero-arg functions only; order-preserving results
  pipeline(items, fn)   — map each ipairs item through fn (may parallelize)
  phase(title)          — progress boundary
  log(message)          — progress log
  args                  — read-only tool args table
  budget                — {total, spent, remaining}

Default run policy is read-only (explorer): tools like read_file+search, \
create_files/dirs=false, write_allow={}. Grant writes only via write_allow paths \
(intersected with run policy). Empty write_allow ⇒ no writes even for implementer.

Caps: default max_parallel=16, max_agents=1000 (clamped ceilings 64 / 5000).
Workers never get nested `subagent` or `workflow` tools.

Example:
  function workflow()
    phase(\"scan\")
    local hits = pipeline(args.files or {}, function(f)
      return agent(\"Audit \" .. f, {label = f})
    end)
    return { count = #hits, hits = hits }
  end

Do NOT use require, io, os, package, loadfile, or JSON.parse. \
Agent results are already Lua tables. \
Do NOT invent host APIs beyond agent/parallel/pipeline/phase/log/args/budget.";

#[async_trait]
impl Tool for WorkflowTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "workflow",
            TOOL_DESCRIPTION,
            ToolKind::Command,
            json!({
                "type": "object",
                "properties": {
                    "script": {
                        "type": "string",
                        "description": "Non-empty Lua 5.4 source. Must define function workflow() or return a value from the chunk."
                    },
                    "args": {
                        "type": "object",
                        "description": "JSON object injected as read-only Lua global `args`."
                    },
                    "max_parallel": {
                        "type": "integer",
                        "description": "Max concurrent workers (default 16, ceiling 64)."
                    },
                    "max_agents": {
                        "type": "integer",
                        "description": "Max agents per run (default 1000, ceiling 5000)."
                    },
                    "name": {
                        "type": "string",
                        "description": "Optional label for UI / journal."
                    },
                    "policy": {
                        "type": "object",
                        "description": "Run-level default agent policy.",
                        "properties": {
                            "profile": { "type": "string" },
                            "tools": { "type": "array", "items": { "type": "string" } },
                            "path_allow": { "type": "array", "items": { "type": "string" } },
                            "path_deny": { "type": "array", "items": { "type": "string" } },
                            "create_files": { "type": "boolean" },
                            "create_dirs": { "type": "boolean" },
                            "write_allow": { "type": "array", "items": { "type": "string" } },
                            "approval": { "type": "string" }
                        }
                    },
                    "resume_from_run_id": {
                        "type": "string",
                        "description": "Resume is not implemented in v1."
                    }
                },
                "required": ["script"],
                "additionalProperties": false
            }),
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
        let started = Instant::now();
        let invocation_id = invocation.id.clone();

        if !self.config.enabled {
            return Ok(fail(
                &invocation_id,
                None,
                WorkflowRunStatus::Failed,
                WorkflowErrorCode::PolicyDenied,
                "workflow tool is disabled in config",
                Some("Set [workflow] enabled = true."),
                WorkflowStats::default(),
                None,
            ));
        }

        if helpers::optional_string(&invocation.input, "resume_from_run_id").is_some() {
            return Ok(fail(
                &invocation_id,
                None,
                WorkflowRunStatus::Failed,
                WorkflowErrorCode::NotImplemented,
                "resume_from_run_id is not implemented in v1",
                None,
                WorkflowStats::default(),
                None,
            ));
        }

        let script = match helpers::required_string(&invocation.input, "script") {
            Ok(s) if !s.trim().is_empty() => s.to_string(),
            Ok(_) => {
                return Ok(fail(
                    &invocation_id,
                    None,
                    WorkflowRunStatus::Failed,
                    WorkflowErrorCode::InvalidHostCall,
                    "script must be non-empty",
                    None,
                    WorkflowStats::default(),
                    None,
                ));
            }
            Err(err) => {
                return Ok(fail(
                    &invocation_id,
                    None,
                    WorkflowRunStatus::Failed,
                    WorkflowErrorCode::InvalidHostCall,
                    &format!("missing or invalid script: {err}"),
                    Some("Provide a non-empty Lua script string."),
                    WorkflowStats::default(),
                    None,
                ));
            }
        };

        let max_script = if self.config.max_script_bytes == 0 {
            DEFAULT_MAX_SCRIPT_BYTES
        } else {
            self.config.max_script_bytes
        };
        if script.len() > max_script {
            return Ok(fail(
                &invocation_id,
                None,
                WorkflowRunStatus::Failed,
                WorkflowErrorCode::ScriptTooLarge,
                &format!("script is {} bytes; max is {max_script}", script.len()),
                Some("Shorten the Lua script."),
                WorkflowStats::default(),
                None,
            ));
        }

        let max_parallel = clamp_max_parallel(
            optional_usize(&invocation.input, "max_parallel")
                .unwrap_or(self.config.max_parallel.max(1)),
        );
        let max_agents = clamp_max_agents(
            optional_usize(&invocation.input, "max_agents")
                .unwrap_or(self.config.max_agents.max(1)),
        );
        let timeout_ms = self.config.run_timeout_ms;
        let name = helpers::optional_string(&invocation.input, "name");
        let args = invocation
            .input
            .get("args")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let run_policy = parse_run_policy(invocation.input.get("policy"));

        let run_id = new_run_id();
        let journal_dir = self.policy.data_dir().join("workflows").join(&run_id);
        let mut journal = match WorkflowJournal::create(&journal_dir, &run_id, name.as_deref()) {
            Ok(j) => j,
            Err(err) => {
                return Ok(fail(
                    &invocation_id,
                    Some(run_id),
                    WorkflowRunStatus::Failed,
                    WorkflowErrorCode::ScriptRuntimeError,
                    &format!("failed to create journal: {err}"),
                    None,
                    WorkflowStats::default(),
                    None,
                ));
            }
        };
        let _ = journal.write_meta_start(
            &script,
            &args,
            max_parallel,
            max_agents,
            self.policy.project_root(),
        );

        let cancel_token = context.cancel_token.clone().unwrap_or_default();
        let semaphore = Arc::new(Semaphore::new(max_parallel.max(1)));
        let backend: Arc<dyn AgentBackend> = Arc::new(PolicyAgentBackend {
            inner: self.backend.clone(),
        });

        let (job_tx, mut job_rx) = tokio::sync::mpsc::unbounded_channel::<AgentJob>();
        let (lua_done_tx, lua_done_rx) =
            tokio::sync::oneshot::channel::<Result<runtime::LuaRunOutcome, WorkflowHostError>>();

        let journal_path = journal.journal_path().to_path_buf();
        let stats = Arc::new(std::sync::Mutex::new(WorkflowStats::default()));
        let in_flight = Arc::new(AtomicUsize::new(0));

        let lua_input = LuaRunInput {
            script,
            args,
            run_policy,
            max_agents,
            job_tx,
            cancel_token: cancel_token.clone(),
        };

        let _lua_thread = std::thread::Builder::new()
            .name("navi-workflow-lua".into())
            .spawn(move || {
                let outcome = run_lua_workflow(lua_input);
                let _ = lua_done_tx.send(outcome);
            })
            .map_err(|e| anyhow::anyhow!("spawn lua thread: {e}"))?;

        let stats_j = stats.clone();
        let cancel_j = cancel_token.clone();
        let in_flight_j = in_flight.clone();
        let job_loop_handle = tokio::spawn(async move {
            let mut handles = Vec::new();
            while let Some(job) = job_rx.recv().await {
                if cancel_j.is_requested() {
                    let _ = job.response.send(AgentBackendResult {
                        ok: false,
                        output: json!({"error": "cancelled"}),
                        error: Some("cancelled".into()),
                    });
                    continue;
                }
                let permit = match semaphore.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => {
                        let _ = job.response.send(AgentBackendResult {
                            ok: false,
                            output: json!({"error": "semaphore closed"}),
                            error: Some("semaphore closed".into()),
                        });
                        continue;
                    }
                };
                let backend = backend.clone();
                let stats_j = stats_j.clone();
                let journal_path = journal_path.clone();
                let cancel_j = cancel_j.clone();
                let in_flight_j = in_flight_j.clone();
                handles.push(tokio::spawn(async move {
                    let n = in_flight_j.fetch_add(1, Ordering::SeqCst) + 1;
                    {
                        let mut s = stats_j.lock().unwrap_or_else(|e| e.into_inner());
                        s.agents_started += 1;
                        s.max_parallel_used = s.max_parallel_used.max(n);
                    }
                    let agent_index = job.request.agent_index;
                    let label = job.request.label.clone();
                    let prompt = job.request.prompt.clone();
                    append_journal_line(
                        &journal_path,
                        &json!({
                            "event": "agent_started",
                            "agent_index": agent_index,
                            "label": label,
                            "prompt": redact_secrets(&prompt),
                        }),
                    );
                    let mut req = job.request;
                    req.cancel_token = cancel_j;
                    let mut result = backend.run_agent(req).await;
                    result.output = truncate_json(result.output, AGENT_RESULT_MAX_BYTES);
                    {
                        let mut s = stats_j.lock().unwrap_or_else(|e| e.into_inner());
                        if result.ok {
                            s.agents_completed += 1;
                        } else {
                            s.agents_failed += 1;
                        }
                    }
                    append_journal_line(
                        &journal_path,
                        &json!({
                            "event": "agent_completed",
                            "agent_index": agent_index,
                            "ok": result.ok,
                        }),
                    );
                    in_flight_j.fetch_sub(1, Ordering::SeqCst);
                    let _ = job.response.send(result);
                    drop(permit);
                }));
            }
            for h in handles {
                let _ = h.await;
            }
        });

        enum WaitKind {
            Cancelled,
            TimedOut,
            Lua(
                Result<
                    Result<runtime::LuaRunOutcome, WorkflowHostError>,
                    tokio::sync::oneshot::error::RecvError,
                >,
            ),
        }
        let wait = if timeout_ms > 0 {
            let timeout = std::time::Duration::from_millis(timeout_ms);
            tokio::select! {
                biased;
                _ = cancel_token.notified() => WaitKind::Cancelled,
                _ = tokio::time::sleep(timeout) => WaitKind::TimedOut,
                outcome = lua_done_rx => WaitKind::Lua(outcome),
            }
        } else {
            tokio::select! {
                biased;
                _ = cancel_token.notified() => WaitKind::Cancelled,
                outcome = lua_done_rx => WaitKind::Lua(outcome),
            }
        };
        let finish = match wait {
            WaitKind::Cancelled => {
                cancel_token.cancel();
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(5), job_loop_handle).await;
                Finish::Cancelled
            }
            WaitKind::TimedOut => {
                cancel_token.cancel();
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(5), job_loop_handle).await;
                Finish::TimedOut
            }
            WaitKind::Lua(outcome) => {
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(30), job_loop_handle).await;
                match outcome {
                    Ok(Ok(o)) => Finish::Lua(o),
                    Ok(Err(e)) => Finish::Err(e),
                    Err(_) => Finish::Err(WorkflowHostError {
                        code: WorkflowErrorCode::ScriptRuntimeError,
                        message: "workflow Lua task dropped".into(),
                        hint: None,
                    }),
                }
            }
        };

        let mut final_stats = stats.lock().unwrap_or_else(|e| e.into_inner()).clone();
        final_stats.elapsed_ms = started.elapsed().as_millis() as u64;
        let journal_path_str = journal.journal_path().display().to_string();

        let tool_result = match finish {
            Finish::Cancelled => {
                final_stats.phases = journal.take_phases();
                let _ = journal.finalize(&run_id, WorkflowRunStatus::Cancelled, &final_stats, None);
                fail(
                    &invocation_id,
                    Some(run_id),
                    WorkflowRunStatus::Cancelled,
                    WorkflowErrorCode::Cancelled,
                    "workflow cancelled",
                    None,
                    final_stats,
                    Some(journal_path_str),
                )
            }
            Finish::TimedOut => {
                final_stats.phases = journal.take_phases();
                let _ = journal.finalize(&run_id, WorkflowRunStatus::TimedOut, &final_stats, None);
                fail(
                    &invocation_id,
                    Some(run_id),
                    WorkflowRunStatus::TimedOut,
                    WorkflowErrorCode::Timeout,
                    "workflow timed out",
                    None,
                    final_stats,
                    Some(journal_path_str),
                )
            }
            Finish::Err(e) => {
                final_stats.phases = journal.take_phases();
                let status = status_for_code(e.code);
                let _ = journal.finalize(&run_id, status, &final_stats, Some(&e.message));
                fail(
                    &invocation_id,
                    Some(run_id),
                    status,
                    e.code,
                    &e.message,
                    e.hint.as_deref(),
                    final_stats,
                    Some(journal_path_str),
                )
            }
            Finish::Lua(outcome) => {
                for p in &outcome.phases {
                    journal.record_phase(p);
                }
                for line in &outcome.logs {
                    journal.record_log(line);
                }
                final_stats.phases = outcome.phases.clone();
                if outcome.agents_started > final_stats.agents_started {
                    final_stats.agents_started = outcome.agents_started;
                }
                final_stats.elapsed_ms = started.elapsed().as_millis() as u64;

                if let Some(err) = outcome.error {
                    let status = status_for_code(err.code);
                    let _ = journal.finalize(&run_id, status, &final_stats, Some(&err.message));
                    fail(
                        &invocation_id,
                        Some(run_id),
                        status,
                        err.code,
                        &err.message,
                        err.hint.as_deref(),
                        final_stats,
                        Some(journal_path_str),
                    )
                } else {
                    let _ =
                        journal.finalize(&run_id, WorkflowRunStatus::Completed, &final_stats, None);
                    let compact = truncate_json(outcome.result, 32 * 1024);
                    ToolResult {
                        invocation_id,
                        ok: true,
                        output: json!({
                            "ok": true,
                            "run_id": run_id,
                            "status": WorkflowRunStatus::Completed,
                            "result": compact,
                            "stats": final_stats,
                            "journal_path": journal_path_str,
                            "error": null,
                            "name": name,
                        }),
                    }
                }
            }
        };

        Ok(tool_result)
    }
}

enum Finish {
    Lua(runtime::LuaRunOutcome),
    Err(WorkflowHostError),
    Cancelled,
    TimedOut,
}

fn status_for_code(code: WorkflowErrorCode) -> WorkflowRunStatus {
    match code {
        WorkflowErrorCode::Cancelled => WorkflowRunStatus::Cancelled,
        WorkflowErrorCode::Timeout => WorkflowRunStatus::TimedOut,
        _ => WorkflowRunStatus::Failed,
    }
}

fn new_run_id() -> String {
    let n = RUN_COUNTER.fetch_add(1, Ordering::SeqCst);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("wf_{millis}_{n}")
}

fn parse_run_policy(value: Option<&Value>) -> RunPolicy {
    let mut policy = default_run_policy();
    let Some(obj) = value.and_then(|v| v.as_object()) else {
        return policy;
    };
    if let Some(p) = obj.get("profile").and_then(|v| v.as_str()) {
        policy.profile = p.to_string();
    }
    if let Some(tools) = obj.get("tools").and_then(|v| v.as_array()) {
        policy.tools = tools
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
    }
    if let Some(v) = obj.get("path_allow").and_then(|v| v.as_array()) {
        policy.path_allow = v
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect();
    }
    if let Some(v) = obj.get("path_deny").and_then(|v| v.as_array()) {
        policy.path_deny = v
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect();
    }
    if let Some(b) = obj.get("create_files").and_then(|v| v.as_bool()) {
        policy.create_files = b;
    }
    if let Some(b) = obj.get("create_dirs").and_then(|v| v.as_bool()) {
        policy.create_dirs = b;
    }
    if let Some(v) = obj.get("write_allow").and_then(|v| v.as_array()) {
        policy.write_allow = v
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect();
    }
    if let Some(a) = obj.get("approval").and_then(|v| v.as_str()) {
        policy.approval = a.to_string();
    }
    policy
}

fn fail(
    invocation_id: &str,
    run_id: Option<String>,
    status: WorkflowRunStatus,
    code: WorkflowErrorCode,
    message: &str,
    hint: Option<&str>,
    stats: WorkflowStats,
    journal_path: Option<String>,
) -> ToolResult {
    ToolResult {
        invocation_id: invocation_id.to_string(),
        ok: false,
        output: json!({
            "ok": false,
            "run_id": run_id,
            "status": status,
            "result": null,
            "stats": stats,
            "journal_path": journal_path,
            "error": {
                "code": code,
                "message": message,
                "hint": hint,
            },
            "error_code": code,
            "message": message,
        }),
    }
}

fn append_journal_line(path: &std::path::Path, value: &Value) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        && let Ok(line) = serde_json::to_string(value)
    {
        let _ = writeln!(f, "{line}");
    }
}

fn truncate_json(value: Value, max_bytes: usize) -> Value {
    let Ok(s) = serde_json::to_string(&value) else {
        return value;
    };
    if s.len() <= max_bytes {
        return value;
    }
    json!({
        "truncated": true,
        "original_bytes": s.len(),
        "preview": redact_secrets(&s.chars().take(max_bytes.min(4096)).collect::<String>()),
    })
}

fn optional_usize(input: &Value, key: &str) -> Option<usize> {
    input.get(key).and_then(|v| {
        v.as_u64()
            .map(|n| n as usize)
            .or_else(|| v.as_i64().map(|n| n.max(0) as usize))
    })
}

/// Description text for snapshot tests (§12).
pub fn workflow_tool_description() -> &'static str {
    TOOL_DESCRIPTION
}
