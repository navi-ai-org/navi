use anyhow::Result;
use clap::Parser;
use navi_core::{LoadedConfig, LoggingRuntimeConfig, init_logging, log_path};
use navi_sdk::{NaviConfigSaveTarget, NaviEngineBuilder, NaviSessionRequest, NaviTurnRequest};
use navi_tui::TuiApp;
use std::path::PathBuf;

mod acp;

#[derive(Debug, Parser)]
#[command(name = "navi")]
#[command(about = "An opinionated, customizable TUI code agent")]
struct Cli {
    #[arg(long)]
    print_config: bool,

    #[arg(long)]
    print_providers: bool,

    #[arg(long)]
    sync_models: bool,

    #[arg(long)]
    print_log_path: bool,

    #[arg(long, value_name = "LEVEL")]
    log_level: Option<String>,

    #[arg(long)]
    no_log_file: bool,

    #[arg(long)]
    debug_payloads: bool,

    #[arg(long)]
    no_tui: bool,

    #[arg(long)]
    acp: bool,

    #[arg(value_name = "TASK")]
    task: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;
    let mut loaded_config = navi_core::NaviConfig::load(&cwd)?;
    if cli.debug_payloads {
        loaded_config.config.logging.include_payloads = true;
    }

    if cli.print_log_path {
        println!("{}", log_path(&loaded_config.data_dir).display());
        return Ok(());
    }

    if cli.print_config {
        println!("{}", serde_json::to_string_pretty(&loaded_config.config)?);
        return Ok(());
    }

    if cli.print_providers {
        println!(
            "{}",
            serde_json::to_string_pretty(&navi_core::provider_catalog(&loaded_config.config))?
        );
        return Ok(());
    }

    if cli.sync_models {
        tracing::info!("starting model sync");
        sync_models(loaded_config, &cwd).await?;
        return Ok(());
    }

    if cli.acp {
        if cli.no_tui {
            anyhow::bail!("--acp cannot be combined with --no-tui");
        }
        if !cli.task.is_empty() {
            anyhow::bail!("--acp runs as a stdio server and does not accept a task argument");
        }
        let _logging_guard = init_logging(
            &loaded_config.config.logging,
            &loaded_config.data_dir,
            LoggingRuntimeConfig {
                stdout_enabled: false,
                file_enabled: !cli.no_log_file,
                level: cli.log_level.clone(),
                include_payloads: cli.debug_payloads,
            },
        )?;
        tracing::info!(project = %cwd.display(), "starting ACP stdio server");
        acp::run_acp_server(loaded_config, cwd).await?;
        return Ok(());
    }

    let _logging_guard = init_logging(
        &loaded_config.config.logging,
        &loaded_config.data_dir,
        LoggingRuntimeConfig {
            stdout_enabled: cli.no_tui,
            file_enabled: !cli.no_log_file,
            level: cli.log_level.clone(),
            include_payloads: cli.debug_payloads,
        },
    )?;

    let task = normalize_task(cli.task);
    if cli.no_tui {
        tracing::info!(project = %cwd.display(), "starting headless run");
        run_headless(loaded_config, cwd, task).await?;
        return Ok(());
    }

    tracing::info!(project = %cwd.display(), "starting TUI");
    navi_tui::run(TuiApp::new(loaded_config, cwd.clone(), task))?;
    Ok(())
}

async fn sync_models(loaded_config: LoadedConfig, cwd: &std::path::Path) -> Result<()> {
    let engine = NaviEngineBuilder::from_project(cwd)
        .loaded_config(loaded_config)
        .build()?;

    let report = engine.sync_models(NaviConfigSaveTarget::Auto).await?;

    for provider in &report.updated {
        println!(
            "Synced {} models for provider \"{}\".",
            provider.model_count, provider.provider_id
        );
        tracing::info!(
            provider = %provider.provider_id,
            models = provider.model_count,
            "synced provider models"
        );
    }

    for skipped in &report.skipped {
        println!(
            "Skipped provider \"{}\": {}",
            skipped.provider_id, skipped.reason
        );
    }

    for failure in &report.failed {
        eprintln!(
            "Failed to sync provider \"{}\": {}",
            failure.provider_id, failure.message
        );
        tracing::warn!(
            provider = %failure.provider_id,
            error = %failure.message,
            "failed to sync provider models"
        );
    }

    if let Some(saved_path) = &report.saved_to {
        println!(
            "Saved updated models configuration to: {}",
            saved_path.display()
        );
    } else if report.updated.is_empty() {
        println!("No models were updated.");
    }

    Ok(())
}

async fn run_headless(
    loaded_config: LoadedConfig,
    cwd: PathBuf,
    task: Option<String>,
) -> Result<()> {
    let Some(task) = task else {
        anyhow::bail!("headless mode requires a task");
    };

    let engine = NaviEngineBuilder::from_project(cwd.clone())
        .loaded_config(loaded_config.clone())
        .build()?;

    tracing::info!(
        provider = %loaded_config.config.model.provider,
        model = %loaded_config.config.model.name,
        "submitting headless task"
    );

    let session = engine
        .start_session(NaviSessionRequest {
            project_dir: Some(cwd),
            session_id: None,
            agent_mode: None,
            context_packets: Vec::new(),
            active_skills: Vec::new(),
            initial_messages: Vec::new(),
        })
        .await?;

    let response = engine
        .send_turn(NaviTurnRequest {
            session_id: session.id.clone(),
            message: task,
            context_packets: Vec::new(),
        })
        .await?;
    println!("{}", response.text);
    engine.snapshot_session(&session.id).await?;

    Ok(())
}

fn normalize_task(parts: Vec<String>) -> Option<String> {
    let task = parts.join(" ");
    let task = task.trim();
    (!task.is_empty()).then(|| task.to_string())
}
