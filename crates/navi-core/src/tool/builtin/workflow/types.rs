//! Shared types for the workflow tool.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::policy::EffectiveAgentPolicy;
use crate::cancel::CancelToken;

/// Default max Lua script size (64 KiB).
pub const DEFAULT_MAX_SCRIPT_BYTES: usize = 64 * 1024;
/// Truncate agent payloads before injecting into Lua (spec NF4).
pub const AGENT_RESULT_MAX_BYTES: usize = 256 * 1024;

/// Orchestration tools stripped from every worker allowlist.
pub const NESTED_WORKFLOW_TOOLS: &[&str] = &["subagent", "workflow"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunStatus {
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowErrorCode {
    ScriptTooLarge,
    ScriptParseError,
    ScriptRuntimeError,
    SandboxViolation,
    InvalidHostCall,
    AgentCapExceeded,
    BudgetExceeded,
    Timeout,
    Cancelled,
    PolicyDenied,
    NotImplemented,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkflowStats {
    pub agents_started: usize,
    pub agents_completed: usize,
    pub agents_failed: usize,
    pub agents_cached: usize,
    pub max_parallel_used: usize,
    pub phases: Vec<String>,
    pub elapsed_ms: u64,
    pub tokens_estimate: Option<u64>,
}

/// Request passed to the agent backend for one `agent()` call.
#[derive(Clone)]
pub struct AgentRequest {
    pub agent_index: u64,
    pub prompt: String,
    pub label: Option<String>,
    pub model: Option<String>,
    pub max_tokens: Option<usize>,
    pub effective: EffectiveAgentPolicy,
    pub cancel_token: CancelToken,
}

impl std::fmt::Debug for AgentRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRequest")
            .field("agent_index", &self.agent_index)
            .field("prompt", &self.prompt)
            .field("label", &self.label)
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field("effective", &self.effective)
            .field("cancel_token", &"<CancelToken>")
            .finish()
    }
}

/// Result returned by the agent backend.
#[derive(Debug, Clone)]
pub struct AgentBackendResult {
    pub ok: bool,
    pub output: Value,
    pub error: Option<String>,
}
