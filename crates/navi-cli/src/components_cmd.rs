//! `navi components` — inspect, swap and audit composed components.
//!
//! The kernel lives in the running supervisor process; this CLI is a *client*.
//! It reads the live status snapshot the supervisor publishes, or the installed
//! manifests when no composed session is running.

use std::path::{Path, PathBuf};

use anyhow::Result;
use navi_compose::{Compose, ComposeOptions, default_home, default_manifests};
use navi_core::LoadedConfig;

/// Sub-actions of `navi components`.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum ComponentsAction {
    /// List installed component definitions
    List,
    /// Live status of a running composed session (generations, waiting reasons, TTY)
    Status,
    /// Reload (swap) a component generation in the running session
    Reload {
        /// Component id, e.g. `navi.ui.plain`
        id: String,
    },
    /// Install a third-party component from a manifest file (TOML or JSON)
    Install {
        /// Path to the component manifest
        path: PathBuf,
        /// Activate it immediately (default: install only)
        #[arg(long, default_value_t = false)]
        activate: bool,
    },
    /// Print the kernel manifest JSON for an installed component
    Manifest {
        /// Component id
        id: String,
    },
    /// Show the tail of the kernel journal
    Journal {
        /// Number of entries
        #[arg(long, default_value_t = 20)]
        lines: usize,
    },
    /// Show the kernel inventory of a fresh host (dependencies + resources)
    Inventory,
}

/// Handles `navi components <action>`.
pub fn handle_components_command(
    action: ComponentsAction,
    loaded_config: &LoadedConfig,
    cwd: &Path,
) -> Result<()> {
    let home = default_home(&loaded_config.data_dir);
    match action {
        ComponentsAction::List => {
            let compose = Compose::new(ComposeOptions {
                home: home.clone(),
                project_dir: cwd.to_path_buf(),
                loaded_config: loaded_config.clone(),
                dry_run: true,
                terminal_effects: false,
                process_components: false,
            })?;
            compose.install_definitions(&default_manifests())?;
            compose.load_installed()?;
            println!("{:<28} {:<10} {:<8} {:<8} {}", "id", "kind", "version", "tier", "capabilities");
            for manifest in compose.manifests() {
                println!(
                    "{:<28} {:<10} {:<8} {:<8} {}",
                    manifest.id,
                    format!("{:?}", manifest.kind).to_lowercase(),
                    manifest.version,
                    manifest.tier,
                    manifest.capabilities.join(", ")
                );
            }
            println!();
            println!("state dir: {}", home.display());
            println!("install a third-party component with: navi components install <manifest.toml>");
            Ok(())
        }
        ComponentsAction::Status => {
            if let Some(snapshot) = Compose::read_status_snapshot(&home) {
                print_live_status(&snapshot);
                return Ok(());
            }
            let compose = Compose::new(ComposeOptions {
                home: home.clone(),
                project_dir: cwd.to_path_buf(),
                loaded_config: loaded_config.clone(),
                dry_run: true,
                terminal_effects: false,
                process_components: false,
            })?;
            compose.install_definitions(&default_manifests())?;
            compose.load_installed()?;
            println!(
                "no composed session is running (no status snapshot at {})",
                home.join("status.json").display()
            );
            println!("installed components:");
            for manifest in compose.manifests() {
                println!(
                    "  {:<28} {:<10} {:<8} {}",
                    manifest.id,
                    format!("{:?}", manifest.kind).to_lowercase(),
                    manifest.tier,
                    manifest.capabilities.join(", ")
                );
            }
            println!();
            println!("start one with: navi --ui {}", default_manifests()[1].id);
            Ok(())
        }
        ComponentsAction::Reload { id } => {
            let compose = Compose::new(ComposeOptions {
                home: home.clone(),
                project_dir: cwd.to_path_buf(),
                loaded_config: loaded_config.clone(),
                dry_run: false,
                terminal_effects: false,
                process_components: false,
            })?;
            compose.install_definitions(&default_manifests())?;
            compose.load_installed()?;
            compose.request_reload(&id)?;
            println!("reload requested for `{id}`");
            println!(
                "the running supervisor applies it within ~50ms (request file: {})",
                compose.reload_request_path().display()
            );
            Ok(())
        }
        ComponentsAction::Install { path, activate } => {
            let text = std::fs::read_to_string(&path)
                .map_err(|err| anyhow::anyhow!("cannot read {}: {err}", path.display()))?;
            let manifest = match path.extension().and_then(|ext| ext.to_str()) {
                Some("json") => navi_compose::ComponentManifest::from_json(&text),
                _ => navi_compose::ComponentManifest::from_toml(&text),
            }
            .map_err(|err| anyhow::anyhow!("invalid manifest {}: {err}", path.display()))?;
            let id = manifest.id.clone();
            let is_process = !manifest.execution.is_inproc();
            let compose = Compose::new(ComposeOptions {
                home: home.clone(),
                project_dir: cwd.to_path_buf(),
                loaded_config: loaded_config.clone(),
                dry_run: false,
                terminal_effects: false,
                process_components: is_process,
            })?;
            compose.install_definitions(&default_manifests())?;
            compose.load_installed()?;
            compose.install_definition(manifest)?;
            println!("installed `{id}` ({} component)", if is_process { "process" } else { "in-process" });
            if activate {
                let outcome = compose.activate(&id)?;
                println!(
                    "activated `{id}`: state={} generation={:?} reason={:?}",
                    outcome.state, outcome.generation, outcome.waiting_reason
                );
                // Activation may have spawned a child process (process tier);
                // this CLI is short-lived, so it must not leave it behind.
                compose.shutdown();
            } else {
                println!("activate it with: navi components reload {id}");
            }
            Ok(())
        }
        ComponentsAction::Manifest { id } => {
            let compose = Compose::new(ComposeOptions {
                home: home.clone(),
                project_dir: cwd.to_path_buf(),
                loaded_config: loaded_config.clone(),
                dry_run: true,
                terminal_effects: false,
                process_components: false,
            })?;
            compose.install_definitions(&default_manifests())?;
            compose.load_installed()?;
            let manifest = compose
                .manifests()
                .into_iter()
                .find(|manifest| manifest.id == id)
                .ok_or_else(|| anyhow::anyhow!("unknown component `{id}`"))?;
            println!("{}", serde_json::to_string_pretty(&manifest.to_kernel_json())?);
            Ok(())
        }
        ComponentsAction::Journal { lines } => {
            let path = home.join("journal.jsonl");
            let content = std::fs::read_to_string(&path)
                .map_err(|err| anyhow::anyhow!("no journal at {}: {err}", path.display()))?;
            let all: Vec<&str> = content.lines().filter(|line| !line.is_empty()).collect();
            let start = all.len().saturating_sub(lines);
            for line in &all[start..] {
                println!("{line}");
            }
            Ok(())
        }
        ComponentsAction::Inventory => {
            let compose = Compose::new(ComposeOptions {
                home: home.clone(),
                project_dir: cwd.to_path_buf(),
                loaded_config: loaded_config.clone(),
                dry_run: true,
                terminal_effects: false,
                process_components: false,
            })?;
            compose.install_definitions(&default_manifests())?;
            compose.load_installed()?;
            for manifest in default_manifests() {
                compose.activate(&manifest.id)?;
            }
            println!("{}", serde_json::to_string_pretty(&compose.inventory())?);
            Ok(())
        }
    }
}

fn print_live_status(snapshot: &serde_json::Value) {
    let epoch = snapshot.get("epoch").and_then(|v| v.as_u64()).unwrap_or(0);
    let pid = snapshot.get("pid").and_then(|v| v.as_u64()).unwrap_or(0);
    let session = snapshot
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("<none>");
    println!("live composed session (pid {pid}, kernel epoch {epoch}, session {session})");
    println!();
    println!(
        "{:<24} {:<14} {:<6} {:<8} {:<6} {:<9} {}",
        "id", "state", "gen", "instance", "tty", "restarts", "waiting"
    );
    if let Some(components) = snapshot.get("components").and_then(|v| v.as_array()) {
        for component in components {
            let id = component.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let state = component
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let generation = component
                .get("generation")
                .and_then(|v| v.as_u64())
                .map(|g| g.to_string())
                .unwrap_or_else(|| "-".to_string());
            let instance = component
                .get("instance")
                .and_then(|v| v.as_u64())
                .map(|i| i.to_string())
                .unwrap_or_else(|| "-".to_string());
            let tty = component
                .get("tty")
                .and_then(|v| v.as_bool())
                .map(|tty| if tty { "yes" } else { "no" })
                .unwrap_or("-");
            let waiting = component
                .get("waiting_reason")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let restarts = component
                .get("restarts")
                .and_then(|v| v.as_u64())
                .map(|count| {
                    if count == 0 {
                        "-".to_string()
                    } else {
                        count.to_string()
                    }
                })
                .unwrap_or_else(|| "-".to_string());
            println!(
                "{id:<24} {state:<14} {generation:<6} {instance:<8} {tty:<6} {restarts:<9} {waiting}"
            );
        }
    }
    // Supervision decisions are the interesting part after a failure.
    if let Some(components) = snapshot.get("components").and_then(|v| v.as_array()) {
        for component in components {
            let Some(decision) = component.get("last_restart").and_then(|v| v.as_str()) else {
                continue;
            };
            let id = component.get("id").and_then(|v| v.as_str()).unwrap_or("");
            println!("supervision: {id} → {decision}");
        }
    }
    println!();
    println!("swap a frontend with: navi components reload navi.ui.<name>");
}
