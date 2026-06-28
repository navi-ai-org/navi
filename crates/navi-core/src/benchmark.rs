//! Agentic benchmark schema and comparison helpers.
//!
//! Benchmarks differ from verifier-replay evals: each case is intended to run a
//! real NAVI headless turn inside an isolated fixture, then validate the result
//! with verifier commands.

use crate::event::RuntimeEvent;
use crate::verifier::VerifierResult;
use crate::verifier::VerifierSpec;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BenchCase {
    pub version: u32,
    pub id: String,
    pub title: String,
    pub category: String,
    pub fixture: PathBuf,
    pub task: String,
    pub max_turns: Option<u32>,
    pub max_tool_calls: Option<u32>,
    pub timeout_ms: Option<u64>,
    pub agent: BenchAgentConfig,
    pub setup: Vec<VerifierSpec>,
    pub verifiers: Vec<VerifierSpec>,
    pub tags: Vec<String>,
    pub notes: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

impl BenchCase {
    pub const CURRENT_VERSION: u32 = 1;
}

impl Default for BenchCase {
    fn default() -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            id: String::new(),
            title: String::new(),
            category: "agentic_repo_task".to_string(),
            fixture: PathBuf::new(),
            task: String::new(),
            max_turns: None,
            max_tool_calls: None,
            timeout_ms: None,
            agent: BenchAgentConfig::default(),
            setup: Vec::new(),
            verifiers: Vec::new(),
            tags: Vec::new(),
            notes: None,
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "kebab-case")]
pub struct BenchAgentConfig {
    pub mode: String,
    pub profile: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub active_skills: Vec<String>,
}

impl Default for BenchAgentConfig {
    fn default() -> Self {
        Self {
            mode: "parity".to_string(),
            profile: None,
            provider: None,
            model: None,
            active_skills: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BenchSuite {
    pub name: String,
    pub cases: Vec<BenchCase>,
}

impl BenchSuite {
    pub fn load(path: &Path) -> Result<Self> {
        if path.is_file() {
            let case = load_case(path)?;
            return Ok(Self {
                name: path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("bench")
                    .to_string(),
                cases: vec![case],
            });
        }

        if !path.is_dir() {
            bail!("benchmark path does not exist: {}", path.display());
        }

        let mut files = Vec::new();
        for entry in std::fs::read_dir(path)
            .with_context(|| format!("failed to read benchmark suite {}", path.display()))?
        {
            let entry = entry?;
            let entry_path = entry.path();
            if is_bench_case_file(&entry_path) {
                files.push(entry_path);
            }
        }
        files.sort();

        let mut cases = Vec::new();
        for file in files {
            cases.push(load_case(&file)?);
        }

        if cases.is_empty() {
            bail!(
                "benchmark suite has no .json or .toml cases: {}",
                path.display()
            );
        }
        validate_unique_case_ids(&cases, path)?;

        Ok(Self {
            name: path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("bench-suite")
                .to_string(),
            cases,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchRun {
    pub version: u32,
    pub run_id: String,
    pub suite_name: String,
    pub started_at: u64,
    pub ended_at: u64,
    pub project_root: PathBuf,
    pub metrics: BenchRunMetrics,
    pub results: Vec<BenchCaseResult>,
}

impl BenchRun {
    pub const CURRENT_VERSION: u32 = 1;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchCaseResult {
    pub case_id: String,
    pub title: String,
    pub category: String,
    pub passed: bool,
    pub workspace: PathBuf,
    pub assistant_text: String,
    pub setup_results: Vec<VerifierResult>,
    pub verifier_results: Vec<VerifierResult>,
    pub metrics: BenchCaseMetrics,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<RuntimeEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BenchCaseMetrics {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub turn_count: usize,
    pub tool_calls: usize,
    pub failed_tool_calls: usize,
    pub verifier_count: usize,
    pub verifier_pass_count: usize,
    pub wall_time_ms: u64,
    pub files_changed: usize,
    pub diff_lines_added: u64,
    pub diff_lines_removed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BenchRunMetrics {
    pub total_cases: usize,
    pub passed_cases: usize,
    pub failed_cases: usize,
    pub verified_success_rate: f64,
    pub tokens_per_success: Option<f64>,
    pub tool_calls_per_success: Option<f64>,
    pub wall_time_ms: u64,
    pub total_tokens: u64,
    pub tool_calls: usize,
    pub failed_tool_calls: usize,
    pub files_changed: usize,
    pub diff_lines_added: u64,
    pub diff_lines_removed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BenchComparison {
    pub passed: bool,
    pub baseline_success_rate: f64,
    pub candidate_success_rate: f64,
    pub success_rate_delta: f64,
    pub baseline_tokens_per_success: Option<f64>,
    pub candidate_tokens_per_success: Option<f64>,
    pub baseline_tool_calls_per_success: Option<f64>,
    pub candidate_tool_calls_per_success: Option<f64>,
    pub failures: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct BenchCompareConfig {
    pub min_success_rate: f64,
    pub max_success_rate_drop: f64,
    pub require_token_improvement: bool,
    pub require_tool_call_improvement: bool,
}

impl Default for BenchCompareConfig {
    fn default() -> Self {
        Self {
            min_success_rate: 1.0,
            max_success_rate_drop: 0.0,
            require_token_improvement: false,
            require_tool_call_improvement: false,
        }
    }
}

pub fn aggregate_bench_metrics(results: &[BenchCaseResult], wall_time_ms: u64) -> BenchRunMetrics {
    let total_cases = results.len();
    let passed_cases = results.iter().filter(|result| result.passed).count();
    let failed_cases = total_cases.saturating_sub(passed_cases);
    let total_tokens = results
        .iter()
        .map(|result| result.metrics.total_tokens)
        .sum();
    let tool_calls = results.iter().map(|result| result.metrics.tool_calls).sum();
    let failed_tool_calls = results
        .iter()
        .map(|result| result.metrics.failed_tool_calls)
        .sum();
    let files_changed = results
        .iter()
        .map(|result| result.metrics.files_changed)
        .sum();
    let diff_lines_added = results
        .iter()
        .map(|result| result.metrics.diff_lines_added)
        .sum();
    let diff_lines_removed = results
        .iter()
        .map(|result| result.metrics.diff_lines_removed)
        .sum();

    BenchRunMetrics {
        total_cases,
        passed_cases,
        failed_cases,
        verified_success_rate: ratio(passed_cases, total_cases),
        tokens_per_success: (passed_cases > 0 && total_tokens > 0)
            .then(|| total_tokens as f64 / passed_cases as f64),
        tool_calls_per_success: (passed_cases > 0 && tool_calls > 0)
            .then(|| tool_calls as f64 / passed_cases as f64),
        wall_time_ms,
        total_tokens,
        tool_calls,
        failed_tool_calls,
        files_changed,
        diff_lines_added,
        diff_lines_removed,
    }
}

pub fn compare_bench_runs(
    baseline: Option<&BenchRun>,
    candidate: &BenchRun,
    config: BenchCompareConfig,
) -> BenchComparison {
    let baseline_success_rate = baseline
        .map(|run| run.metrics.verified_success_rate)
        .unwrap_or(0.0);
    let candidate_success_rate = candidate.metrics.verified_success_rate;
    let mut failures = Vec::new();

    if candidate_success_rate < config.min_success_rate {
        failures.push(format!(
            "candidate success rate {:.3} is below minimum {:.3}",
            candidate_success_rate, config.min_success_rate
        ));
    }
    if let Some(baseline) = baseline {
        let allowed = baseline
            .metrics
            .verified_success_rate
            .saturating_sub(config.max_success_rate_drop);
        if candidate_success_rate < allowed {
            failures.push(format!(
                "candidate success rate {:.3} dropped below allowed {:.3}",
                candidate_success_rate, allowed
            ));
        }
        if config.require_token_improvement
            && let (Some(base), Some(candidate)) = (
                baseline.metrics.tokens_per_success,
                candidate.metrics.tokens_per_success,
            )
            && candidate > base
        {
            failures.push(format!(
                "candidate tokens_per_success {:.3} is worse than baseline {:.3}",
                candidate, base
            ));
        }
        if config.require_tool_call_improvement
            && let (Some(base), Some(candidate)) = (
                baseline.metrics.tool_calls_per_success,
                candidate.metrics.tool_calls_per_success,
            )
            && candidate > base
        {
            failures.push(format!(
                "candidate tool_calls_per_success {:.3} is worse than baseline {:.3}",
                candidate, base
            ));
        }
    }

    BenchComparison {
        passed: failures.is_empty(),
        baseline_success_rate,
        candidate_success_rate,
        success_rate_delta: candidate_success_rate - baseline_success_rate,
        baseline_tokens_per_success: baseline.and_then(|run| run.metrics.tokens_per_success),
        candidate_tokens_per_success: candidate.metrics.tokens_per_success,
        baseline_tool_calls_per_success: baseline
            .and_then(|run| run.metrics.tool_calls_per_success),
        candidate_tool_calls_per_success: candidate.metrics.tool_calls_per_success,
        failures,
    }
}

trait SaturatingSubF64 {
    fn saturating_sub(self, rhs: f64) -> f64;
}

impl SaturatingSubF64 for f64 {
    fn saturating_sub(self, rhs: f64) -> f64 {
        (self - rhs).max(0.0)
    }
}

fn load_case(path: &Path) -> Result<BenchCase> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read benchmark case {}", path.display()))?;
    let case = match path.extension().and_then(|ext| ext.to_str()) {
        Some("json") => serde_json::from_str(&content)
            .with_context(|| format!("failed to parse JSON benchmark case {}", path.display()))?,
        Some("toml") => toml::from_str(&content)
            .with_context(|| format!("failed to parse TOML benchmark case {}", path.display()))?,
        _ => bail!("unsupported benchmark case format: {}", path.display()),
    };
    validate_case(case, path)
}

fn validate_case(case: BenchCase, path: &Path) -> Result<BenchCase> {
    if case.version != BenchCase::CURRENT_VERSION {
        bail!(
            "unsupported benchmark case version {} in {}",
            case.version,
            path.display()
        );
    }
    if case.id.trim().is_empty() {
        bail!("benchmark case missing id: {}", path.display());
    }
    if case.title.trim().is_empty() {
        bail!("benchmark case missing title: {}", path.display());
    }
    if case.category.trim().is_empty() {
        bail!("benchmark case missing category: {}", path.display());
    }
    if case.fixture.as_os_str().is_empty() {
        bail!("benchmark case missing fixture: {}", path.display());
    }
    if case.task.trim().is_empty() {
        bail!("benchmark case missing task: {}", path.display());
    }
    if case.verifiers.is_empty() {
        bail!(
            "benchmark case must define at least one verifier: {}",
            path.display()
        );
    }
    for spec in case.setup.iter().chain(&case.verifiers) {
        if spec.command.trim().is_empty() {
            bail!(
                "benchmark case has an empty verifier command: {}",
                path.display()
            );
        }
    }
    Ok(case)
}

fn validate_unique_case_ids(cases: &[BenchCase], suite_path: &Path) -> Result<()> {
    let mut seen = BTreeSet::new();
    for case in cases {
        if !seen.insert(case.id.as_str()) {
            bail!(
                "duplicate benchmark case id `{}` in suite {}",
                case.id,
                suite_path.display()
            );
        }
    }
    Ok(())
}

fn is_bench_case_file(path: &Path) -> bool {
    path.is_file()
        && matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("json" | "toml")
        )
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verifier(command: &str) -> VerifierSpec {
        VerifierSpec {
            verifier_type: "command".to_string(),
            command: command.to_string(),
            cwd: None,
            timeout_ms: None,
            required: true,
        }
    }

    fn result(id: &str, passed: bool, tokens: u64, tool_calls: usize) -> BenchCaseResult {
        BenchCaseResult {
            case_id: id.to_string(),
            title: id.to_string(),
            category: "test".to_string(),
            passed,
            workspace: PathBuf::from("/tmp/workspace"),
            assistant_text: String::new(),
            setup_results: Vec::new(),
            verifier_results: Vec::new(),
            metrics: BenchCaseMetrics {
                total_tokens: tokens,
                tool_calls,
                ..BenchCaseMetrics::default()
            },
            events: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn loads_benchmark_suite_from_toml_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("case.toml"),
            r#"
version = 1
id = "bench-case"
title = "Bench case"
category = "repo"
fixture = "benchmarks/fixtures/example"
task = "Fix the repo."

[[verifiers]]
verifier_type = "command"
command = "test -f Cargo.toml"
required = true
"#,
        )
        .unwrap();

        let suite = BenchSuite::load(dir.path()).unwrap();

        assert_eq!(
            suite.name,
            dir.path().file_name().unwrap().to_string_lossy()
        );
        assert_eq!(suite.cases[0].id, "bench-case");
        assert_eq!(suite.cases[0].agent.mode, "parity");
    }

    #[test]
    fn rejects_duplicate_benchmark_case_ids() {
        let dir = tempfile::tempdir().unwrap();
        let case = r#"
version = 1
id = "duplicate"
title = "Bench case"
category = "repo"
fixture = "benchmarks/fixtures/example"
task = "Fix the repo."

[[verifiers]]
verifier_type = "command"
command = "true"
required = true
"#;
        std::fs::write(dir.path().join("a.toml"), case).unwrap();
        std::fs::write(dir.path().join("b.toml"), case).unwrap();

        let error = BenchSuite::load(dir.path()).expect_err("duplicates must fail");

        assert!(error.to_string().contains("duplicate benchmark case id"));
    }

    #[test]
    fn aggregates_metrics_for_success_rates_and_efficiency() {
        let metrics =
            aggregate_bench_metrics(&[result("a", true, 100, 3), result("b", false, 50, 2)], 250);

        assert_eq!(metrics.total_cases, 2);
        assert_eq!(metrics.passed_cases, 1);
        assert_eq!(metrics.verified_success_rate, 0.5);
        assert_eq!(metrics.tokens_per_success, Some(150.0));
        assert_eq!(metrics.tool_calls_per_success, Some(5.0));
        assert_eq!(metrics.wall_time_ms, 250);
    }

    #[test]
    fn compare_reports_success_rate_regression() {
        let baseline = BenchRun {
            version: 1,
            run_id: "base".to_string(),
            suite_name: "suite".to_string(),
            started_at: 1,
            ended_at: 2,
            project_root: PathBuf::from("."),
            metrics: aggregate_bench_metrics(&[result("a", true, 100, 3)], 10),
            results: Vec::new(),
        };
        let candidate = BenchRun {
            version: 1,
            run_id: "candidate".to_string(),
            suite_name: "suite".to_string(),
            started_at: 1,
            ended_at: 2,
            project_root: PathBuf::from("."),
            metrics: aggregate_bench_metrics(&[result("a", false, 100, 3)], 10),
            results: Vec::new(),
        };

        let comparison = compare_bench_runs(Some(&baseline), &candidate, Default::default());

        assert!(!comparison.passed);
        assert!(
            comparison
                .failures
                .iter()
                .any(|failure| failure.contains("success rate"))
        );
    }

    #[test]
    fn verifier_helper_keeps_required_default_explicit() {
        assert!(verifier("true").required);
    }
}
