//! `navi components` — inspect, swap and audit composed components.
//!
//! The kernel lives in the running supervisor process; this CLI is a *client*.
//! It reads the live status snapshot the supervisor publishes, or the installed
//! manifests when no composed session is running.

use std::path::Path;

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
            println!("{:<24} {:<10} {:<12} {}", "id", "kind", "version", "capabilities");
            for manifest in default_manifests() {
                println!(
                    "{:<24} {:<10} {:<12} {}",
                    manifest.id,
                    format!("{:?}", manifest.kind).to_lowercase(),
                    manifest.version,
                    manifest.capabilities.join(", ")
                );
            }
            println!();
            println!("state dir: {}", home.display());
            Ok(())
        }
        ComponentsAction::Status => {
            if let Some(snapshot) = Compose::read_status_snapshot(&home) {
                print_live_status(&snapshot);
                return Ok(());
            }
            println!(
                "no composed session is running (no status snapshot at {})",
                home.join("status.json").display()
            );
            println!("installed definitions:");
            for manifest in default_manifests() {
                println!(
                    "  {:<24} {:<10} {}",
                    manifest.id,
                    format!("{:?}", manifest.kind).to_lowercase(),
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
            })?;
            compose.install_definitions(&default_manifests())?;
            compose.request_reload(&id)?;
            println!("reload requested for `{id}`");
            println!(
                "the running supervisor applies it within ~50ms (request file: {})",
                compose.reload_request_path().display()
            );
            Ok(())
        }
        ComponentsAction::Manifest { id } => {
            let compose = Compose::new(ComposeOptions {
                home: home.clone(),
                project_dir: cwd.to_path_buf(),
                loaded_config: loaded_config.clone(),
                dry_run: true,
                terminal_effects: false,
            })?;
            compose.install_definitions(&default_manifests())?;
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
            })?;
            compose.install_definitions(&default_manifests())?;
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
        "{:<24} {:<14} {:<6} {:<8} {:<6} {}",
        "id", "state", "gen", "instance", "tty", "waiting"
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
            println!("{id:<24} {state:<14} {generation:<6} {instance:<8} {tty:<6} {waiting}");
        }
    }
    println!();
    println!("swap a frontend with: navi components reload navi.ui.<name>");
}
