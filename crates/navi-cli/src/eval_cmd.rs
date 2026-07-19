use anyhow::Result;
use navi_core::{
    EvalRun, EvalRunner, EvalSuite, ReplayGateConfig, TraceStore, evaluate_replay_gate,
    evaluate_superiority_gate, export_jsonl, trace_to_dataset_rows, traces_to_eval_candidates,
    unsafe_guarded_auto_approval_count,
};
use std::path::PathBuf;

use crate::EvalAction;

pub async fn handle_eval_command(action: EvalAction, cwd: PathBuf) -> Result<()> {
    match action {
        EvalAction::Run {
            path,
            project,
            json,
        } => run_eval(path, project.unwrap_or(cwd), json).await,
        EvalAction::GenerateFromTraces {
            data_dir,
            output_dir,
            dataset_jsonl,
        } => generate_from_traces(data_dir, output_dir, dataset_jsonl),
        EvalAction::Gate {
            candidate,
            baseline,
            min_success_rate,
            max_success_drop,
            unsafe_guarded_auto_approvals,
            trace_data_dir,
            superiority,
            json,
        } => gate_eval(
            candidate,
            baseline,
            min_success_rate,
            max_success_drop,
            unsafe_guarded_auto_approvals,
            trace_data_dir,
            superiority,
            json,
        ),
    }
}

async fn run_eval(path: PathBuf, project_root: PathBuf, json: bool) -> Result<()> {
    let suite = EvalSuite::load(&path)?;
    let run = EvalRunner::run_suite(suite, &project_root).await;

    if json {
        println!("{}", serde_json::to_string_pretty(&run)?);
        return Ok(());
    }

    println!(
        "Eval suite `{}`: {}/{} passed ({:.1}%)",
        run.suite_name,
        run.metrics.passed_cases,
        run.metrics.total_cases,
        run.metrics.verified_success_rate * 100.0
    );
    match run.metrics.verified_success_per_1k_tokens {
        Some(value) => println!("verified_success_per_1k_tokens: {value:.3}"),
        None => println!("verified_success_per_1k_tokens: n/a (no token usage recorded)"),
    }

    for result in &run.results {
        let status = if result.passed { "PASS" } else { "FAIL" };
        println!("[{status}] {} - {}", result.case_id, result.title);
        if !result.passed {
            for verifier in &result.setup_results {
                if !verifier.is_ok() {
                    println!(
                        "  setup `{}` -> {}{}",
                        verifier.command,
                        verifier.status,
                        verifier
                            .exit_code
                            .map(|code| format!(" ({code})"))
                            .unwrap_or_default()
                    );
                }
            }
            for verifier in &result.verifier_results {
                if !verifier.is_ok() {
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
    }

    if run.metrics.failed_cases > 0 {
        anyhow::bail!("{} eval case(s) failed", run.metrics.failed_cases);
    }

    Ok(())
}

fn generate_from_traces(
    data_dir: PathBuf,
    output_dir: PathBuf,
    dataset_jsonl: Option<PathBuf>,
) -> Result<()> {
    let store = TraceStore::new(&data_dir);
    let mut traces = Vec::new();
    for session_id in store.list_sessions() {
        traces.extend(store.load_session_traces(&session_id));
    }
    std::fs::create_dir_all(&output_dir)?;

    let evals = traces_to_eval_candidates(&traces);
    for case in &evals {
        let path = output_dir.join(format!("{}.toml", case.id));
        std::fs::write(&path, toml::to_string_pretty(case)?)?;
    }

    let rows = traces
        .iter()
        .flat_map(trace_to_dataset_rows)
        .collect::<Vec<_>>();
    if let Some(path) = dataset_jsonl {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, export_jsonl(&rows)?)?;
    }

    println!(
        "generated {} eval candidate(s) and {} dataset row(s)",
        evals.len(),
        rows.len()
    );
    Ok(())
}

fn gate_eval(
    candidate_path: PathBuf,
    baseline_path: Option<PathBuf>,
    min_success_rate: f64,
    max_success_drop: f64,
    unsafe_guarded_auto_approvals: u64,
    trace_data_dir: Option<PathBuf>,
    superiority: bool,
    json_output: bool,
) -> Result<()> {
    let candidate: EvalRun = serde_json::from_str(&std::fs::read_to_string(&candidate_path)?)?;
    let baseline = match baseline_path.as_ref() {
        Some(path) => Some(serde_json::from_str::<EvalRun>(&std::fs::read_to_string(
            path,
        )?)?),
        None => None,
    };
    let trace_unsafe_count = trace_data_dir
        .as_ref()
        .map(|data_dir| {
            let store = TraceStore::new(data_dir);
            let mut traces = Vec::new();
            for session_id in store.list_sessions() {
                traces.extend(store.load_session_traces(&session_id));
            }
            unsafe_guarded_auto_approval_count(&traces)
        })
        .unwrap_or(0);
    let total_unsafe_guarded_auto_approvals =
        unsafe_guarded_auto_approvals.saturating_add(trace_unsafe_count);
    let replay = evaluate_replay_gate(
        baseline.as_ref(),
        &candidate,
        total_unsafe_guarded_auto_approvals,
        &ReplayGateConfig {
            min_verified_success_rate: min_success_rate,
            max_success_rate_drop: max_success_drop,
            require_zero_unsafe_guarded_auto_approvals: true,
        },
    );
    let superiority_report = if superiority {
        Some(evaluate_superiority_gate(
            baseline
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("--superiority requires --baseline"))?,
            &candidate,
        ))
    } else {
        None
    };
    let passed = replay.passed
        && superiority_report
            .as_ref()
            .map(|report| report.passed)
            .unwrap_or(true);

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "passed": passed,
                "replay": replay,
                "superiority": superiority_report,
                "unsafe_guarded_auto_approvals": total_unsafe_guarded_auto_approvals,
            }))?
        );
    } else {
        println!("eval gate: {}", if passed { "PASS" } else { "FAIL" });
        println!("unsafe_guarded_auto_approvals: {total_unsafe_guarded_auto_approvals}");
        for failure in &replay.failures {
            println!("replay: {failure}");
        }
        if let Some(report) = &superiority_report {
            for failure in &report.failures {
                println!("superiority: {failure}");
            }
        }
    }

    if !passed {
        anyhow::bail!("eval gate failed");
    }
    Ok(())
}
