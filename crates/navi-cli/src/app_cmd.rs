//! `navi app` subcommand: list and remove app-scoped storage (ADR 0019 §4).

use anyhow::{Context, Result, bail};
use navi_core::{AppScope, LoadedConfig};
use std::fs;
use std::path::PathBuf;

use crate::AppAction;

pub fn handle_app_command(action: AppAction, loaded_config: &LoadedConfig) -> Result<()> {
    match action {
        AppAction::List => list_apps(loaded_config),
        AppAction::Remove { id, force } => remove_app(&id, force, loaded_config),
    }
}

fn list_apps(loaded_config: &LoadedConfig) -> Result<()> {
    let apps_root = loaded_config.data_dir.join("apps");
    if !apps_root.is_dir() {
        println!("No app-scoped storage found.");
        println!("Apps directory: {} (does not exist)", apps_root.display());
        return Ok(());
    }

    let entries = fs::read_dir(&apps_root)
        .with_context(|| format!("read apps dir {}", apps_root.display()))?;

    let mut apps: Vec<(String, AppInfo)> = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // Validate it's a proper app id
        if AppScope::validate_id(&name).is_err() {
            continue;
        }
        let info = collect_app_info(&path);
        apps.push((name, info));
    }

    apps.sort_by(|a, b| a.0.cmp(&b.0));

    if apps.is_empty() {
        println!("No installed app namespaces in {}", apps_root.display());
        return Ok(());
    }

    println!(
        "{:<30}  {:>10}  {:>8}  {:>12}",
        "app id", "size", "sessions", "last modified"
    );
    for (id, info) in &apps {
        println!(
            "{:<30}  {:>10}  {:>8}  {:>12}",
            id,
            format_size(info.size_bytes),
            info.session_count,
            info.last_modified
                .map(|t| format_unix_timestamp(t))
                .unwrap_or_else(|| "—".to_string()),
        );
    }
    Ok(())
}

fn remove_app(id: &str, force: bool, loaded_config: &LoadedConfig) -> Result<()> {
    // Validate the app id before doing anything
    AppScope::validate_id(id).with_context(|| format!("invalid app id '{id}'"))?;

    let app_root = loaded_config.data_dir.join("apps").join(id);
    if !app_root.is_dir() {
        bail!(
            "app namespace '{id}' does not exist at {}",
            app_root.display()
        );
    }

    let info = collect_app_info(&app_root);

    if !force {
        println!("About to remove app namespace '{id}':");
        println!("  Path:     {}", app_root.display());
        println!("  Size:     {}", format_size(info.size_bytes));
        println!("  Sessions: {}", info.session_count);
        println!();
        print!("Type the app id to confirm removal: ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let confirmed = input.trim();
        if confirmed != id {
            println!("Confirmation did not match. Aborting.");
            return Ok(());
        }
    }

    fs::remove_dir_all(&app_root)
        .with_context(|| format!("failed to remove app dir {}", app_root.display()))?;
    println!(
        "Removed app namespace '{id}' ({} freed).",
        format_size(info.size_bytes)
    );
    Ok(())
}

struct AppInfo {
    size_bytes: u64,
    session_count: usize,
    last_modified: Option<u64>,
}

fn collect_app_info(app_root: &PathBuf) -> AppInfo {
    let size_bytes = dir_size(app_root);
    let session_count = count_sessions(app_root);
    let last_modified = newest_mtime(app_root);
    AppInfo {
        size_bytes,
        session_count,
        last_modified,
    }
}

fn dir_size(path: &PathBuf) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                total += dir_size(&p);
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

fn count_sessions(app_root: &PathBuf) -> usize {
    let sessions_dir = app_root.join("sessions");
    if !sessions_dir.is_dir() {
        return 0;
    }
    fs::read_dir(&sessions_dir)
        .map(|entries| {
            entries
                .filter(|e| e.is_ok())
                .filter(|e| e.as_ref().unwrap().path().is_file())
                .count()
        })
        .unwrap_or(0)
}

fn newest_mtime(path: &PathBuf) -> Option<u64> {
    let mut newest: Option<u64> = None;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if let Some(t) = newest_mtime(&p) {
                    newest = Some(newest.map_or(t, |c| c.max(t)));
                }
            }
            if let Ok(meta) = entry.metadata() {
                if let Ok(t) = meta.modified() {
                    let secs = t
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    newest = Some(newest.map_or(secs, |c| c.max(secs)));
                }
            }
        }
    }
    newest
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn format_unix_timestamp(secs: u64) -> String {
    // Simple ISO-ish format without chrono dependency
    let days = secs / 86400;
    let time = secs % 86400;
    let h = time / 3600;
    let m = (time % 3600) / 60;
    format!("day+{days} {h:02}:{m:02}")
}
