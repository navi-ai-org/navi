use anyhow::Result;
use clap::Parser;
use navi_lite::{LiteConfig, LiteMission, LiteRuntime};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "navi-lite")]
#[command(about = "Run a sealed NAVI Lite health-check mission")]
struct Args {
    #[arg(long)]
    base_url: Option<String>,

    #[arg(long)]
    model: Option<String>,

    #[arg(long)]
    api_key: Option<String>,

    #[arg(long, default_value = ".")]
    project: PathBuf,

    #[arg(long)]
    task: Option<String>,

    #[arg(long)]
    json: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let mut config = LiteConfig::from_env(args.project)?;
    if let Some(base_url) = args.base_url {
        config.base_url = base_url;
    }
    if let Some(model) = args.model {
        config.model = model;
    }
    if let Some(api_key) = args.api_key {
        config.api_key = api_key;
    }

    let mut mission = LiteMission::health_check();
    if let Some(task) = args.task {
        mission.task = task;
    }

    let runtime = LiteRuntime::new(config)?;
    let result = runtime.run_mission(mission).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if let Some(report) = result.report {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}", result.raw_agent_text);
    }
    Ok(())
}
