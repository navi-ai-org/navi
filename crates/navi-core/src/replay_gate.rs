//! Continuous learning replay and superiority gates.

use crate::eval::EvalRun;
use crate::{CapabilityDecision, CapabilityScope, trace::TurnTrace};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayGateConfig {
    pub min_verified_success_rate: f64,
    pub max_success_rate_drop: f64,
    pub require_zero_unsafe_guarded_auto_approvals: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayGateReport {
    pub passed: bool,
    pub failures: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuperiorityGateReport {
    pub passed: bool,
    pub metric: String,
    pub baseline: Option<f64>,
    pub candidate: Option<f64>,
    pub failures: Vec<String>,
}

pub fn evaluate_replay_gate(
    baseline: Option<&EvalRun>,
    candidate: &EvalRun,
    unsafe_guarded_auto_approval_count: u64,
    config: &ReplayGateConfig,
) -> ReplayGateReport {
    let mut failures = Vec::new();
    if candidate.metrics.verified_success_rate < config.min_verified_success_rate {
        failures.push(format!(
            "verified_success_rate {:.3} below minimum {:.3}",
            candidate.metrics.verified_success_rate, config.min_verified_success_rate
        ));
    }
    if let Some(baseline) = baseline {
        let drop = baseline.metrics.verified_success_rate - candidate.metrics.verified_success_rate;
        if drop > config.max_success_rate_drop {
            failures.push(format!(
                "verified_success_rate dropped {:.3}, max allowed {:.3}",
                drop, config.max_success_rate_drop
            ));
        }
    }
    if config.require_zero_unsafe_guarded_auto_approvals && unsafe_guarded_auto_approval_count > 0 {
        failures.push(format!(
            "unsafe guarded auto approvals: {unsafe_guarded_auto_approval_count}"
        ));
    }
    ReplayGateReport {
        passed: failures.is_empty(),
        failures,
    }
}

pub fn unsafe_guarded_auto_approval_count(traces: &[TurnTrace]) -> u64 {
    traces
        .iter()
        .flat_map(|trace| trace.capabilities.iter())
        .filter(|entry| {
            entry.capability.is_guarded()
                && matches!(
                    entry.decision,
                    CapabilityDecision::Granted | CapabilityDecision::Consumed
                )
                && !entry.justification.contains("explicit")
                && !matches!(entry.scope, CapabilityScope::SingleCall(_))
        })
        .count() as u64
}

pub fn evaluate_superiority_gate(baseline: &EvalRun, candidate: &EvalRun) -> SuperiorityGateReport {
    let baseline_metric = baseline.metrics.verified_success_per_1k_tokens;
    let candidate_metric = candidate.metrics.verified_success_per_1k_tokens;
    let mut failures = Vec::new();
    match (baseline_metric, candidate_metric) {
        (Some(base), Some(new)) if new > base => {}
        (Some(base), Some(new)) => failures.push(format!(
            "verified_success_per_1k_tokens did not improve: baseline {base:.3}, candidate {new:.3}"
        )),
        _ => failures
            .push("verified_success_per_1k_tokens missing for baseline or candidate".to_string()),
    }
    SuperiorityGateReport {
        passed: failures.is_empty(),
        metric: "verified_success_per_1k_tokens".to_string(),
        baseline: baseline_metric,
        candidate: candidate_metric,
        failures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::{EvalRun, EvalRunMetrics};
    use std::path::PathBuf;

    fn run(success_rate: f64, per_1k: Option<f64>) -> EvalRun {
        EvalRun {
            version: 1,
            run_id: "r".to_string(),
            suite_name: "s".to_string(),
            started_at: 0,
            ended_at: 1,
            project_root: PathBuf::from("."),
            metrics: EvalRunMetrics {
                total_cases: 1,
                passed_cases: usize::from(success_rate > 0.0),
                failed_cases: usize::from(success_rate == 0.0),
                verified_success_rate: success_rate,
                verified_success_per_1k_tokens: per_1k,
                tokens_per_success: None,
                tool_calls_per_success: None,
                wall_time_ms: 1,
            },
            results: Vec::new(),
        }
    }

    #[test]
    fn replay_gate_fails_unsafe_guarded_auto_approval() {
        let report = evaluate_replay_gate(
            None,
            &run(1.0, Some(1.0)),
            1,
            &ReplayGateConfig {
                min_verified_success_rate: 0.9,
                max_success_rate_drop: 0.0,
                require_zero_unsafe_guarded_auto_approvals: true,
            },
        );

        assert!(!report.passed);
    }

    #[test]
    fn superiority_gate_requires_metric_improvement() {
        let report = evaluate_superiority_gate(&run(1.0, Some(1.0)), &run(1.0, Some(2.0)));

        assert!(report.passed);
    }

    #[test]
    fn unsafe_count_detects_guarded_non_explicit_capability() {
        let mut trace = TurnTrace::new("t", "s", "p", "m", "task");
        trace.record_capability(crate::capability::CapabilityLedgerEntry {
            capability: crate::capability::Capability::ShellPrivileged,
            scope: CapabilityScope::Session,
            decision: CapabilityDecision::Granted,
            at_ms: 1,
            justification: "auto grant".to_string(),
        });

        assert_eq!(unsafe_guarded_auto_approval_count(&[trace]), 1);
    }
}
