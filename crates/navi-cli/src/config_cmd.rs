//! `navi config` — show where NAVI reads configuration from, and reset the
//! machine-owned state file.
//!
//! NAVI splits configuration by ownership:
//!
//! * `config.toml` (global and project) — hand-authored. NAVI never writes it.
//! * `state.toml` (global and project) — machine-owned (last model, TUI
//!   preferences, MCP/update flags). Rewritten atomically on every change.
//! * `providers.d/**/*.toml` — custom providers and models, re-read at runtime.
//!
//! `state.toml` wins over `config.toml` for the keys it owns, so a UI choice
//! beats a declared default. `navi config reset-state` deletes it and hands the
//! decision back to `config.toml`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use navi_core::LoadedConfig;
use navi_core::config::providers::dropin;

use crate::ConfigAction;

pub fn handle_config_command(
    action: ConfigAction,
    loaded_config: &LoadedConfig,
    cwd: &Path,
) -> Result<()> {
    match action {
        ConfigAction::Path { json } => paths(loaded_config, cwd, json),
        ConfigAction::Show { json } => show(loaded_config, cwd, json),
        ConfigAction::ResetState { project, yes } => reset_state(loaded_config, cwd, project, yes),
    }
}

/// Resolved config surfaces: `(label, path, exists)`.
fn surfaces(loaded_config: &LoadedConfig, cwd: &Path) -> Vec<(&'static str, PathBuf)> {
    let global_config = loaded_config
        .global_config_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("(unresolved)"));
    let global_state = navi_core::state_path_for_config(&global_config);
    let project_dir = cwd.join(".navi");
    vec![
        ("global config", global_config),
        ("global state", global_state),
        ("project config", project_dir.join("config.toml")),
        ("project state", project_dir.join("state.toml")),
        (
            "global providers.d",
            dropin::roots_for(
                loaded_config
                    .global_config_path
                    .as_ref()
                    .and_then(|path| path.parent()),
                cwd,
            )
            .global
            .unwrap_or_default(),
        ),
        (
            "project providers.d",
            project_dir.join(dropin::PROVIDERS_DIR_NAME),
        ),
    ]
}

fn paths(loaded_config: &LoadedConfig, cwd: &Path, json: bool) -> Result<()> {
    let rows = surfaces(loaded_config, cwd);
    if json {
        let value: Vec<serde_json::Value> = rows
            .iter()
            .map(|(label, path)| {
                serde_json::json!({
                    "kind": label,
                    "path": path,
                    "exists": path.exists(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    for (label, path) in rows {
        let marker = if path.exists() { "" } else { "  (missing)" };
        println!("{label:<20} {}{marker}", path.display());
    }
    Ok(())
}

fn show(loaded_config: &LoadedConfig, cwd: &Path, json: bool) -> Result<()> {
    let config = &loaded_config.config;
    let providers = navi_core::provider_catalog(config);
    let snapshot = dropin::scan(&dropin::roots_for(
        loaded_config
            .global_config_path
            .as_ref()
            .and_then(|path| path.parent()),
        cwd,
    ));

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "model": { "provider": config.model.provider, "name": config.model.name },
                "theme": config.tui.theme,
                "thinking_level": config.tui.thinking_level,
                "yolo_mode": config.tui.yolo_mode,
                "mcp_enabled": config.mcp.enabled,
                "providers": providers.len(),
                "drop_in_files": snapshot.files.len(),
                "drop_in_diagnostics": snapshot.diagnostics.len(),
                "files": surfaces(loaded_config, cwd)
                    .into_iter()
                    .map(|(label, path)| serde_json::json!({
                        "kind": label,
                        "path": path,
                        "exists": path.exists(),
                    }))
                    .collect::<Vec<_>>(),
            }))?
        );
        return Ok(());
    }

    println!("Effective configuration");
    println!("  model          {}/{}", config.model.provider, config.model.name);
    println!(
        "  theme          {} (thinking {}, yolo {})",
        config.tui.theme, config.tui.thinking_level, config.tui.yolo_mode
    );
    println!("  mcp            {}", if config.mcp.enabled { "on" } else { "off" });
    println!("  providers      {} ({} from providers.d)", providers.len(), snapshot.files.len());
    println!();
    println!("Layers (later wins)");
    for (label, path) in surfaces(loaded_config, cwd) {
        let marker = if path.exists() { "" } else { "  (missing)" };
        println!("  {label:<20} {}{marker}", path.display());
    }
    println!();
    println!("Precedence: defaults < config.toml < state.toml, per layer (global, then project).");
    println!("`config.toml` is hand-authored and never written by NAVI.");
    println!("`state.toml` holds the last model/TUI choices; delete it with");
    println!("`navi config reset-state` to fall back to `config.toml`.");
    if !snapshot.diagnostics.is_empty() {
        println!();
        println!("providers.d diagnostics");
        for diagnostic in &snapshot.diagnostics {
            println!("  {diagnostic}");
        }
    }
    Ok(())
}

fn reset_state(loaded_config: &LoadedConfig, cwd: &Path, project: bool, yes: bool) -> Result<()> {
    let target = if project {
        cwd.join(".navi").join("state.toml")
    } else {
        match &loaded_config.global_config_path {
            Some(path) => navi_core::state_path_for_config(path),
            None => bail!("global config path is unavailable"),
        }
    };

    if !target.exists() {
        println!("Nothing to reset: {} does not exist", target.display());
        return Ok(());
    }
    if !yes {
        bail!(
            "about to delete {} (re-run with --yes to confirm)",
            target.display()
        );
    }
    fs::remove_file(&target).with_context(|| format!("failed to delete {}", target.display()))?;
    println!("Removed {}", target.display());
    println!("NAVI will fall back to config.toml for the model and TUI preferences.");
    Ok(())
}
