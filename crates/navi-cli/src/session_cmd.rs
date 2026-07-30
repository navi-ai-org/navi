use anyhow::{Context, Result};
use navi_core::{AtifExportOptions, LoadedConfig, SessionStore, atif_to_json, build_trajectory};
use std::fs;
use std::path::Path;

use crate::SessionAction;

pub fn handle_session_command(
    action: SessionAction,
    loaded_config: &LoadedConfig,
    _cwd: &Path,
) -> Result<()> {
    match action {
        SessionAction::List => list_sessions(loaded_config),
        SessionAction::Export { id, out, no_redact } => {
            export_session(&id, out.as_deref(), !no_redact, loaded_config)
        }
    }
}

fn list_sessions(loaded_config: &LoadedConfig) -> Result<()> {
    let store = SessionStore::with_redaction(
        loaded_config.data_dir.clone(),
        loaded_config.config.security.redact_secrets_in_sessions,
    );
    let mut infos = store.list_info();
    infos.sort_by_key(|i| i.updated_at);
    if infos.is_empty() {
        println!("No saved sessions in {}", store.root().display());
        return Ok(());
    }
    println!("{:<48}  {:<24}  updated", "session id", "title");
    for info in infos.iter().rev() {
        let title = info
            .title
            .clone()
            .unwrap_or_else(|| "(untitled)".to_string());
        let title = truncate(&title, 24);
        println!(
            "{:<48}  {:<24}  {}",
            info.id.as_str(),
            title,
            info.updated_at
        );
    }
    Ok(())
}

fn export_session(
    id: &str,
    out: Option<&Path>,
    redact: bool,
    loaded_config: &LoadedConfig,
) -> Result<()> {
    let store = SessionStore::with_redaction(
        loaded_config.data_dir.clone(),
        loaded_config.config.security.redact_secrets_in_sessions,
    );
    let snapshot = store.load(id).with_context(|| {
        format!(
            "failed to load session {id} from {}",
            store.root().display()
        )
    })?;
    let model_name = loaded_config.config.model.name.clone();
    let opts = AtifExportOptions {
        agent_version: env!("CARGO_PKG_VERSION"),
        model_name: &model_name,
        redact_secrets: redact,
    };
    let trajectory = build_trajectory(&snapshot, &opts);
    let json = atif_to_json(&trajectory)?;

    match out {
        Some(path) => {
            write_out(path, &json)?;
            eprintln!(
                "Exported session {id} as ATIF v1.7 to {} ({} steps, redaction {})",
                path.display(),
                trajectory.steps.len(),
                if redact { "on" } else { "off" }
            );
        }
        None => println!("{json}"),
    }
    Ok(())
}

fn write_out(path: &Path, json: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
    }
    fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}
