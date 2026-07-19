//! Trace-to-dataset export for evals, routing, and permission tuning.

use crate::eval::{EvalCase, eval_case_from_trace};
use crate::security::redact_secrets;
use crate::trace::{TurnOutcome, TurnTrace};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetRow {
    pub version: u32,
    pub row_type: DatasetRowType,
    pub task: String,
    pub outcome: String,
    pub tools: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub mcp_tainted: bool,
    pub verifier_passed: bool,
    pub reward: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetRowType {
    PreferencePair,
    NegativeExample,
    ToolRouterTraining,
    PermissionClassifier,
    VerifierReward,
}

pub fn trace_to_dataset_rows(trace: &TurnTrace) -> Vec<DatasetRow> {
    let tools = trace
        .tool_calls
        .iter()
        .map(|call| call.invocation.tool_name.clone())
        .collect::<Vec<_>>();
    let verifier_passed = trace
        .verifier_results
        .iter()
        .any(|verifier| verifier.passed);
    let success = matches!(
        trace.outcome,
        TurnOutcome::Success | TurnOutcome::PartialSuccess
    );
    let task = redact_secrets(&trace.task);
    let capabilities = trace
        .capabilities
        .iter()
        .map(|entry| entry.capability.as_key())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mcp_tainted = trace.tool_calls.iter().any(|call| {
        call.result
            .output
            .get("tainted")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
            || call
                .result
                .output
                .get("provenance")
                .and_then(|value| value.get("source"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|source| source == "mcp")
    });
    let mut rows = vec![
        DatasetRow {
            version: 1,
            row_type: if success {
                DatasetRowType::ToolRouterTraining
            } else {
                DatasetRowType::NegativeExample
            },
            task: task.clone(),
            outcome: outcome_label(&trace.outcome),
            tools: tools.clone(),
            capabilities: capabilities.clone(),
            mcp_tainted,
            verifier_passed,
            reward: if success && verifier_passed { 1.0 } else { 0.0 },
        },
        DatasetRow {
            version: 1,
            row_type: DatasetRowType::VerifierReward,
            task,
            outcome: outcome_label(&trace.outcome),
            tools,
            capabilities: capabilities.clone(),
            mcp_tainted,
            verifier_passed,
            reward: if verifier_passed { 1.0 } else { -1.0 },
        },
    ];
    if !capabilities.is_empty() {
        rows.push(DatasetRow {
            version: 1,
            row_type: DatasetRowType::PermissionClassifier,
            task: redact_secrets(&trace.task),
            outcome: outcome_label(&trace.outcome),
            tools: trace
                .tool_calls
                .iter()
                .map(|call| call.invocation.tool_name.clone())
                .collect(),
            capabilities,
            mcp_tainted,
            verifier_passed,
            reward: if success { 1.0 } else { -1.0 },
        });
    }
    rows
}

pub fn traces_to_eval_candidates(traces: &[TurnTrace]) -> Vec<EvalCase> {
    traces.iter().filter_map(eval_case_from_trace).collect()
}

pub fn export_jsonl(rows: &[DatasetRow]) -> Result<String> {
    let mut lines = Vec::with_capacity(rows.len());
    for row in rows {
        lines.push(serde_json::to_string(row).context("failed to serialize dataset row to JSON")?);
    }
    Ok(lines.join("\n"))
}

fn outcome_label(outcome: &TurnOutcome) -> String {
    match outcome {
        TurnOutcome::Success => "success".to_string(),
        TurnOutcome::PartialSuccess => "partial_success".to_string(),
        TurnOutcome::Stopped(reason) => format!("stopped:{reason}"),
        TurnOutcome::Failed(reason) => format!("failed:{reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dataset_export_redacts_task_secrets() {
        let mut trace = TurnTrace::new("t", "s", "p", "m", "token=sk-12345678901234567890");
        trace.outcome = TurnOutcome::Failed("no".to_string());

        let rows = trace_to_dataset_rows(&trace);
        let jsonl = export_jsonl(&rows).expect("export");

        assert!(!jsonl.contains("sk-1234567890"));
        assert!(jsonl.contains("negative_example"));
    }

    #[test]
    fn dataset_export_includes_permission_classifier_rows() {
        let mut trace = TurnTrace::new("t", "s", "p", "m", "read repo");
        trace.record_capability(crate::capability::CapabilityLedgerEntry {
            capability: crate::capability::Capability::RepoRead,
            scope: crate::capability::CapabilityScope::Turn("t".to_string()),
            decision: crate::capability::CapabilityDecision::Consumed,
            at_ms: 1,
            justification: "read".to_string(),
        });

        let rows = trace_to_dataset_rows(&trace);

        assert!(
            rows.iter()
                .any(|row| row.row_type == DatasetRowType::PermissionClassifier
                    && row.capabilities == vec!["repo.read"])
        );
    }
}
