//! Branch racing primitives.
//!
//! The first implementation is a deterministic planning/scoring layer over
//! verifier evidence and effect risk. Runtime workers can execute these plans
//! in snapshots or worktrees without changing the scoring contract.

use crate::effect::{BlastRadius, EffectReport};
use crate::verifier::VerifierResult;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BranchRaceRequest {
    pub task: String,
    pub strategies: Vec<BranchStrategy>,
    pub verifier_commands: Vec<String>,
    pub max_parallel: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BranchStrategy {
    MinimalFix,
    TestFirst,
    RefactorSafe,
    RollbackRevert,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BranchHypothesis {
    pub id: String,
    pub strategy: BranchStrategy,
    pub summary: String,
    pub worktree_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BranchCandidate {
    pub hypothesis: BranchHypothesis,
    pub verifier_results: Vec<VerifierResult>,
    pub effect_report: Option<EffectReport>,
    pub diff_stat_files: usize,
    pub diff_stat_lines: usize,
    pub reviewer_passed: bool,
    pub security_reviewer_passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BranchRaceReport {
    pub task: String,
    pub candidates: Vec<ScoredBranchCandidate>,
    pub winner_id: Option<String>,
    pub rejected: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScoredBranchCandidate {
    pub candidate: BranchCandidate,
    pub score: f64,
    pub reasons: Vec<String>,
}

pub struct BranchRacePlanner;

impl BranchRacePlanner {
    pub fn plan(request: &BranchRaceRequest) -> Vec<BranchHypothesis> {
        let strategies = if request.strategies.is_empty() {
            vec![
                BranchStrategy::MinimalFix,
                BranchStrategy::TestFirst,
                BranchStrategy::RefactorSafe,
            ]
        } else {
            request.strategies.clone()
        };

        strategies
            .into_iter()
            .enumerate()
            .map(|(idx, strategy)| BranchHypothesis {
                id: format!("branch-{}-{}", idx + 1, strategy_slug(&strategy)),
                summary: strategy_summary(&strategy, &request.task),
                strategy,
                worktree_path: None,
            })
            .collect()
    }

    pub fn score(candidate: BranchCandidate) -> ScoredBranchCandidate {
        let mut score = 0.0;
        let mut reasons = Vec::new();
        let required_total = candidate.verifier_results.len();
        let required_passed = candidate
            .verifier_results
            .iter()
            .filter(|result| result.is_ok())
            .count();

        if required_total > 0 {
            let ratio = required_passed as f64 / required_total as f64;
            score += ratio * 70.0;
            reasons.push(format!(
                "{required_passed}/{required_total} verifiers passed"
            ));
        }
        if candidate.reviewer_passed {
            score += 10.0;
            reasons.push("independent reviewer passed".to_string());
        }
        if candidate.security_reviewer_passed {
            score += 10.0;
            reasons.push("security reviewer passed".to_string());
        }

        let diff_penalty = (candidate.diff_stat_files as f64 * 1.5)
            + (candidate.diff_stat_lines as f64 / 200.0).min(8.0);
        score -= diff_penalty;
        if diff_penalty > 0.0 {
            reasons.push(format!("diff penalty {:.2}", diff_penalty));
        }

        if let Some(report) = &candidate.effect_report {
            let penalty = blast_radius_penalty(report.blast_radius);
            score -= penalty;
            reasons.push(format!("effect risk penalty {:.1}", penalty));
        }

        ScoredBranchCandidate {
            candidate,
            score,
            reasons,
        }
    }

    pub fn report(task: impl Into<String>, candidates: Vec<BranchCandidate>) -> BranchRaceReport {
        let mut scored = candidates.into_iter().map(Self::score).collect::<Vec<_>>();
        scored.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    left.candidate
                        .hypothesis
                        .id
                        .cmp(&right.candidate.hypothesis.id)
                })
        });
        let winner_id = scored
            .first()
            .filter(|candidate| candidate.score > 0.0)
            .map(|candidate| candidate.candidate.hypothesis.id.clone());
        let rejected = scored
            .iter()
            .skip(usize::from(winner_id.is_some()))
            .map(|candidate| candidate.candidate.hypothesis.id.clone())
            .collect();
        BranchRaceReport {
            task: task.into(),
            candidates: scored,
            winner_id,
            rejected,
        }
    }
}

fn strategy_slug(strategy: &BranchStrategy) -> String {
    match strategy {
        BranchStrategy::MinimalFix => "minimal-fix".to_string(),
        BranchStrategy::TestFirst => "test-first".to_string(),
        BranchStrategy::RefactorSafe => "refactor-safe".to_string(),
        BranchStrategy::RollbackRevert => "rollback-revert".to_string(),
        BranchStrategy::Custom(value) => value
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect(),
    }
}

fn strategy_summary(strategy: &BranchStrategy, task: &str) -> String {
    match strategy {
        BranchStrategy::MinimalFix => format!("Apply the smallest verifiable fix for: {task}"),
        BranchStrategy::TestFirst => format!("Add or run focused tests before fixing: {task}"),
        BranchStrategy::RefactorSafe => {
            format!("Prefer structure-preserving changes with low blast radius: {task}")
        }
        BranchStrategy::RollbackRevert => {
            format!("Evaluate rollback or revert as a safer solution for: {task}")
        }
        BranchStrategy::Custom(value) => format!("{value}: {task}"),
    }
}

fn blast_radius_penalty(radius: BlastRadius) -> f64 {
    match radius {
        BlastRadius::SingleFile => 0.0,
        BlastRadius::MultipleFiles => 4.0,
        BlastRadius::DependencyChange => 12.0,
        BlastRadius::CiConfig => 16.0,
        BlastRadius::SecuritySensitive => 40.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, passed: bool, files: usize, lines: usize) -> BranchCandidate {
        BranchCandidate {
            hypothesis: BranchHypothesis {
                id: id.to_string(),
                strategy: BranchStrategy::MinimalFix,
                summary: id.to_string(),
                worktree_path: None,
            },
            verifier_results: vec![if passed {
                VerifierResult::pass("true", 1)
            } else {
                VerifierResult::fail("false", 1, String::new(), String::new(), 1, None, None)
            }],
            effect_report: None,
            diff_stat_files: files,
            diff_stat_lines: lines,
            reviewer_passed: true,
            security_reviewer_passed: true,
        }
    }

    #[test]
    fn planner_creates_default_hypotheses() {
        let request = BranchRaceRequest {
            task: "fix bug".to_string(),
            strategies: Vec::new(),
            verifier_commands: vec!["just test".to_string()],
            max_parallel: 2,
        };

        let plan = BranchRacePlanner::plan(&request);

        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].id, "branch-1-minimal-fix");
    }

    #[test]
    fn report_selects_verified_lower_diff_winner() {
        let report = BranchRacePlanner::report(
            "fix",
            vec![
                candidate("wide", true, 10, 1000),
                candidate("small", true, 1, 20),
            ],
        );

        assert_eq!(report.winner_id.as_deref(), Some("small"));
        assert_eq!(report.rejected, vec!["wide"]);
    }
}
