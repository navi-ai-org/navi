use anyhow::Result;
use clap::Parser;
use navi_core::{
    AgentRuntime, AgentRuntimeOptions, LoadedConfig, ModelProvider, SessionSnapshot, SessionStore,
    resolve_provider_config,
};
use navi_openai::OpenAiProvider;
use navi_tui::TuiApp;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Parser)]
#[command(name = "navi")]
#[command(about = "An opinionated, customizable TUI code agent")]
struct Cli {
    #[arg(long)]
    print_config: bool,

    #[arg(long)]
    print_providers: bool,

    #[arg(long)]
    no_tui: bool,

    #[arg(value_name = "TASK")]
    task: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;
    let loaded_config = navi_core::NaviConfig::load(&cwd)?;

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

    let task = normalize_task(cli.task);
    if cli.no_tui {
        run_headless(loaded_config, cwd, task).await?;
        return Ok(());
    }

    navi_tui::run(TuiApp::new(loaded_config, cwd.clone(), task))?;
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

    let provider = model_provider_for_config(&loaded_config)?;
    let mut runtime = AgentRuntime::new(AgentRuntimeOptions {
        loaded_config: loaded_config.clone(),
        model_provider: provider,
    });
    let response = runtime.submit_task(task).await?;
    println!("{}", response.text);

    let store = SessionStore::new(loaded_config.data_dir);
    store.save(&SessionSnapshot {
        id: SessionStore::create_id(),
        project: cwd,
        events: runtime.events().to_vec(),
    })?;

    Ok(())
}

fn model_provider_for_config(loaded_config: &LoadedConfig) -> Result<Arc<dyn ModelProvider>> {
    let provider_config =
        resolve_provider_config(&loaded_config.config, &loaded_config.config.model.provider)
            .ok_or_else(|| {
                anyhow::anyhow!("unknown provider {}", loaded_config.config.model.provider)
            })?;

    Ok(Arc::new(OpenAiProvider::from_provider_config(
        &provider_config,
    )?))
}

fn normalize_task(parts: Vec<String>) -> Option<String> {
    let task = parts.join(" ");
    let task = task.trim();
    (!task.is_empty()).then(|| task.to_string())
}
