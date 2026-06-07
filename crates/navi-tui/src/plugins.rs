//! Plugin marketplace modal: browse registry catalog and manage installed plugins.

use std::path::Path;

use anyhow::Result;
use navi_plugin_manifest::{
    Lockfile, PluginCatalogEntry, aggregate_lockfile_path, installed_plugins_dir, registry_url,
};

use crate::TuiApp;
use crate::plugin_approval::{request_install_approval, request_update_approval};
use crate::runtime::spawn_runtime_task;

/// Row in the plugins modal list.
#[derive(Debug, Clone)]
pub(crate) enum PluginPickerRow {
    /// Plugin available in the marketplace catalog.
    Catalog(PluginCatalogEntry),
    /// Plugin installed under `{data_dir}/plugins/<id>/`.
    Installed {
        id: String,
        version: String,
        publisher: String,
        tool_count: usize,
    },
}

/// Fetch the marketplace catalog from the configured registry URL.
pub(crate) fn refresh_plugin_catalog(app: &mut TuiApp) {
    app.plugin_catalog_loading = true;
    app.plugin_catalog_error.clear();
    let registry = registry_url(
        app.loaded_config
            .config
            .plugin_marketplace
            .registry_url
            .as_deref(),
    )
    .to_string();
    let tx = app.async_sender();
    spawn_runtime_task(async move {
        let result = navi_plugin_manifest::fetch_catalog(&registry).await;
        let event = match result {
            Ok(catalog) => crate::dispatch::AsyncEvent::PluginCatalogLoaded {
                entries: catalog.plugins,
                error: None,
            },
            Err(err) => crate::dispatch::AsyncEvent::PluginCatalogLoaded {
                entries: Vec::new(),
                error: Some(format!("{err}")),
            },
        };
        let _ = tx.send(event);
    });
}

/// Build picker rows: catalog entries first, then installed (not duplicated in catalog section).
pub(crate) fn plugin_picker_rows(app: &TuiApp) -> Vec<PluginPickerRow> {
    let mut rows = Vec::new();
    for entry in &app.plugin_catalog {
        rows.push(PluginPickerRow::Catalog(entry.clone()));
    }
    let plugin_dir = app.loaded_config.data_dir.join("plugins");
    if plugin_dir.exists()
        && let Ok(rd) = std::fs::read_dir(&plugin_dir)
    {
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let manifest_path = path.join("plugin.toml");
            if !manifest_path.exists() {
                continue;
            }
            if app.plugin_catalog.iter().any(|c| c.id == name) {
                continue;
            }
            let (version, publisher, tool_count) =
                read_installed_summary(&manifest_path).unwrap_or(("?".into(), "?".into(), 0));
            rows.push(PluginPickerRow::Installed {
                id: name,
                version,
                publisher,
                tool_count,
            });
        }
    }
    rows.sort_by_key(plugin_row_sort_key);
    rows
}

fn plugin_row_sort_key(row: &PluginPickerRow) -> (u8, String) {
    match row {
        PluginPickerRow::Catalog(e) => (0, e.id.clone()),
        PluginPickerRow::Installed { id, .. } => (1, id.clone()),
    }
}

fn read_installed_summary(manifest_path: &Path) -> Result<(String, String, usize)> {
    let content = std::fs::read_to_string(manifest_path)?;
    let manifest = navi_plugin_manifest::parse_manifest(&content)?;
    Ok((
        manifest.plugin.version,
        manifest.plugin.publisher,
        manifest.tools.len(),
    ))
}

/// Install or update the selected plugin from the marketplace (staging + approval modal).
pub(crate) fn install_or_update_from_marketplace(app: &mut TuiApp, plugin_id: &str, update: bool) {
    let data_dir = app.loaded_config.data_dir.clone();
    let registry = registry_url(
        app.loaded_config
            .config
            .plugin_marketplace
            .registry_url
            .as_deref(),
    )
    .to_string();
    let plugin_id = plugin_id.to_string();
    let tx = app.async_sender();
    spawn_runtime_task(async move {
        let event = match navi_plugin_manifest::stage_plugin_by_id(&registry, &plugin_id, &data_dir)
            .await
        {
            Ok((_, staging)) => crate::dispatch::AsyncEvent::PluginStaged {
                plugin_id,
                staging_path: staging,
                update,
                error: None,
            },
            Err(err) => crate::dispatch::AsyncEvent::PluginStaged {
                plugin_id,
                staging_path: std::path::PathBuf::new(),
                update,
                error: Some(format!("{err}")),
            },
        };
        let _ = tx.send(event);
    });
}

/// Handle staged plugin: open approval modal.
pub(crate) fn handle_plugin_staged(
    app: &mut TuiApp,
    plugin_id: &str,
    staging_path: &Path,
    update: bool,
) -> Result<()> {
    if update {
        request_update_approval(app, staging_path)
    } else {
        request_install_approval(app, staging_path)
    }
    .map_err(|e| {
        tracing::warn!(plugin = %plugin_id, "plugin staging approval failed: {e:#}");
        e
    })
}

/// Hot-reload WASM plugins on the running engine after install/update.
pub(crate) fn reload_engine_plugins(app: &TuiApp) {
    let engine = app.engine();
    let tx = app.async_sender();
    spawn_runtime_task(async move {
        let event = match engine.reload_wasm_plugins().await {
            Ok(warnings) => crate::dispatch::AsyncEvent::PluginsReloaded {
                error: None,
                warnings,
            },
            Err(err) => crate::dispatch::AsyncEvent::PluginsReloaded {
                error: Some(format!("{err:#}")),
                warnings: Vec::new(),
            },
        };
        let _ = tx.send(event);
    });
}

/// List installed plugin ids from the aggregate lockfile.
pub(crate) fn list_installed_plugin_ids(app: &TuiApp) -> Vec<String> {
    let plugins_root = installed_plugins_dir(&app.loaded_config.data_dir);
    let lockfile_path = aggregate_lockfile_path(&plugins_root);
    Lockfile::load(&lockfile_path)
        .map(|lock| lock.plugins.into_iter().map(|e| e.id).collect())
        .unwrap_or_default()
}
