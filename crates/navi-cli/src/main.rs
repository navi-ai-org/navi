use anyhow::Result;
use clap::{Parser, Subcommand};
use navi_core::{LoadedConfig, LoggingRuntimeConfig, init_logging, log_path};
use navi_sdk::{NaviConfigSaveTarget, NaviEngineBuilder, NaviSessionRequest, NaviTurnRequest};
use navi_tui::TuiApp;
use std::path::PathBuf;

mod bench_cmd;
mod browser_cmd;
mod eval_cmd;
mod mcp_cmd;
mod memory_cmd;
mod plugin_cmd;
mod registry_cmd;
mod server_cmd;
mod skill_cmd;
mod voice_cmd;

#[derive(Debug, Parser)]
#[command(name = "navi")]
#[command(about = "An opinionated, customizable TUI code agent")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

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

    /// Auto-approve all tool calls (YOLO mode)
    #[arg(long)]
    yolo: bool,

    /// Auto-approve reads, edits, and commands except guarded commands (e.g. git)
    #[arg(long)]
    auto: bool,

    /// Auto-approve reads and edits, prompt for commands
    #[arg(long)]
    accept_edits: bool,

    /// Require approval for every tool call
    #[arg(long)]
    restricted: bool,

    /// Activate skill(s) for this session (repeatable). Enables skills discovery when set.
    #[arg(long = "skill", value_name = "ID", action = clap::ArgAction::Append)]
    skill: Vec<String>,

    #[arg(value_name = "TASK")]
    task: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Manage WASM plugins
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
    /// Manage MCP servers
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
    /// Manage state-continuity memory
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
    },
    /// Local voice / dictation models and mic diagnostics
    Voice {
        #[command(subcommand)]
        action: VoiceAction,
    },
    /// Run local harness eval suites
    Eval {
        #[command(subcommand)]
        action: EvalAction,
    },
    /// Run agentic benchmark suites
    Bench {
        #[command(subcommand)]
        action: BenchAction,
    },
    /// Run interactive setup wizard (provider login, agent-configured interview)
    Setup,
    /// Sync and list providers from the registry database
    Registry {
        #[command(subcommand)]
        action: RegistryAction,
    },
    /// Headless browser tool backend (CloakBrowser / Chrome / CDP)
    Browser {
        #[command(subcommand)]
        action: BrowserAction,
    },
    /// Remote HTTP/WebSocket server (`navi-server`) as a systemd service
    Server {
        #[command(subcommand)]
        action: ServerAction,
    },
    /// Manage skills (install into skills.sqlite, list)
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum SkillAction {
    /// Install a skill from a markdown (.md) or TOML (.toml) file into skills.sqlite
    Install {
        /// Path to skill markdown (.md) or skill.toml
        path: PathBuf,
        /// Optional skill id override
        #[arg(long)]
        id: Option<String>,
        /// user (default) or project scope
        #[arg(long, default_value = "user")]
        scope: String,
    },
    /// List built-in and store skills
    List,
}

#[derive(Debug, Subcommand)]
pub enum ServerAction {
    /// Install a systemd unit for navi-server (user unit by default)
    Install {
        /// Listen port
        #[arg(long, default_value = "9800")]
        port: u16,
        /// Bind address (`0.0.0.0` for LAN/Tailscale)
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
        /// Optional default workspace on the host.
        ///
        /// When omitted, uses `$HOME` (agent home mode). Mobile/clients can still
        /// open any project per session via `projectDir` on `POST /sessions`.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Shared secret (or set NAVI_SERVER_SECRET). Generated if omitted.
        #[arg(long)]
        secret: Option<String>,
        /// Install system-wide unit under /etc/systemd/system (needs root)
        #[arg(long)]
        system: bool,
        /// Overwrite existing unit/env
        #[arg(long)]
        force: bool,
    },
    /// Start the installed navi-server service
    Start {
        #[arg(long)]
        system: bool,
    },
    /// Stop the service
    Stop {
        #[arg(long)]
        system: bool,
    },
    /// Restart the service
    Restart {
        #[arg(long)]
        system: bool,
    },
    /// Show install + systemd status
    Status {
        #[arg(long)]
        system: bool,
    },
    /// Remove the systemd unit (env file kept)
    Uninstall {
        #[arg(long)]
        system: bool,
    },
    /// Show journal logs for the service
    Logs {
        #[arg(long)]
        system: bool,
        /// Follow log output
        #[arg(long, short = 'f')]
        follow: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum BrowserAction {
    /// Show browser backend status
    Status,
    /// Diagnose binary discovery and print install hints (JSON)
    Doctor,
    /// Print install instructions for CloakBrowser / Chrome / cloakserve
    Install,
}

#[derive(Debug, Subcommand)]
pub enum BenchAction {
    /// Run an agentic benchmark suite or single benchmark case
    Run {
        /// Path to a benchmark case file or directory of .toml/.json cases
        path: PathBuf,
        /// Project root used to resolve relative fixtures
        #[arg(long)]
        project: Option<PathBuf>,
        /// Write the full BenchRun JSON to this path
        #[arg(long)]
        output: Option<PathBuf>,
        /// Print the full BenchRun JSON
        #[arg(long)]
        json: bool,
        /// Provider override for every benchmark case unless the case sets its own provider
        #[arg(long)]
        provider: Option<String>,
        /// Model override for every benchmark case unless the case sets its own model
        #[arg(long)]
        model: Option<String>,
        /// Automatically approve tool approval requests during the benchmark run
        #[arg(long)]
        auto_approve: bool,
        /// Keep temporary workspaces after each case for inspection
        #[arg(long)]
        keep_workspaces: bool,
    },
    /// Compare a candidate benchmark run against an optional baseline
    Compare {
        /// Candidate BenchRun JSON path
        candidate: PathBuf,
        /// Baseline BenchRun JSON path
        #[arg(long)]
        baseline: Option<PathBuf>,
        /// Minimum verified success rate for the candidate
        #[arg(long, default_value_t = 1.0)]
        min_success_rate: f64,
        /// Maximum allowed success-rate drop from baseline
        #[arg(long, default_value_t = 0.0)]
        max_success_drop: f64,
        /// Require candidate tokens_per_success to be no worse than baseline
        #[arg(long)]
        require_token_improvement: bool,
        /// Require candidate tool_calls_per_success to be no worse than baseline
        #[arg(long)]
        require_tool_call_improvement: bool,
        /// Print JSON report
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum EvalAction {
    /// Run a verifier-replay eval suite or single eval case
    Run {
        /// Path to an eval case file or directory of .toml/.json cases
        path: PathBuf,
        /// Project root where verifier commands should run
        #[arg(long)]
        project: Option<PathBuf>,
        /// Print the full EvalRun JSON
        #[arg(long)]
        json: bool,
    },
    /// Generate eval candidates and dataset JSONL from stored traces
    GenerateFromTraces {
        /// NAVI data directory containing traces/
        data_dir: PathBuf,
        /// Directory where generated EvalCase TOML files should be written
        #[arg(long)]
        output_dir: PathBuf,
        /// Optional JSONL dataset output path
        #[arg(long)]
        dataset_jsonl: Option<PathBuf>,
    },
    /// Evaluate replay/superiority gates from EvalRun JSON files
    Gate {
        /// Candidate EvalRun JSON path
        candidate: PathBuf,
        /// Baseline EvalRun JSON path
        #[arg(long)]
        baseline: Option<PathBuf>,
        /// Minimum verified success rate for replay gate
        #[arg(long, default_value_t = 1.0)]
        min_success_rate: f64,
        /// Maximum allowed success-rate drop from baseline
        #[arg(long, default_value_t = 0.0)]
        max_success_drop: f64,
        /// Unsafe guarded effects auto-approved count
        #[arg(long, default_value_t = 0)]
        unsafe_guarded_auto_approvals: u64,
        /// Optional NAVI data directory; unsafe guarded auto-approvals are derived from traces/
        #[arg(long)]
        trace_data_dir: Option<PathBuf>,
        /// Also require verified_success_per_1k_tokens improvement over baseline
        #[arg(long)]
        superiority: bool,
        /// Print JSON report
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum MemoryAction {
    /// Show current memory system status
    Status,
    /// Manually run checkpoint writer
    Checkpoint,
    /// Print the context that would be injected on rebuild
    RebuildPreview,
    /// Search raw history
    History {
        /// Search query
        query: String,
        /// Optional limit
        #[arg(long)]
        limit: Option<i64>,
        /// Filter by session ID
        #[arg(long)]
        session_id: Option<String>,
    },
    /// Run dream maintenance
    Dream {
        /// Apply the dream output to active memory after writing the review copy
        #[arg(long)]
        apply: bool,
        /// Number of recent sessions to mine, capped at 100
        #[arg(long, default_value_t = 10)]
        sessions: usize,
        /// High-level synthesis guidance for the dream
        #[arg(long)]
        instructions: Option<String>,
    },
    /// Run distill maintenance
    Distill,
    /// Initialize or repair the auto-memory database and download embedding model
    Init {
        /// Download the embedding model for semantic search (Qwen3-Embedding-0.6B GGUF)
        #[arg(long)]
        embeddings: bool,
        /// Force re-download even if the model already exists
        #[arg(long)]
        force: bool,
    },
    /// List all stored memories
    List {
        /// Filter by status: active, needs_review, obsolete
        #[arg(long)]
        status: Option<String>,
        /// Max results
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Search memories by text query
    Search {
        /// Search query
        query: String,
        /// Max results
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Validate files, paths, permissions, SQLite schema, and config
    Doctor,
}

#[derive(Debug, Subcommand)]
pub enum VoiceAction {
    /// Show voice config and install status (local + remote)
    Status,
    /// List remote transcription providers from the registry catalog
    Providers,
    /// Download a local ASR engine package into {data_dir}/voice/models/
    Init {
        /// Engine id: nemotron_streaming (default) | distil_whisper (later)
        #[arg(long, default_value = "nemotron_streaming")]
        engine: String,
        /// Force re-download even if already installed
        #[arg(long)]
        force: bool,
    },
    /// Check recorders, model files, checksums, or remote credentials
    Doctor,
    /// Transcribe a WAV file (local ONNX or remote provider from [voice])
    Transcribe {
        /// Path to a WAV file (any rate; resampled to 16 kHz mono for local / remote)
        path: String,
        /// Language prompt: auto | en-US | pt-BR | …
        #[arg(long, default_value = "auto")]
        language: String,
    },
}

#[derive(Debug, Subcommand)]
enum McpAction {
    /// List configured MCP servers, connection status, and tools
    List,
}

#[derive(Debug, Subcommand)]
enum RegistryAction {
    /// Force-sync the provider registry from the remote database
    Sync,
    /// List all providers and model counts from the local cache
    List,
}

#[derive(Debug, Subcommand)]
enum PluginAction {
    /// Install a plugin from a local directory (developer workflow)
    Install {
        /// Path to the plugin directory (containing plugin.toml and .wasm)
        path: PathBuf,
        /// Skip the approval prompt and install non-interactively
        #[arg(long)]
        yes: bool,
    },
    /// Install a plugin from the marketplace registry by id
    InstallMarketplace {
        /// Plugin id from catalog.json
        plugin_id: String,
        /// Skip the approval prompt and install non-interactively
        #[arg(long)]
        yes: bool,
    },
    /// Update an installed plugin from a local directory (developer workflow)
    Update {
        /// Path to the new plugin directory (containing plugin.toml and .wasm)
        path: PathBuf,
        /// Force the update even when the publisher changed
        #[arg(long)]
        force: bool,
    },
    /// Update an installed plugin from the marketplace registry
    UpdateMarketplace {
        /// Plugin id from catalog.json
        plugin_id: String,
        /// Force the update even when the publisher changed
        #[arg(long)]
        force: bool,
    },
    /// Search the marketplace catalog
    Search {
        /// Optional search query (id, name, description)
        query: Option<String>,
    },
    /// List installed plugins
    List,
    /// Remove an installed plugin
    Remove {
        /// Plugin ID to remove
        plugin_id: String,
    },
    /// Show details of a plugin
    Info {
        /// Plugin ID or path
        plugin_id: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;
    let mut loaded_config = navi_core::NaviConfig::load(&cwd)?;
    if cli.debug_payloads {
        loaded_config.config.logging.include_payloads = true;
    }

    // Apply permission mode CLI flags
    if cli.yolo {
        loaded_config.config.security.permission_mode = navi_core::PermissionMode::Yolo;
    } else if cli.auto {
        loaded_config.config.security.permission_mode = navi_core::PermissionMode::Auto;
    } else if cli.accept_edits {
        loaded_config.config.security.permission_mode = navi_core::PermissionMode::AcceptEdits;
    } else if cli.restricted {
        loaded_config.config.security.permission_mode = navi_core::PermissionMode::Restricted;
    }

    // CLI --skill: enable discovery for this run and seed the session active set.
    // Runtime-only; does not persist config.
    let cli_skills = normalize_skill_ids(cli.skill);
    if !cli_skills.is_empty() {
        loaded_config.config.skills.enabled = true;
        loaded_config.config.skills.active = cli_skills.clone();
    }

    // Handle skill subcommand early
    if let Some(Commands::Skill { action }) = cli.command {
        return skill_cmd::handle_skill_command(action, &loaded_config, &cwd);
    }

    // Handle plugin subcommand early
    if let Some(Commands::Plugin { action }) = cli.command {
        return plugin_cmd::handle_plugin_command(action, &loaded_config, &cwd).await;
    }

    // Handle mcp subcommand early
    if let Some(Commands::Mcp { action }) = cli.command {
        return mcp_cmd::handle_mcp_command(action, &loaded_config).await;
    }

    // Handle memory subcommand early
    if let Some(Commands::Memory { action }) = cli.command {
        return memory_cmd::handle_memory_command(action, &loaded_config, &cwd).await;
    }

    // Handle voice subcommand early
    if let Some(Commands::Voice { action }) = cli.command {
        return voice_cmd::handle_voice_command(action, &loaded_config).await;
    }

    // Handle eval subcommand early
    if let Some(Commands::Eval { action }) = cli.command {
        return eval_cmd::handle_eval_command(action, cwd).await;
    }

    // Handle benchmark subcommand early
    if let Some(Commands::Bench { action }) = cli.command {
        return bench_cmd::handle_bench_command(action, loaded_config, cwd).await;
    }

    // Handle setup subcommand early — launch TUI in setup mode
    if let Some(Commands::Setup) = cli.command {
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
        tracing::info!("starting interactive setup wizard");
        navi_tui::run(TuiApp::setup_mode(loaded_config, cwd)?)?;
        return Ok(());
    }

    // Handle registry subcommand early
    if let Some(Commands::Registry { action }) = cli.command {
        return registry_cmd::handle_registry_command(action, &loaded_config, &cwd).await;
    }

    // Handle browser subcommand early
    if let Some(Commands::Browser { action }) = cli.command {
        return browser_cmd::handle_browser_command(action, &loaded_config).await;
    }

    // Handle remote server (systemd) subcommand early
    if let Some(Commands::Server { action }) = cli.command {
        return server_cmd::handle_server_command(action, &loaded_config, &cwd);
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
        init_registry_store(&loaded_config);
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
        run_headless(loaded_config, cwd, task, cli_skills).await?;
        return Ok(());
    }

    // Onboarding wizard removed — config v2 doesn't track onboarding_completed
    // The TUI will now start normally and prompt for setup if needed.
    // TUI seeds app.active_skills from loaded_config.config.skills.active (set above when --skill).

    tracing::info!(project = %cwd.display(), "starting TUI");
    navi_tui::run(TuiApp::new(loaded_config, cwd.clone(), task)?)?;
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
    active_skills: Vec<String>,
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
        skills = ?active_skills,
        "submitting headless task"
    );

    let session = engine
        .start_session(NaviSessionRequest {
            project_dir: Some(cwd),
            session_id: None,
            context_packets: Vec::new(),
            active_skills,
            initial_messages: Vec::new(),
            ..NaviSessionRequest::default()
        })
        .await?;

    let response = engine
        .send_turn(NaviTurnRequest {
            session_id: session.id.clone(),
            message: task,
            content_parts: Vec::new(),
            context_packets: Vec::new(),
            thinking: None,
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

/// Trim, drop empty entries, and dedupe skill ids while preserving order.
fn normalize_skill_ids(skills: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(skills.len());
    for skill in skills {
        let id = skill.trim();
        if id.is_empty() {
            continue;
        }
        if !out.iter().any(|existing: &String| existing == id) {
            out.push(id.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::normalize_skill_ids;

    #[test]
    fn normalize_skill_ids_trims_dedupes_and_drops_empty() {
        let input = vec![
            "  foo  ".into(),
            "".into(),
            "bar".into(),
            "   ".into(),
            "foo".into(),
            "baz".into(),
            "bar".into(),
        ];
        assert_eq!(
            normalize_skill_ids(input),
            vec!["foo".to_string(), "bar".to_string(), "baz".to_string()]
        );
    }

    #[test]
    fn normalize_skill_ids_empty_input() {
        assert!(normalize_skill_ids(Vec::new()).is_empty());
        assert!(normalize_skill_ids(vec!["".into(), "  ".into()]).is_empty());
    }
}

/// Initializes the thread-local registry store from the SQLite cache so that
/// `provider_catalog()` reads from the live database instead of falling back to
/// the embedded snapshot. Needed for CLI paths that don't construct a full
/// `NaviEngine` (e.g. `--print-providers`).
fn init_registry_store(loaded_config: &LoadedConfig) {
    if let Ok(store) = navi_core::registry::RegistryStore::open(&loaded_config.data_dir) {
        let store = std::sync::Arc::new(store);
        navi_core::registry::load_registry(&store);
        navi_core::set_registry_store(store);
    }
}
