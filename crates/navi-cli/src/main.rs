use anyhow::Result;
use clap::Parser;
use navi_core::{
    AgentRuntime, AgentRuntimeOptions, LoadedConfig, LoggingRuntimeConfig, ModelProvider,
    SecurityPolicy, SessionSnapshot, SessionStore, ToolExecutor, init_logging, log_path,
    resolve_provider_config,
};
use navi_openai::OpenAiProvider;
use navi_plugin_host::load_configured_plugins;
use navi_tui::TuiApp;
use std::path::PathBuf;
use std::sync::Arc;

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

async fn sync_models(mut loaded_config: LoadedConfig, cwd: &std::path::Path) -> Result<()> {
    let credential_store = navi_core::CredentialStore::new(loaded_config.data_dir.clone());
    let catalog = navi_core::provider_catalog(&loaded_config.config);
    let mut updated_any = false;

    for provider_config in catalog {
        if let Some(api_key) =
            credential_store.resolve_api_key(&provider_config.id, &provider_config.api_key_env)
        {
            println!("Syncing models for provider \"{}\"...", provider_config.id);
            tracing::info!(provider = %provider_config.id, "syncing provider models");

            match OpenAiProvider::from_provider_config_with_key(&provider_config, api_key) {
                Ok(provider) => match provider.list_models().await {
                    Ok(models) => {
                        if models.is_empty() {
                            println!(
                                "No models returned for provider \"{}\".",
                                provider_config.id
                            );
                        } else {
                            println!(
                                "Found {} models for provider \"{}\":",
                                models.len(),
                                provider_config.id
                            );
                            for m in &models {
                                println!("  - {}", m);
                            }
                            loaded_config
                                .config
                                .update_provider_models(&provider_config.id, &models);
                            updated_any = true;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(provider = %provider_config.id, error = %e, "failed to fetch provider models");
                        eprintln!(
                            "Failed to fetch models for provider \"{}\": {}",
                            provider_config.id, e
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(provider = %provider_config.id, error = %e, "failed to initialize provider");
                    eprintln!(
                        "Failed to initialize provider \"{}\": {}",
                        provider_config.id, e
                    );
                }
            }
        }
    }

    if updated_any {
        let saved_path = if let Some(_) = &loaded_config.project_config_path {
            navi_core::save_project_config(cwd, &loaded_config.config)?
        } else if let Some(global_path) = &loaded_config.global_config_path {
            navi_core::save_global_config(global_path, &loaded_config.config)?
        } else {
            anyhow::bail!("no config file path found to save");
        };
        println!(
            "Successfully saved updated models configuration to: {}",
            saved_path.display()
        );
    } else {
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

    let provider = model_provider_for_config(&loaded_config)?;
    let security_policy = SecurityPolicy::new(
        cwd.clone(),
        loaded_config.data_dir.clone(),
        loaded_config.config.security.clone(),
    )?;
    let mut tool_executor = ToolExecutor::new(security_policy.clone());
    let plugin_report = load_configured_plugins(
        &loaded_config.config.plugins,
        &security_policy,
        &mut tool_executor,
    );
    for warning in &plugin_report.warnings {
        tracing::warn!(warning = %warning, "plugin load warning");
        eprintln!("Plugin warning: {warning}");
    }
    let _loaded_plugins = plugin_report.loaded_plugins;

    let mut runtime = AgentRuntime::new(AgentRuntimeOptions {
        loaded_config: loaded_config.clone(),
        model_provider: provider,
        project_dir: cwd.clone(),
        tool_executor: Some(Arc::new(tool_executor)),
        agent_mode: None,
        context_packets: Vec::new(),
        event_tx: None,
    });
    tracing::info!(
        provider = %loaded_config.config.model.provider,
        model = %loaded_config.config.model.name,
        "submitting headless task"
    );
    let response = runtime.submit_task(task).await?;
    println!("{}", response.text);

    let store = SessionStore::with_redaction(
        loaded_config.data_dir,
        loaded_config.config.security.redact_secrets_in_sessions,
    );
    store.save(&SessionSnapshot {
        id: SessionStore::create_id(),
        title: None,
        project: cwd,
        created_at: navi_core::session::current_unix_timestamp(),
        updated_at: navi_core::session::current_unix_timestamp(),
        events: runtime.events().to_vec(),
        memory: None,
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
