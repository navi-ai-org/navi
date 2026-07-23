use crate::trace::{TraceStore, TurnOutcome, TurnTrace};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Kind of failure a turn experienced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// The provider stream succeeded but produced no assistant content.
    EmptyResponse,
    /// A provider-level transport or API error occurred.
    ProviderError,
    /// One or more tool calls failed during the turn.
    ToolFailure,
    /// The harness stopped the turn for policy reasons.
    HarnessStopped,
    /// Failure kind could not be classified.
    Unknown,
}

impl FailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmptyResponse => "empty_response",
            Self::ProviderError => "provider_error",
            Self::ToolFailure => "tool_failure",
            Self::HarnessStopped => "harness_stopped",
            Self::Unknown => "unknown",
        }
    }
}

/// Action the runtime can take to recover from a failed turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairAction {
    /// No automatic recovery is available.
    None,
    /// Retry the model request with reasoning/thinking disabled.
    RetryWithoutThinking,
    /// Retry with a smaller visible tool set.
    ReduceToolSet,
    /// Switch to a different provider/model.
    SwitchModel { provider: String, model: String },
    /// Surface a clear message to the user.
    ReportToUser { message: String },
}

/// Diagnosis produced for a single turn trace.
#[derive(Debug, Clone)]
pub struct Diagnosis {
    pub failure_kind: Option<FailureKind>,
    pub repair_action: RepairAction,
    pub summary: String,
}

/// Local, privacy-first diagnostician that recommends recovery actions by
/// inspecting the current turn and recent local traces. No network calls.
pub struct TurnDiagnostician;

impl TurnDiagnostician {
    pub fn diagnose(
        turn: &TurnTrace,
        _recent: &[TurnTrace],
        reliability: &ReliabilityIndex,
    ) -> Diagnosis {
        let failure_kind = turn.failure_kind;
        let repair_action = Self::repair_action_for(turn, failure_kind);
        let summary = Self::build_summary(turn, failure_kind, &repair_action, reliability);
        Diagnosis {
            failure_kind,
            repair_action,
            summary,
        }
    }

    fn repair_action_for(turn: &TurnTrace, failure_kind: Option<FailureKind>) -> RepairAction {
        match failure_kind {
            Some(FailureKind::EmptyResponse) => {
                if turn.recovery_attempts == 0 {
                    RepairAction::RetryWithoutThinking
                } else {
                    RepairAction::ReportToUser {
                        message: format!(
                            "`{}` via `{}` returned empty content after {} recovery attempt(s). Try again, turn thinking off, or switch models.",
                            turn.model_name, turn.model_provider, turn.recovery_attempts
                        ),
                    }
                }
            }
            Some(FailureKind::ToolFailure) => {
                if turn.metrics.failed_tool_calls > 0 && turn.visible_tool_count > 3 {
                    RepairAction::ReduceToolSet
                } else {
                    RepairAction::ReportToUser {
                        message: "One or more tools failed during this turn. Review the tool output and try again.".to_string(),
                    }
                }
            }
            Some(FailureKind::ProviderError) => RepairAction::ReportToUser {
                message: "A provider error occurred. Check your connection, API key, or switch models.".to_string(),
            },
            Some(FailureKind::HarnessStopped) => RepairAction::ReportToUser {
                message: "The harness stopped this turn to prevent an infinite loop. Try a smaller or more specific request.".to_string(),
            },
            Some(FailureKind::Unknown) | None => RepairAction::None,
        }
    }

    fn build_summary(
        turn: &TurnTrace,
        failure_kind: Option<FailureKind>,
        _action: &RepairAction,
        reliability: &ReliabilityIndex,
    ) -> String {
        match failure_kind {
            Some(FailureKind::EmptyResponse) => {
                let score = reliability.score(&turn.model_provider, &turn.model_name);
                let reliability_note = score
                    .map(|s| format!("; historical reliability {:.0}%", s * 100.0))
                    .unwrap_or_default();
                format!(
                    "empty response from {}/{} after {}ms; {} recovery attempt(s){}",
                    turn.model_provider,
                    turn.model_name,
                    turn.metrics.wall_time_ms,
                    turn.recovery_attempts,
                    reliability_note
                )
            }
            Some(FailureKind::ToolFailure) => format!(
                "tool failure: {} of {} tool calls failed from {}/{}",
                turn.metrics.failed_tool_calls,
                turn.metrics.tool_call_count,
                turn.model_provider,
                turn.model_name
            ),
            Some(FailureKind::ProviderError) => {
                let detail = match &turn.outcome {
                    TurnOutcome::Failed(msg) => format!(": {msg}"),
                    _ => String::new(),
                };
                format!(
                    "provider error from {}/{}{}",
                    turn.model_provider, turn.model_name, detail
                )
            }
            Some(FailureKind::HarnessStopped) => {
                let detail = match &turn.outcome {
                    TurnOutcome::Stopped(reason) => format!(" ({reason})"),
                    _ => String::new(),
                };
                format!(
                    "harness stopped from {}/{}{}",
                    turn.model_provider, turn.model_name, detail
                )
            }
            Some(FailureKind::Unknown) => "unclassified failure".to_string(),
            None => format!(
                "{} outcome from {}/{} in {}ms; {} tool calls ({} failed)",
                match turn.outcome {
                    TurnOutcome::Success => "success",
                    TurnOutcome::PartialSuccess => "partial_success",
                    _ => "completed",
                },
                turn.model_provider,
                turn.model_name,
                turn.metrics.wall_time_ms,
                turn.metrics.tool_call_count,
                turn.metrics.failed_tool_calls
            ),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct ModelReliability {
    total: u64,
    failures: u64,
}

/// In-memory per-provider/model success-rate index built from local traces.
/// Scans `data_dir/traces/` once; no network access.
#[derive(Debug, Clone, Default)]
pub struct ReliabilityIndex {
    scores: HashMap<(String, String), ModelReliability>,
}

impl ReliabilityIndex {
    pub fn load(data_dir: &Path) -> Self {
        let mut index = Self::default();
        let store = TraceStore::new(data_dir);
        let Ok(entries) = std::fs::read_dir(store.root()) else {
            return index;
        };

        const MAX_LINES: usize = 100_000;
        let mut lines_read = 0usize;
        for entry in entries.flatten() {
            if lines_read >= MAX_LINES {
                break;
            }
            let path = entry.path();
            if path.extension().map_or(true, |ext| ext != "jsonl") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in content.lines() {
                if lines_read >= MAX_LINES {
                    break;
                }
                lines_read += 1;
                if let Ok(trace) = serde_json::from_str::<TurnTrace>(line) {
                    let key = (trace.model_provider.clone(), trace.model_name.clone());
                    let rel = index.scores.entry(key).or_default();
                    rel.total += 1;
                    if !matches!(trace.outcome, TurnOutcome::Success) {
                        rel.failures += 1;
                    }
                }
            }
        }
        index
    }

    pub fn score(&self, provider: &str, model: &str) -> Option<f64> {
        self.scores
            .get(&(provider.to_string(), model.to_string()))
            .and_then(|r| {
                if r.total == 0 {
                    None
                } else {
                    Some((r.total.saturating_sub(r.failures)) as f64 / r.total as f64)
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::TurnTrace;

    #[test]
    fn empty_response_suggests_retry_without_thinking() {
        let mut trace = TurnTrace::new("t1", "s1", "charm-hyper", "deepseek-v4-flash", "task");
        trace.failure_kind = Some(FailureKind::EmptyResponse);
        trace.finalize();

        let index = ReliabilityIndex::default();
        let diag = TurnDiagnostician::diagnose(&trace, &[], &index);
        assert_eq!(diag.failure_kind, Some(FailureKind::EmptyResponse));
        assert_eq!(diag.repair_action, RepairAction::RetryWithoutThinking);
        assert!(diag.summary.contains("empty response"));
    }

    #[test]
    fn repeated_empty_response_reports_to_user() {
        let mut trace = TurnTrace::new("t1", "s1", "charm-hyper", "deepseek-v4-flash", "task");
        trace.failure_kind = Some(FailureKind::EmptyResponse);
        trace.recovery_attempts = 1;
        trace.finalize();

        let index = ReliabilityIndex::default();
        let diag = TurnDiagnostician::diagnose(&trace, &[], &index);
        assert!(matches!(
            diag.repair_action,
            RepairAction::ReportToUser { .. }
        ));
    }

    #[test]
    fn reliability_index_counts_failures() {
        let dir = tempfile::tempdir().unwrap();
        let store = TraceStore::new(dir.path());

        let mut ok = TurnTrace::new("ok", "s-ok", "p1", "m1", "ok");
        ok.finalize();
        store.save_trace(&ok).unwrap();

        let mut fail = TurnTrace::new("fail", "s-fail", "p1", "m1", "fail");
        fail.outcome = TurnOutcome::Failed("boom".to_string());
        fail.finalize();
        store.save_trace(&fail).unwrap();

        let index = ReliabilityIndex::load(dir.path());
        let score = index.score("p1", "m1").unwrap();
        assert!((score - 0.5).abs() < 0.01);
    }
}
