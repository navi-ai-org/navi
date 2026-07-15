use anyhow::{Context, Result};
use navi_core::{
    ApprovalDecision, BenchCase, BenchCaseMetrics, BenchCaseResult, BenchCompareConfig, BenchRun,
    BenchSuite, HarnessProfile, LoadedConfig, PermissionMode, PlanReviewDecision,
    PlanReviewResponse, ProviderConfig, ProviderKind, RuntimeEvent, RuntimeEventKind,
    ThinkingConfig, ToolResult, VerifierResult, VerifierRunner, aggregate_bench_metrics,
    canonical_provider_id, compare_bench_runs,
};
use navi_sdk::{NaviEngineBuilder, NaviSessionRequest, NaviTurnRequest};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

use crate::BenchAction;

pub async fn handle_bench_command(
    action: BenchAction,
    loaded_config: LoadedConfig,
    cwd: PathBuf,
) -> Result<()> {
    match action {
        BenchAction::Run {
            path,
            output,
            project,
            json,
            provider,
            model,
            auto_approve,
            keep_workspaces,
        } => {
            run_benchmark(
                path,
                project.unwrap_or(cwd),
                loaded_config,
                output,
                json,
                provider,
                model,
                auto_approve,
                keep_workspaces,
            )
            .await
        }
        BenchAction::Compare {
            candidate,
            baseline,
            min_success_rate,
            max_success_drop,
            require_token_improvement,
            require_tool_call_improvement,
            json,
        } => compare_benchmark(
            candidate,
            baseline,
            BenchCompareConfig {
                min_success_rate,
                max_success_rate_drop: max_success_drop,
                require_token_improvement,
                require_tool_call_improvement,
            },
            json,
        ),
    }
}

async fn run_benchmark(
    path: PathBuf,
    project_root: PathBuf,
    loaded_config: LoadedConfig,
    output: Option<PathBuf>,
    json: bool,
    provider: Option<String>,
    model: Option<String>,
    auto_approve: bool,
    keep_workspaces: bool,
) -> Result<()> {
    let suite = BenchSuite::load(&path)?;
    let started_at = current_unix_millis();
    let started = Instant::now();
    let run_provider = provider
        .clone()
        .unwrap_or_else(|| loaded_config.config.model.provider.clone());
    let run_model = model
        .clone()
        .unwrap_or_else(|| loaded_config.config.model.name.clone());
    let mut results = Vec::new();

    for case in suite.cases {
        results.push(
            run_case(
                case,
                &project_root,
                loaded_config.clone(),
                provider.as_deref(),
                model.as_deref(),
                auto_approve,
                keep_workspaces,
            )
            .await,
        );
    }

    let ended_at = current_unix_millis();
    let metrics = aggregate_bench_metrics(&results, started.elapsed().as_millis() as u64);
    let run = BenchRun {
        version: BenchRun::CURRENT_VERSION,
        run_id: format!("bench-{started_at}"),
        suite_name: suite.name,
        provider: Some(run_provider),
        model: Some(run_model),
        started_at,
        ended_at,
        project_root,
        metrics,
        results,
    };

    if let Some(path) = output {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(&run)?)?;
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&run)?);
        return Ok(());
    }

    print_run_summary(&run);
    if run.metrics.failed_cases > 0 {
        anyhow::bail!("{} benchmark case(s) failed", run.metrics.failed_cases);
    }
    Ok(())
}

async fn run_case(
    case: BenchCase,
    project_root: &Path,
    loaded_config: LoadedConfig,
    provider_override: Option<&str>,
    model_override: Option<&str>,
    auto_approve: bool,
    keep_workspace: bool,
) -> BenchCaseResult {
    let started = Instant::now();
    let prepared = prepare_workspace(&case, project_root, keep_workspace);
    let workspace = match prepared {
        Ok(workspace) => workspace,
        Err(error) => {
            return BenchCaseResult {
                case_id: case.id,
                title: case.title,
                category: case.category,
                passed: false,
                workspace: PathBuf::new(),
                assistant_text: String::new(),
                setup_results: Vec::new(),
                verifier_results: Vec::new(),
                metrics: BenchCaseMetrics {
                    wall_time_ms: started.elapsed().as_millis() as u64,
                    ..BenchCaseMetrics::default()
                },
                events: Vec::new(),
                error: Some(error.to_string()),
            };
        }
    };
    let workspace_path = workspace.path().to_path_buf();
    let before = snapshot_workspace(&workspace_path).unwrap_or_default();

    let mut setup_results = Vec::new();
    for spec in &case.setup {
        setup_results.push(VerifierRunner::run(spec, &workspace_path).await);
    }
    let setup_failed = setup_results
        .iter()
        .any(|result| matches!(result.status.as_str(), "fail" | "error"));

    let mut events = Vec::new();
    let mut assistant_text = String::new();
    let mut agent_error = None;
    if !setup_failed {
        match run_agent_turn(
            &case,
            &workspace_path,
            loaded_config,
            provider_override,
            model_override,
            auto_approve,
            &mut events,
        )
        .await
        {
            Ok(text) => assistant_text = text,
            Err(error) => agent_error = Some(error.to_string()),
        }
    }

    let mut verifier_results = Vec::new();
    if !setup_failed {
        for spec in &case.verifiers {
            verifier_results.push(VerifierRunner::run(spec, &workspace_path).await);
        }
    }

    let after = snapshot_workspace(&workspace_path).unwrap_or_default();
    let diff = diff_snapshots(&before, &after);
    let mut metrics = metrics_from_events(&events);
    metrics.wall_time_ms = started.elapsed().as_millis() as u64;
    metrics.verifier_count = verifier_results.len();
    metrics.verifier_pass_count = verifier_results
        .iter()
        .filter(|result| result.is_ok())
        .count();
    metrics.files_changed = diff.files_changed;
    metrics.diff_lines_added = diff.lines_added;
    metrics.diff_lines_removed = diff.lines_removed;

    let passed = agent_error.is_none()
        && !setup_failed
        && !case.verifiers.is_empty()
        && required_verifiers_passed(&case.verifiers, &verifier_results);
    let workspace_path = workspace.into_path_if_persisted();

    BenchCaseResult {
        case_id: case.id,
        title: case.title,
        category: case.category,
        passed,
        workspace: workspace_path,
        assistant_text,
        setup_results,
        verifier_results,
        metrics,
        events,
        error: agent_error,
    }
}

async fn run_agent_turn(
    case: &BenchCase,
    workspace: &Path,
    loaded_config: LoadedConfig,
    provider_override: Option<&str>,
    model_override: Option<&str>,
    auto_approve: bool,
    events_out: &mut Vec<RuntimeEvent>,
) -> Result<String> {
    let loaded_config = apply_agent_config(loaded_config, case, provider_override, model_override)?;
    let engine = Arc::new(
        NaviEngineBuilder::from_project(workspace.to_path_buf())
            .loaded_config(loaded_config)
            .build()?,
    );
    let session = engine
        .start_session(NaviSessionRequest {
            project_dir: Some(workspace.to_path_buf()),
            active_skills: case.agent.active_skills.clone(),
            ..NaviSessionRequest::default()
        })
        .await?;
    let mut receiver = engine.subscribe_events(&session.id)?;
    let mut limits = BenchLimitState::new(case.max_turns, case.max_tool_calls);
    let request = NaviTurnRequest {
        session_id: session.id.clone(),
        message: bench_task_message(case),
        content_parts: Vec::new(),
        context_packets: Vec::new(),
        // Keep maximum reasoning available; token wins come from context engineering,
        // not from disabling structured thinking.
        thinking: Some(ThinkingConfig::Max),
    };
    let send_engine = Arc::clone(&engine);
    let mut send_task = tokio::spawn(async move { send_engine.send_turn(request).await });
    let timeout = Duration::from_millis(case.timeout_ms.unwrap_or(600_000));
    let timeout_sleep = tokio::time::sleep(timeout);
    tokio::pin!(timeout_sleep);

    let response = loop {
        tokio::select! {
            joined = &mut send_task => {
                break joined.context("benchmark turn task join error")??;
            }
            _ = &mut timeout_sleep => {
                let _ = engine.cancel_turn(&session.id).await;
                send_task.abort();
                let _ = engine.close_session(&session.id).await;
                anyhow::bail!("benchmark case `{}` timed out after {} ms", case.id, timeout.as_millis());
            }
            event = receiver.recv() => {
                match event {
                    Ok(event) => {
                        // Headless bench has no human: auto-resolve interactive gates.
                        if auto_approve {
                            match &event.kind {
                                RuntimeEventKind::ApprovalRequired(request) => {
                                    let _ = engine
                                        .resolve_approval(
                                            &session.id,
                                            ApprovalDecision::Approved {
                                                id: request.id.clone(),
                                            },
                                        )
                                        .await;
                                }
                                RuntimeEventKind::PlanReviewRequired(request) => {
                                    // Models sometimes enter plan mode; approving unblocks the turn.
                                    let _ = engine
                                        .resolve_plan_review(
                                            &session.id,
                                            PlanReviewResponse {
                                                id: request.id.clone(),
                                                plan_id: request.plan_id.clone(),
                                                decision: PlanReviewDecision::Approve,
                                                comments: Vec::new(),
                                                freeform: String::new(),
                                            },
                                        )
                                        .await;
                                }
                                RuntimeEventKind::QuestionRequired(request) => {
                                    use navi_core::event::QuestionResponse;
                                    let answer = request
                                        .options
                                        .first()
                                        .map(|o| o.label.clone())
                                        .unwrap_or_else(|| "ok".to_string());
                                    let _ = engine
                                        .resolve_question(
                                            &session.id,
                                            QuestionResponse::Answered {
                                                id: request.id.clone(),
                                                answers: vec![answer],
                                            },
                                        )
                                        .await;
                                }
                                _ => {}
                            }
                        }
                        let limit_error = limits.observe(&event);
                        events_out.push(event);
                        if let Err(error) = limit_error {
                            let _ = engine.cancel_turn(&session.id).await;
                            send_task.abort();
                            let _ = engine.close_session(&session.id).await;
                            anyhow::bail!("benchmark case `{}` exceeded limit: {error}", case.id);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break navi_sdk::NaviTurnResponse {
                        session_id: session.id.clone(),
                        text: String::new(),
                    },
                }
            }
        }
    };

    drain_ready_events(&mut receiver, events_out);
    let _ = engine.snapshot_session(&session.id).await;
    let _ = engine.close_session(&session.id).await;
    Ok(response.text)
}

#[derive(Default)]
struct BenchLimitState {
    max_turns: Option<u32>,
    max_tool_calls: Option<u32>,
    turns: u32,
    tool_calls: u32,
}

impl BenchLimitState {
    fn new(max_turns: Option<u32>, max_tool_calls: Option<u32>) -> Self {
        Self {
            max_turns,
            max_tool_calls,
            turns: 0,
            tool_calls: 0,
        }
    }

    fn observe(&mut self, event: &RuntimeEvent) -> Result<()> {
        match &event.kind {
            RuntimeEventKind::TurnStarted { .. } => {
                self.turns = self.turns.saturating_add(1);
                if let Some(max_turns) = self.max_turns
                    && self.turns > max_turns
                {
                    anyhow::bail!("turn count {} exceeded max_turns {}", self.turns, max_turns);
                }
            }
            RuntimeEventKind::ToolRequested(_) => {
                self.tool_calls = self.tool_calls.saturating_add(1);
                if let Some(max_tool_calls) = self.max_tool_calls
                    && self.tool_calls > max_tool_calls
                {
                    anyhow::bail!(
                        "tool call count {} exceeded max_tool_calls {}",
                        self.tool_calls,
                        max_tool_calls
                    );
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Headless-only: tools that require a human UI or idle forever.
/// Do **not** ban plan/goal/codebase tools — that trades tokens for quality.
const BENCH_HEADLESS_DENY_TOOLS: &[&str] = &[
    "request_user_input", // blocks without TUI
    "sleep",              // pure wall-time burn in automated runs
];

/// Soft calibration (not a ban). Prefer direct fix on small scoped bugs;
/// plan/goal remain available when the work is multi-step or multi-module.
const BENCH_SCOPE_PREAMBLE: &str = "\
[navi-bench scope guidance]
- Default: inspect → edit → verify. Do not plan a one-line or one-file fix.
- Use the plan tool only when multi-module, ambiguous, or high-risk.
- set_goal is only for long-running thread goals — not a synonym for plan.
- Prefer apply_patch for surgical edits; re-read a file only when tests show new errors.
";

fn apply_agent_config(
    mut loaded_config: LoadedConfig,
    case: &BenchCase,
    provider_override: Option<&str>,
    model_override: Option<&str>,
) -> Result<LoadedConfig> {
    if let Some(provider) = provider_override {
        loaded_config.config.model.provider = provider.to_string();
    }
    if let Some(model) = model_override {
        loaded_config.config.model.name = model.to_string();
    }
    if let Some(provider) = &case.agent.provider {
        loaded_config.config.model.provider = provider.clone();
    }
    if let Some(model) = &case.agent.model {
        loaded_config.config.model.name = model.clone();
    }
    if let Some(profile) = &case.agent.profile {
        loaded_config.config.harness.profile = match profile.as_str() {
            "auto" => HarnessProfile::Auto,
            "small" => HarnessProfile::Small,
            "medium" => HarnessProfile::Medium,
            "long-running" => HarnessProfile::LongRunning,
            other => anyhow::bail!("unsupported benchmark harness profile `{other}`"),
        };
    }
    if let Some(max_tool_calls) = case.max_tool_calls {
        let max_tool_calls = max_tool_calls as usize;
        loaded_config.config.harness.max_tool_calls_small = max_tool_calls;
        loaded_config.config.harness.max_tool_calls_medium = max_tool_calls;
    }

    // Headless: auto-run tools; plan/goal stay enabled (auto-approved on review events).
    loaded_config.config.tui.yolo_mode = true;
    loaded_config.config.security.permission_mode = PermissionMode::Yolo;
    // Full tool-coverage audits set NAVI_BENCH_ALLOW_ALL_TOOLS=1 to exercise sleep /
    // request_user_input (still auto-resolved / short-lived when possible).
    let allow_all_tools = std::env::var("NAVI_BENCH_ALLOW_ALL_TOOLS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if !allow_all_tools {
        for name in BENCH_HEADLESS_DENY_TOOLS {
            let name = (*name).to_string();
            if !loaded_config
                .config
                .security
                .deny_tools
                .iter()
                .any(|t| t == &name)
            {
                loaded_config.config.security.deny_tools.push(name);
            }
        }
    }
    // Mild observation caps — shrink context without starving multi-file work.
    loaded_config.config.harness.observation_bytes_small = loaded_config
        .config
        .harness
        .observation_bytes_small
        .min(10 * 1024);
    loaded_config.config.harness.observation_bytes_medium = loaded_config
        .config
        .harness
        .observation_bytes_medium
        .min(24 * 1024);
    // Native tool calling is on for these providers; skip duplicate tool-manifest prose.
    use navi_core::config::ToolPromptManifest;
    loaded_config.config.harness.tool_prompt_manifest = ToolPromptManifest::Never;

    // Optional metrics proxy: route the selected provider through a local reverse
    // proxy so the bench can measure cache hits / prefix breaks on every request.
    if let Ok(base_url) = std::env::var("NAVI_BENCH_BASE_URL") {
        let base_url = base_url.trim().to_string();
        if !base_url.is_empty() {
            let provider_id = loaded_config.config.model.provider.clone();
            if let Some(provider) = loaded_config.config.providers.iter_mut().find(|p| {
                p.id == provider_id
                    || canonical_provider_id(&p.id) == canonical_provider_id(&provider_id)
            }) {
                provider.base_url = Some(base_url);
            } else {
                loaded_config.config.providers.push(ProviderConfig {
                    id: provider_id,
                    label: "bench-proxy".to_string(),
                    description: "NAVI_BENCH_BASE_URL override".to_string(),
                    kind: ProviderKind::OpenAiChatCompletions,
                    api_key_env: "OPENCODE_API_KEY".to_string(),
                    base_url: Some(base_url),
                    ..Default::default()
                });
            }
        }
    }

    Ok(loaded_config)
}

fn bench_task_message(case: &BenchCase) -> String {
    format!("{BENCH_SCOPE_PREAMBLE}\n{}", case.task.trim())
}

fn drain_ready_events(
    receiver: &mut tokio::sync::broadcast::Receiver<RuntimeEvent>,
    events_out: &mut Vec<RuntimeEvent>,
) {
    while let Ok(event) = receiver.try_recv() {
        events_out.push(event);
    }
}

fn metrics_from_events(events: &[RuntimeEvent]) -> BenchCaseMetrics {
    let mut metrics = BenchCaseMetrics::default();
    for event in events {
        match &event.kind {
            RuntimeEventKind::TurnStarted { .. } => {
                metrics.turn_count += 1;
            }
            RuntimeEventKind::ToolRequested(_) => {
                metrics.tool_calls += 1;
            }
            RuntimeEventKind::ToolCompleted(result)
                if !result.ok && counts_as_failed_tool_call(result) =>
            {
                metrics.failed_tool_calls += 1;
            }
            RuntimeEventKind::TokensUpdated {
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            } => {
                // Gross provider input (includes cache-read volume when reported as input).
                metrics.input_tokens = metrics.input_tokens.saturating_add(*input_tokens);
                metrics.output_tokens = metrics.output_tokens.saturating_add(*output_tokens);
                metrics.total_tokens = metrics
                    .total_tokens
                    .saturating_add(input_tokens.saturating_add(*output_tokens));
                metrics.cache_read_tokens =
                    metrics.cache_read_tokens.saturating_add(*cache_read_tokens);
                metrics.cache_write_tokens = metrics
                    .cache_write_tokens
                    .saturating_add(*cache_creation_tokens);
            }
            _ => {}
        }
    }
    metrics
}

fn counts_as_failed_tool_call(result: &ToolResult) -> bool {
    !is_diagnostic_process_exit(result)
}

fn is_diagnostic_process_exit(result: &ToolResult) -> bool {
    result
        .output
        .get("status")
        .and_then(Value::as_i64)
        .is_some()
        && (result.output.get("stdout").is_some() || result.output.get("stderr").is_some())
}

fn required_verifiers_passed(
    specs: &[navi_core::VerifierSpec],
    results: &[VerifierResult],
) -> bool {
    specs
        .iter()
        .zip(results)
        .all(|(spec, result)| !spec.required || result.is_ok())
}

fn prepare_workspace(
    case: &BenchCase,
    project_root: &Path,
    keep_workspace: bool,
) -> Result<PreparedWorkspace> {
    let fixture = resolve_fixture(project_root, &case.fixture);
    if !fixture.is_dir() {
        anyhow::bail!(
            "benchmark fixture for `{}` does not exist or is not a directory: {}",
            case.id,
            fixture.display()
        );
    }
    let tempdir = tempfile::Builder::new()
        .prefix(&format!("navi-bench-{}-", sanitize_path_part(&case.id)))
        .tempdir()?;
    copy_dir_recursive(&fixture, tempdir.path())?;
    initialize_git_workspace(tempdir.path())?;
    if keep_workspace {
        let path = tempdir.keep();
        Ok(PreparedWorkspace::Persisted(path))
    } else {
        Ok(PreparedWorkspace::Temporary(tempdir))
    }
}

fn resolve_fixture(project_root: &Path, fixture: &Path) -> PathBuf {
    if fixture.is_absolute() {
        fixture.to_path_buf()
    } else {
        project_root.join(fixture)
    }
}

enum PreparedWorkspace {
    Temporary(TempDir),
    Persisted(PathBuf),
}

impl PreparedWorkspace {
    fn path(&self) -> &Path {
        match self {
            Self::Temporary(tempdir) => tempdir.path(),
            Self::Persisted(path) => path,
        }
    }

    fn into_path_if_persisted(self) -> PathBuf {
        match self {
            Self::Temporary(tempdir) => tempdir.path().to_path_buf(),
            Self::Persisted(path) => path,
        }
    }
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)
        .with_context(|| format!("failed to read fixture {}", source.display()))?
    {
        let entry = entry?;
        let file_name = entry.file_name();
        if file_name == ".git" {
            continue;
        }
        let source_path = entry.path();
        let destination_path = destination.join(file_name);
        if source_path.is_dir() {
            copy_dir_recursive(&source_path, &destination_path)?;
        } else {
            std::fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "failed to copy fixture file {} to {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn initialize_git_workspace(workspace: &Path) -> Result<()> {
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(workspace)
        .status()
        .context("failed to start git init for benchmark workspace")?;
    if !status.success() {
        anyhow::bail!(
            "git init failed for benchmark workspace {}",
            workspace.display()
        );
    }
    Ok(())
}

#[derive(Default)]
struct WorkspaceDiff {
    files_changed: usize,
    lines_added: u64,
    lines_removed: u64,
}

fn snapshot_workspace(root: &Path) -> Result<BTreeMap<PathBuf, String>> {
    let mut snapshot = BTreeMap::new();
    snapshot_dir(root, root, &mut snapshot)?;
    Ok(snapshot)
}

fn snapshot_dir(root: &Path, dir: &Path, snapshot: &mut BTreeMap<PathBuf, String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_name = entry.file_name();
        if file_name == ".git" || file_name == "target" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            snapshot_dir(root, &path, snapshot)?;
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        snapshot.insert(relative, content);
    }
    Ok(())
}

fn diff_snapshots(
    before: &BTreeMap<PathBuf, String>,
    after: &BTreeMap<PathBuf, String>,
) -> WorkspaceDiff {
    let mut diff = WorkspaceDiff::default();
    let paths = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for path in paths {
        let before = before.get(&path);
        let after = after.get(&path);
        if before == after {
            continue;
        }
        diff.files_changed += 1;
        let before_lines = before.map(|content| content.lines().count()).unwrap_or(0);
        let after_lines = after.map(|content| content.lines().count()).unwrap_or(0);
        if after_lines >= before_lines {
            diff.lines_added += (after_lines - before_lines) as u64;
        } else {
            diff.lines_removed += (before_lines - after_lines) as u64;
        }
    }
    diff
}

fn compare_benchmark(
    candidate_path: PathBuf,
    baseline_path: Option<PathBuf>,
    config: BenchCompareConfig,
    json: bool,
) -> Result<()> {
    let candidate: BenchRun = serde_json::from_str(&std::fs::read_to_string(&candidate_path)?)?;
    let baseline = match baseline_path.as_ref() {
        Some(path) => Some(serde_json::from_str::<BenchRun>(&std::fs::read_to_string(
            path,
        )?)?),
        None => None,
    };
    let comparison = compare_bench_runs(baseline.as_ref(), &candidate, config);

    if json {
        println!("{}", serde_json::to_string_pretty(&comparison)?);
    } else {
        println!(
            "bench compare: {}",
            if comparison.passed { "PASS" } else { "FAIL" }
        );
        println!(
            "success_rate: baseline {:.1}% candidate {:.1}% delta {:+.1}%",
            comparison.baseline_success_rate * 100.0,
            comparison.candidate_success_rate * 100.0,
            comparison.success_rate_delta * 100.0
        );
        if let Some(value) = comparison.candidate_tokens_per_success {
            println!("candidate tokens_per_success: {value:.1}");
        }
        if let Some(value) = comparison.candidate_tool_calls_per_success {
            println!("candidate tool_calls_per_success: {value:.1}");
        }
        for failure in &comparison.failures {
            println!("failure: {failure}");
        }
    }

    if !comparison.passed {
        anyhow::bail!("benchmark comparison failed");
    }
    Ok(())
}

fn print_run_summary(run: &BenchRun) {
    println!(
        "Benchmark suite `{}`: {}/{} passed ({:.1}%)",
        run.suite_name,
        run.metrics.passed_cases,
        run.metrics.total_cases,
        run.metrics.verified_success_rate * 100.0
    );
    println!(
        "tokens_per_success: {}",
        run.metrics
            .tokens_per_success
            .map(|value| format!("{value:.1}"))
            .unwrap_or_else(|| "n/a".to_string())
    );
    println!(
        "tool_calls_per_success: {}",
        run.metrics
            .tool_calls_per_success
            .map(|value| format!("{value:.1}"))
            .unwrap_or_else(|| "n/a".to_string())
    );

    for result in &run.results {
        let status = if result.passed { "PASS" } else { "FAIL" };
        println!("[{status}] {} - {}", result.case_id, result.title);
        if let Some(error) = &result.error {
            println!("  agent error: {error}");
        }
        for verifier in result
            .setup_results
            .iter()
            .chain(result.verifier_results.iter())
            .filter(|verifier| !verifier.is_ok())
        {
            println!(
                "  verifier `{}` -> {}{}",
                verifier.command,
                verifier.status,
                verifier
                    .exit_code
                    .map(|code| format!(" ({code})"))
                    .unwrap_or_default()
            );
        }
    }
}

fn sanitize_path_part(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
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

    #[test]
    fn copies_fixture_into_isolated_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("benchmarks/fixtures/simple");
        std::fs::create_dir_all(&fixture).unwrap();
        std::fs::write(fixture.join("file.txt"), "before\n").unwrap();
        let case = BenchCase {
            id: "simple".to_string(),
            fixture: PathBuf::from("benchmarks/fixtures/simple"),
            task: "Edit file".to_string(),
            verifiers: vec![navi_core::VerifierSpec {
                verifier_type: "command".to_string(),
                command: "test -f file.txt".to_string(),
                cwd: None,
                timeout_ms: None,
                required: true,
            }],
            ..BenchCase::default()
        };

        let workspace = prepare_workspace(&case, dir.path(), false).unwrap();

        assert!(workspace.path().join("file.txt").exists());
        assert!(workspace.path().join(".git").exists());
        assert_ne!(workspace.path(), fixture);
    }

    #[test]
    fn snapshot_diff_counts_changed_files_and_line_delta() {
        let mut before = BTreeMap::new();
        before.insert(PathBuf::from("a.txt"), "one\n".to_string());
        let mut after = BTreeMap::new();
        after.insert(PathBuf::from("a.txt"), "one\ntwo\n".to_string());
        after.insert(PathBuf::from("b.txt"), "new\n".to_string());

        let diff = diff_snapshots(&before, &after);

        assert_eq!(diff.files_changed, 2);
        assert_eq!(diff.lines_added, 2);
        assert_eq!(diff.lines_removed, 0);
    }

    #[test]
    fn metrics_from_events_counts_tokens_and_tools() {
        let events = vec![
            RuntimeEvent::new(RuntimeEventKind::TurnStarted {
                turn_id: "t1".to_string(),
            }),
            RuntimeEvent::new(RuntimeEventKind::ToolRequested(navi_core::ToolInvocation {
                id: "one".to_string(),
                tool_name: "bash".to_string(),
                input: serde_json::json!({"command": "cargo test"}),
            })),
            RuntimeEvent::new(RuntimeEventKind::ToolCompleted(navi_core::ToolResult {
                invocation_id: "one".to_string(),
                ok: false,
                output: serde_json::json!({
                    "status": 101,
                    "stdout": "test failed",
                    "stderr": "",
                }),
            })),
            RuntimeEvent::new(RuntimeEventKind::ToolRequested(navi_core::ToolInvocation {
                id: "two".to_string(),
                tool_name: "apply_patch".to_string(),
                input: serde_json::json!({"patch": "*** Begin Patch\n*** End Patch"}),
            })),
            RuntimeEvent::new(RuntimeEventKind::ToolCompleted(navi_core::ToolResult {
                invocation_id: "two".to_string(),
                ok: false,
                output: serde_json::json!({
                    "error_code": "verification_failed",
                    "error": "patch failed",
                }),
            })),
            RuntimeEvent::new(RuntimeEventKind::TokensUpdated {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: 2,
                cache_read_tokens: 7,
            }),
        ];

        let metrics = metrics_from_events(&events);

        assert_eq!(metrics.turn_count, 1);
        assert_eq!(metrics.tool_calls, 2);
        assert_eq!(metrics.failed_tool_calls, 1);
        assert_eq!(metrics.input_tokens, 10);
        assert_eq!(metrics.output_tokens, 5);
        assert_eq!(metrics.total_tokens, 15);
        assert_eq!(metrics.cache_read_tokens, 7);
        assert_eq!(metrics.cache_write_tokens, 2);
    }

    #[test]
    fn benchmark_limits_are_not_applied_when_omitted() {
        let mut limits = BenchLimitState::new(None, None);

        for idx in 0..5 {
            limits
                .observe(&RuntimeEvent::new(RuntimeEventKind::TurnStarted {
                    turn_id: format!("t{idx}"),
                }))
                .unwrap();
        }

        assert_eq!(limits.turns, 5);
    }

    #[test]
    fn benchmark_limits_fail_when_explicitly_exceeded() {
        let mut limits = BenchLimitState::new(Some(1), Some(1));

        limits
            .observe(&RuntimeEvent::new(RuntimeEventKind::TurnStarted {
                turn_id: "t1".to_string(),
            }))
            .unwrap();
        let turn_error = limits
            .observe(&RuntimeEvent::new(RuntimeEventKind::TurnStarted {
                turn_id: "t2".to_string(),
            }))
            .unwrap_err();

        assert!(turn_error.to_string().contains("max_turns"));

        let mut limits = BenchLimitState::new(Some(10), Some(1));
        limits
            .observe(&RuntimeEvent::new(RuntimeEventKind::ToolRequested(
                navi_core::ToolInvocation {
                    id: "one".to_string(),
                    tool_name: "read_file".to_string(),
                    input: serde_json::json!({"path": "src/lib.rs"}),
                },
            )))
            .unwrap();
        let tool_error = limits
            .observe(&RuntimeEvent::new(RuntimeEventKind::ToolRequested(
                navi_core::ToolInvocation {
                    id: "two".to_string(),
                    tool_name: "read_file".to_string(),
                    input: serde_json::json!({"path": "src/main.rs"}),
                },
            )))
            .unwrap_err();

        assert!(tool_error.to_string().contains("max_tool_calls"));
    }

    #[test]
    fn benchmark_provider_model_overrides_are_used_unless_case_overrides() {
        let mut loaded = LoadedConfig::default();
        loaded.config.model.provider = "current-provider".to_string();
        loaded.config.model.name = "current-model".to_string();

        let case = BenchCase::default();
        let resolved = apply_agent_config(
            loaded.clone(),
            &case,
            Some("opencode"),
            Some("deepseek-v4-flash-free"),
        )
        .unwrap();

        assert_eq!(resolved.config.model.provider, "opencode");
        assert_eq!(resolved.config.model.name, "deepseek-v4-flash-free");

        let mut case = BenchCase::default();
        case.agent.provider = Some("anthropic".to_string());
        case.agent.model = Some("claude-sonnet-4-20250514".to_string());
        let resolved = apply_agent_config(
            loaded,
            &case,
            Some("opencode"),
            Some("deepseek-v4-flash-free"),
        )
        .unwrap();

        assert_eq!(resolved.config.model.provider, "anthropic");
        assert_eq!(resolved.config.model.name, "claude-sonnet-4-20250514");
    }
}
