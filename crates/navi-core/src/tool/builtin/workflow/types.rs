//! Shared types for the workflow tool.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::policy::{EffectiveAgentPolicy, RunPolicy};
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
    pub run_policy: RunPolicy,
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
            .field("run_policy", &self.run_policy)
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

#[cfg(test)]
mod tests {
    use super::super::policy::default_run_policy;
    use super::*;

    #[test]
    fn workflow_run_status_serde_roundtrip() {
        for status in [
            WorkflowRunStatus::Completed,
            WorkflowRunStatus::Failed,
            WorkflowRunStatus::Cancelled,
            WorkflowRunStatus::TimedOut,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let back: WorkflowRunStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, back);
        }
    }

    #[test]
    fn workflow_run_status_snake_case_serialization() {
        assert_eq!(
            serde_json::to_string(&WorkflowRunStatus::Completed).unwrap(),
            "\"completed\""
        );
        assert_eq!(
            serde_json::to_string(&WorkflowRunStatus::Failed).unwrap(),
            "\"failed\""
        );
        assert_eq!(
            serde_json::to_string(&WorkflowRunStatus::Cancelled).unwrap(),
            "\"cancelled\""
        );
        assert_eq!(
            serde_json::to_string(&WorkflowRunStatus::TimedOut).unwrap(),
            "\"timed_out\""
        );
    }

    #[test]
    fn workflow_run_status_deserialize_from_snake_case() {
        let s: WorkflowRunStatus = serde_json::from_str("\"completed\"").unwrap();
        assert_eq!(s, WorkflowRunStatus::Completed);
        let s: WorkflowRunStatus = serde_json::from_str("\"timed_out\"").unwrap();
        assert_eq!(s, WorkflowRunStatus::TimedOut);
    }

    #[test]
    fn workflow_error_code_serde_roundtrip() {
        for code in [
            WorkflowErrorCode::ScriptTooLarge,
            WorkflowErrorCode::ScriptParseError,
            WorkflowErrorCode::ScriptRuntimeError,
            WorkflowErrorCode::SandboxViolation,
            WorkflowErrorCode::InvalidHostCall,
            WorkflowErrorCode::AgentCapExceeded,
            WorkflowErrorCode::BudgetExceeded,
            WorkflowErrorCode::Timeout,
            WorkflowErrorCode::Cancelled,
            WorkflowErrorCode::PolicyDenied,
            WorkflowErrorCode::NotImplemented,
        ] {
            let json = serde_json::to_string(&code).unwrap();
            let back: WorkflowErrorCode = serde_json::from_str(&json).unwrap();
            assert_eq!(code, back);
        }
    }

    #[test]
    fn workflow_error_code_snake_case_serialization() {
        assert_eq!(
            serde_json::to_string(&WorkflowErrorCode::ScriptTooLarge).unwrap(),
            "\"script_too_large\""
        );
        assert_eq!(
            serde_json::to_string(&WorkflowErrorCode::AgentCapExceeded).unwrap(),
            "\"agent_cap_exceeded\""
        );
        assert_eq!(
            serde_json::to_string(&WorkflowErrorCode::NotImplemented).unwrap(),
            "\"not_implemented\""
        );
    }

    #[test]
    fn workflow_stats_default() {
        let stats = WorkflowStats::default();
        assert_eq!(stats.agents_started, 0);
        assert_eq!(stats.agents_completed, 0);
        assert_eq!(stats.agents_failed, 0);
        assert_eq!(stats.agents_cached, 0);
        assert_eq!(stats.max_parallel_used, 0);
        assert!(stats.phases.is_empty());
        assert_eq!(stats.elapsed_ms, 0);
        assert!(stats.tokens_estimate.is_none());
    }

    #[test]
    fn workflow_stats_serde_roundtrip() {
        let stats = WorkflowStats {
            agents_started: 5,
            agents_completed: 3,
            agents_failed: 1,
            agents_cached: 1,
            max_parallel_used: 2,
            phases: vec!["phase1".into(), "phase2".into()],
            elapsed_ms: 12345,
            tokens_estimate: Some(50000),
        };
        let json = serde_json::to_string(&stats).unwrap();
        let back: WorkflowStats = serde_json::from_str(&json).unwrap();
        assert_eq!(back.agents_started, 5);
        assert_eq!(back.agents_completed, 3);
        assert_eq!(back.agents_failed, 1);
        assert_eq!(back.agents_cached, 1);
        assert_eq!(back.max_parallel_used, 2);
        assert_eq!(back.phases, vec!["phase1", "phase2"]);
        assert_eq!(back.elapsed_ms, 12345);
        assert_eq!(back.tokens_estimate, Some(50000));
    }

    #[test]
    fn workflow_stats_serde_default_tokens_estimate() {
        let json = r#"{"agents_started":0,"agents_completed":0,"agents_failed":0,"agents_cached":0,"max_parallel_used":0,"phases":[],"elapsed_ms":0}"#;
        let stats: WorkflowStats = serde_json::from_str(json).unwrap();
        assert!(stats.tokens_estimate.is_none());
    }

    #[test]
    fn agent_request_debug_excludes_cancel_token_internals() {
        let req = AgentRequest {
            agent_index: 42,
            prompt: "do work".into(),
            label: Some("worker-1".into()),
            model: Some("gpt-4".into()),
            run_policy: default_run_policy(),
            effective: EffectiveAgentPolicy {
                profile: "explorer".into(),
                path_allow: vec!["**".into()],
                path_deny: vec![],
            },
            cancel_token: CancelToken::new(),
        };
        let debug = format!("{req:?}");
        assert!(debug.contains("AgentRequest"));
        assert!(debug.contains("42"));
        assert!(debug.contains("do work"));
        assert!(debug.contains("worker-1"));
        assert!(debug.contains("gpt-4"));
        // CancelToken should be shown as "<CancelToken>" not its internals.
        assert!(debug.contains("<CancelToken>"));
    }

    #[test]
    fn agent_backend_result_debug() {
        let result = AgentBackendResult {
            ok: true,
            output: serde_json::json!({"key": "value"}),
            error: None,
        };
        let debug = format!("{result:?}");
        assert!(debug.contains("AgentBackendResult"));
        assert!(debug.contains("true"));
    }

    #[test]
    fn agent_backend_result_clone() {
        let result = AgentBackendResult {
            ok: true,
            output: serde_json::json!({"key": "value"}),
            error: Some("err".into()),
        };
        let cloned = result.clone();
        assert_eq!(result.ok, cloned.ok);
        assert_eq!(result.error, cloned.error);
    }

    #[test]
    fn constants_have_expected_values() {
        assert_eq!(DEFAULT_MAX_SCRIPT_BYTES, 64 * 1024);
        assert_eq!(AGENT_RESULT_MAX_BYTES, 256 * 1024);
        assert_eq!(NESTED_WORKFLOW_TOOLS, &["subagent", "workflow"]);
    }
}
