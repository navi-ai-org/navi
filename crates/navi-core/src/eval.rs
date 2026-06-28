//! Local eval harness for measuring NAVI harness behavior.
//!
//! This is the B0 "superiority baseline" foundation: versioned eval cases,
//! verifier replay, aggregate metrics, and trace-to-eval candidate generation.

use crate::trace::{TurnOutcome, TurnTrace};
use crate::verifier::{VerifierRunner, VerifierSpec};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A versioned, replayable eval case.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EvalCase {
    /// Schema version for forward compatibility.
    pub version: u32,
    /// Stable eval id, unique within a suite.
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// Suite category, e.g. "simple_repo_task" or "security_stress".
    pub category: String,
    /// Harness mode this case is intended to measure.
    pub mode: EvalMode,
    /// User-facing task prompt or objective.
    pub task: String,
    /// Optional setup commands, run before verifiers.
    pub setup: Vec<VerifierSpec>,
    /// Required and optional verification commands.
    pub verifiers: Vec<VerifierSpec>,
    /// Tags for filtering/reporting.
    pub tags: Vec<String>,
    /// Optional notes for humans.
    pub notes: Option<String>,
    /// Extensible metadata for future routing/training.
    pub metadata: BTreeMap<String, String>,
}

impl EvalCase {
    pub const CURRENT_VERSION: u32 = 1;
}

impl Default for EvalCase {
    fn default() -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            id: String::new(),
            title: String::new(),
            category: "simple_repo_task".to_string(),
            mode: EvalMode::Parity,
            task: String::new(),
            setup: Vec::new(),
            verifiers: Vec::new(),
            tags: Vec::new(),
            notes: None,
            metadata: BTreeMap::new(),
        }
    }
}

/// Harness mode under measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum EvalMode {
    /// Current linear/parity harness behavior.
    #[default]
    Parity,
    /// Verifier-first mode. B0 records the intent before the runtime mode exists.
    VerifierFirst,
    /// Branch-race mode. B0 records the intent before the runtime mode exists.
    BranchRace,
}

/// A loaded eval suite.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalSuite {
    pub name: String,
    pub cases: Vec<EvalCase>,
}

impl EvalSuite {
    /// Loads one eval case file or all `.json`/`.toml` case files in a directory.
    pub fn load(path: &Path) -> Result<Self> {
        if path.is_file() {
            let case = load_case(path)?;
            return Ok(Self {
                name: path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("eval")
                    .to_string(),
                cases: vec![case],
            });
        }

        if !path.is_dir() {
            bail!("eval path does not exist: {}", path.display());
        }

        let mut files = Vec::new();
        for entry in std::fs::read_dir(path)
            .with_context(|| format!("failed to read eval suite {}", path.display()))?
        {
            let entry = entry?;
            let entry_path = entry.path();
            if is_eval_case_file(&entry_path) {
                files.push(entry_path);
            }
        }
        files.sort();

        let mut cases = Vec::new();
        for file in files {
            cases.push(load_case(&file)?);
        }

        if cases.is_empty() {
            bail!("eval suite has no .json or .toml cases: {}", path.display());
        }
        validate_unique_case_ids(&cases, path)?;

        Ok(Self {
            name: path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("eval-suite")
                .to_string(),
            cases,
        })
    }
}

/// Result for a single eval case replay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalCaseResult {
    pub case_id: String,
    pub title: String,
    pub category: String,
    pub mode: EvalMode,
    pub passed: bool,
    pub setup_results: Vec<crate::verifier::VerifierResult>,
    pub verifier_results: Vec<crate::verifier::VerifierResult>,
    pub metrics: EvalCaseMetrics,
}

/// Metrics for one eval case.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalCaseMetrics {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub tool_calls: usize,
    pub failed_tool_calls: usize,
    pub verifier_count: usize,
    pub wall_time_ms: u64,
}

impl EvalCaseMetrics {
    fn from_verifier_results(
        setup_results: &[crate::verifier::VerifierResult],
        verifier_results: &[crate::verifier::VerifierResult],
    ) -> Self {
        let wall_time_ms = setup_results
            .iter()
            .chain(verifier_results)
            .map(|result| result.duration_ms)
            .sum();
        Self {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            tool_calls: 0,
            failed_tool_calls: 0,
            verifier_count: verifier_results.len(),
            wall_time_ms,
        }
    }
}

/// Aggregate eval run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalRun {
    pub version: u32,
    pub run_id: String,
    pub suite_name: String,
    pub started_at: u64,
    pub ended_at: u64,
    pub project_root: PathBuf,
    pub metrics: EvalRunMetrics,
    pub results: Vec<EvalCaseResult>,
}

impl EvalRun {
    pub const CURRENT_VERSION: u32 = 1;
}

/// Aggregate metrics for comparing harness modes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalRunMetrics {
    pub total_cases: usize,
    pub passed_cases: usize,
    pub failed_cases: usize,
    pub verified_success_rate: f64,
    pub verified_success_per_1k_tokens: Option<f64>,
    pub tokens_per_success: Option<f64>,
    pub tool_calls_per_success: Option<f64>,
    pub wall_time_ms: u64,
}

/// Runs verifier-based eval replay.
pub struct EvalRunner;

impl EvalRunner {
    pub async fn run_suite(suite: EvalSuite, project_root: &Path) -> EvalRun {
        let started_at = current_unix_millis();
        let mut results = Vec::new();

        for case in suite.cases {
            results.push(Self::run_case(case, project_root).await);
        }

        let ended_at = current_unix_millis();
        let metrics = aggregate_metrics(&results, ended_at.saturating_sub(started_at));
        EvalRun {
            version: EvalRun::CURRENT_VERSION,
            run_id: format!("eval-{started_at}"),
            suite_name: suite.name,
            started_at,
            ended_at,
            project_root: project_root.to_path_buf(),
            metrics,
            results,
        }
    }

    pub async fn run_case(case: EvalCase, project_root: &Path) -> EvalCaseResult {
        let mut setup_results = Vec::new();
        for spec in &case.setup {
            setup_results.push(VerifierRunner::run(spec, project_root).await);
        }

        let setup_failed = setup_results
            .iter()
            .any(|result| matches!(result.status.as_str(), "fail" | "error"));

        let mut verifier_results = Vec::new();
        if !setup_failed {
            for spec in &case.verifiers {
                verifier_results.push(VerifierRunner::run(spec, project_root).await);
            }
        }

        let passed = !setup_failed
            && required_verifiers_passed(&case.verifiers, &verifier_results)
            && !case.verifiers.is_empty();
        let metrics = EvalCaseMetrics::from_verifier_results(&setup_results, &verifier_results);

        EvalCaseResult {
            case_id: case.id,
            title: case.title,
            category: case.category,
            mode: case.mode,
            passed,
            setup_results,
            verifier_results,
            metrics,
        }
    }
}

/// Converts a successful trace with verifier evidence into an eval candidate.
pub fn eval_case_from_trace(trace: &TurnTrace) -> Option<EvalCase> {
    let verified = trace
        .verifier_results
        .iter()
        .filter(|verifier| verifier.passed)
        .collect::<Vec<_>>();
    if verified.is_empty() {
        return None;
    }

    let success_like = matches!(
        trace.outcome,
        TurnOutcome::Success | TurnOutcome::PartialSuccess
    );
    if !success_like {
        return None;
    }

    Some(EvalCase {
        id: sanitize_eval_id(&format!("trace-{}-{}", trace.session_id, trace.turn_id)),
        title: format!("Trace replay: {}", trace.task),
        category: "trace_replay_candidate".to_string(),
        mode: EvalMode::Parity,
        task: trace.task.clone(),
        verifiers: verified
            .into_iter()
            .map(|verifier| VerifierSpec {
                verifier_type: verifier.verifier.clone(),
                command: verifier.command.clone(),
                cwd: None,
                timeout_ms: Some(verifier.duration_ms.max(1).saturating_mul(4).max(30_000)),
                required: true,
            })
            .collect(),
        tags: vec!["trace-generated".to_string()],
        notes: Some(
            "Generated from a successful trace with passing verifier evidence.".to_string(),
        ),
        ..EvalCase::default()
    })
}

fn load_case(path: &Path) -> Result<EvalCase> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read eval case {}", path.display()))?;
    let case = match path.extension().and_then(|ext| ext.to_str()) {
        Some("json") => serde_json::from_str(&content)
            .with_context(|| format!("failed to parse JSON eval case {}", path.display()))?,
        Some("toml") => toml::from_str(&content)
            .with_context(|| format!("failed to parse TOML eval case {}", path.display()))?,
        _ => bail!("unsupported eval case format: {}", path.display()),
    };
    validate_case(case, path)
}

fn validate_case(case: EvalCase, path: &Path) -> Result<EvalCase> {
    if case.version != EvalCase::CURRENT_VERSION {
        bail!(
            "unsupported eval case version {} in {}",
            case.version,
            path.display()
        );
    }
    if case.id.trim().is_empty() {
        bail!("eval case missing id: {}", path.display());
    }
    if case.title.trim().is_empty() {
        bail!("eval case missing title: {}", path.display());
    }
    if case.category.trim().is_empty() {
        bail!("eval case missing category: {}", path.display());
    }
    if case.task.trim().is_empty() {
        bail!("eval case missing task: {}", path.display());
    }
    if case.verifiers.is_empty() {
        bail!(
            "eval case must define at least one verifier: {}",
            path.display()
        );
    }
    for spec in case.setup.iter().chain(&case.verifiers) {
        if spec.command.trim().is_empty() {
            bail!(
                "eval case has an empty verifier command: {}",
                path.display()
            );
        }
    }
    Ok(case)
}

fn validate_unique_case_ids(cases: &[EvalCase], suite_path: &Path) -> Result<()> {
    let mut seen = BTreeSet::new();
    for case in cases {
        if !seen.insert(case.id.as_str()) {
            bail!(
                "duplicate eval case id `{}` in suite {}",
                case.id,
                suite_path.display()
            );
        }
    }
    Ok(())
}

fn is_eval_case_file(path: &Path) -> bool {
    path.is_file()
        && matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("json" | "toml")
        )
}

fn required_verifiers_passed(
    specs: &[VerifierSpec],
    results: &[crate::verifier::VerifierResult],
) -> bool {
    specs
        .iter()
        .zip(results)
        .all(|(spec, result)| !spec.required || result.is_ok())
}

fn aggregate_metrics(results: &[EvalCaseResult], wall_time_ms: u64) -> EvalRunMetrics {
    let total_cases = results.len();
    let passed_cases = results.iter().filter(|result| result.passed).count();
    let failed_cases = total_cases.saturating_sub(passed_cases);
    let total_tokens: u64 = results
        .iter()
        .map(|result| result.metrics.total_tokens)
        .sum();
    let tool_calls: usize = results.iter().map(|result| result.metrics.tool_calls).sum();

    EvalRunMetrics {
        total_cases,
        passed_cases,
        failed_cases,
        verified_success_rate: ratio(passed_cases, total_cases),
        verified_success_per_1k_tokens: (total_tokens > 0)
            .then(|| passed_cases as f64 / (total_tokens as f64 / 1000.0)),
        tokens_per_success: (passed_cases > 0 && total_tokens > 0)
            .then(|| total_tokens as f64 / passed_cases as f64),
        tool_calls_per_success: (passed_cases > 0 && tool_calls > 0)
            .then(|| tool_calls as f64 / passed_cases as f64),
        wall_time_ms,
    }
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn sanitize_eval_id(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::TurnOutcome;

    fn verifier(command: &str) -> VerifierSpec {
        VerifierSpec {
            verifier_type: "command".to_string(),
            command: command.to_string(),
            cwd: None,
            timeout_ms: Some(10_000),
            required: true,
        }
    }

    #[test]
    fn loads_eval_suite_from_toml_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("case.toml"),
            r#"
version = 1
id = "case-one"
title = "Case One"
category = "simple_repo_task"
task = "check file"

[[verifiers]]
verifier_type = "command"
command = "test -f Cargo.toml"
required = true
"#,
        )
        .unwrap();

        let suite = EvalSuite::load(dir.path()).unwrap();

        assert_eq!(suite.cases.len(), 1);
        assert_eq!(suite.cases[0].id, "case-one");
    }

    #[test]
    fn rejects_duplicate_eval_case_ids() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["one.toml", "two.toml"] {
            std::fs::write(
                dir.path().join(name),
                r#"
version = 1
id = "duplicate"
title = "Duplicate"
category = "simple_repo_task"
task = "check file"

[[verifiers]]
verifier_type = "command"
command = "true"
required = true
"#,
            )
            .unwrap();
        }

        let error = EvalSuite::load(dir.path()).expect_err("duplicate id must fail");

        assert!(error.to_string().contains("duplicate eval case id"));
    }

    #[test]
    fn rejects_empty_verifier_command() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("case.toml"),
            r#"
version = 1
id = "bad-command"
title = "Bad Command"
category = "simple_repo_task"
task = "check file"

[[verifiers]]
verifier_type = "command"
command = ""
required = true
"#,
        )
        .unwrap();

        let error = EvalSuite::load(dir.path()).expect_err("empty command must fail");

        assert!(error.to_string().contains("empty verifier command"));
    }

    #[tokio::test]
    async fn eval_runner_fails_reproducibly_when_verifier_fails() {
        let dir = tempfile::tempdir().unwrap();
        let case = EvalCase {
            id: "missing-file".to_string(),
            title: "Missing file".to_string(),
            task: "prove missing file fails".to_string(),
            verifiers: vec![verifier("test -f definitely-missing-file")],
            ..EvalCase::default()
        };

        let result = EvalRunner::run_case(case, dir.path()).await;

        assert!(!result.passed);
        assert_eq!(result.verifier_results[0].status, "fail");
    }

    #[tokio::test]
    async fn eval_runner_aggregates_success_metrics() {
        let dir = tempfile::tempdir().unwrap();
        let case = EvalCase {
            id: "passing".to_string(),
            title: "Passing".to_string(),
            task: "prove pass".to_string(),
            verifiers: vec![verifier("true")],
            ..EvalCase::default()
        };
        let suite = EvalSuite {
            name: "test-suite".to_string(),
            cases: vec![case],
        };

        let run = EvalRunner::run_suite(suite, dir.path()).await;

        assert_eq!(run.metrics.total_cases, 1);
        assert_eq!(run.metrics.passed_cases, 1);
        assert_eq!(run.metrics.verified_success_rate, 1.0);
        assert_eq!(run.metrics.verified_success_per_1k_tokens, None);
        assert_eq!(run.metrics.tokens_per_success, None);
    }

    #[test]
    fn trace_with_passing_verifier_generates_eval_candidate() {
        let mut trace = TurnTrace::new("turn-1", "session-1", "openai", "gpt-5", "fix bug");
        trace.outcome = TurnOutcome::Success;
        trace.record_verifier("test", "just test-crate navi-core", true, 100, Some(0));

        let case = eval_case_from_trace(&trace).expect("candidate");

        assert_eq!(case.category, "trace_replay_candidate");
        assert_eq!(case.verifiers.len(), 1);
        assert_eq!(case.verifiers[0].command, "just test-crate navi-core");
    }

    #[test]
    fn trace_without_verifier_does_not_generate_eval_candidate() {
        let trace = TurnTrace::new("turn-1", "session-1", "openai", "gpt-5", "answer");

        assert!(eval_case_from_trace(&trace).is_none());
    }
}
