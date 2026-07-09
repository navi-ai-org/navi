//! Plugin lifecycle APIs on [`NaviEngine`] (install / list / remove / marketplace).
//!
//! Non-interactive: callers pass `confirm = true` to approve install/update.
//! Interactive prompts stay in the CLI only.

use std::fs;
use std::path::{Path, PathBuf};

use navi_plugin_broker::{ReconsentAction, check_update_reconsent, prepare_install_approval};
use navi_plugin_manifest::{
    Lockfile, TrustLevel, aggregate_lockfile_path, compute_wasm_hash, installed_plugins_dir,
    lock_entry_from_manifest, parse_manifest, registry_url, remove_aggregate_lock_entry,
    search_catalog, stage_plugin_by_id, upsert_aggregate_lock_entry, validate,
};
use serde::{Deserialize, Serialize};

use crate::engine::NaviEngine;
use crate::types::NaviError;

type Result<T> = std::result::Result<T, NaviError>;

/// Summary of an installed plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    pub version: String,
    pub publisher: String,
    pub name: String,
    pub description: String,
    pub tools: Vec<String>,
    pub path: String,
}

/// Marketplace catalog entry (search hit).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMarketplaceEntry {
    pub id: String,
    pub version: String,
    pub name: String,
    pub publisher: String,
    pub description: String,
}

/// Result of an install/update operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInstallResult {
    pub id: String,
    pub version: String,
    pub path: String,
    pub tools: usize,
    pub capabilities: usize,
}

impl NaviEngine {
    fn plugin_registry_url(&self) -> String {
        let loaded = self.loaded_config();
        registry_url(
            loaded
                .config
                .plugin_marketplace
                .registry_url
                .as_deref(),
        )
        .to_string()
    }

    /// List installed plugins under `{data_dir}/plugins`.
    pub fn plugin_list(&self) -> Result<Vec<PluginInfo>> {
        let loaded = self.loaded_config();
        let plugin_dir = loaded.data_dir.join("plugins");
        if !plugin_dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&plugin_dir)
            .map_err(|e| NaviError::Config(format!("read plugins dir: {e}")))?
        {
            let entry = entry.map_err(|e| NaviError::Config(e.to_string()))?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let manifest_path = path.join("plugin.toml");
            if !manifest_path.is_file() {
                continue;
            }
            let content = fs::read_to_string(&manifest_path)
                .map_err(|e| NaviError::Config(e.to_string()))?;
            if let Ok(manifest) = parse_manifest(&content) {
                out.push(PluginInfo {
                    id: manifest.plugin.id.clone(),
                    version: manifest.plugin.version.clone(),
                    publisher: manifest.plugin.publisher.clone(),
                    name: manifest.plugin.name.clone(),
                    description: String::new(),
                    tools: manifest.tools.iter().map(|t| t.id.clone()).collect(),
                    path: path.display().to_string(),
                });
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// Details for one installed plugin.
    pub fn plugin_info(&self, plugin_id: &str) -> Result<PluginInfo> {
        let loaded = self.loaded_config();
        let path = loaded.data_dir.join("plugins").join(plugin_id);
        let manifest_path = path.join("plugin.toml");
        if !manifest_path.is_file() {
            return Err(NaviError::Config(format!(
                "plugin '{plugin_id}' not found"
            )));
        }
        let content =
            fs::read_to_string(&manifest_path).map_err(|e| NaviError::Config(e.to_string()))?;
        let manifest =
            parse_manifest(&content).map_err(|e| NaviError::Config(e.to_string()))?;
        Ok(PluginInfo {
            id: manifest.plugin.id.clone(),
            version: manifest.plugin.version.clone(),
            publisher: manifest.plugin.publisher.clone(),
            name: manifest.plugin.name.clone(),
            description: String::new(),
            tools: manifest.tools.iter().map(|t| t.id.clone()).collect(),
            path: path.display().to_string(),
        })
    }

    /// Search the marketplace catalog.
    pub async fn plugin_search(&self, query: Option<&str>) -> Result<Vec<PluginMarketplaceEntry>> {
        let registry = self.plugin_registry_url();
        let catalog = navi_plugin_manifest::fetch_catalog(&registry)
            .await
            .map_err(|e| NaviError::Config(e.to_string()))?;
        let hits = search_catalog(&catalog, query.unwrap_or(""));
        Ok(hits
            .into_iter()
            .map(|e| PluginMarketplaceEntry {
                id: e.id.clone(),
                version: e.version.clone(),
                name: e.name.clone(),
                publisher: e.publisher.clone(),
                description: e.description.clone(),
            })
            .collect())
    }

    /// Install from a local plugin directory. Requires `confirm = true`.
    pub fn plugin_install_path(&self, path: &Path, confirm: bool) -> Result<PluginInstallResult> {
        if !confirm {
            return Err(NaviError::Config(
                "plugin install requires confirm=true (non-interactive approval)".into(),
            ));
        }
        let loaded = self.loaded_config();
        let manifest = load_and_validate_manifest(path)?;
        let _approval = prepare_install_approval(&manifest);
        install_files(path, &manifest, &loaded.data_dir)?;
        write_lockfile(&loaded.data_dir, &manifest)?;
        Ok(PluginInstallResult {
            id: manifest.plugin.id.clone(),
            version: manifest.plugin.version.clone(),
            path: loaded
                .data_dir
                .join("plugins")
                .join(&manifest.plugin.id)
                .display()
                .to_string(),
            tools: manifest.tools.len(),
            capabilities: manifest.capabilities.len(),
        })
    }

    /// Install from marketplace by plugin id. Requires `confirm = true`.
    pub async fn plugin_install_marketplace(
        &self,
        plugin_id: &str,
        confirm: bool,
    ) -> Result<PluginInstallResult> {
        if !confirm {
            return Err(NaviError::Config(
                "plugin install requires confirm=true (non-interactive approval)".into(),
            ));
        }
        let loaded = self.loaded_config();
        let registry = self.plugin_registry_url();
        let (_, staging) = stage_plugin_by_id(&registry, plugin_id, &loaded.data_dir)
            .await
            .map_err(|e| NaviError::Config(e.to_string()))?;
        self.plugin_install_path(&staging, true)
    }

    /// Update from a local directory. `force` overrides publisher-change block.
    pub fn plugin_update_path(
        &self,
        path: &Path,
        force: bool,
        confirm: bool,
    ) -> Result<PluginInstallResult> {
        let loaded = self.loaded_config();
        let new_manifest = load_and_validate_manifest(path)?;
        let plugin_id = new_manifest.plugin.id.clone();
        let installed_dir = loaded.data_dir.join("plugins").join(&plugin_id);
        if !installed_dir.exists() {
            return Err(NaviError::Config(format!(
                "plugin '{plugin_id}' is not installed; use plugin_install_path first"
            )));
        }

        let old_manifest_path = installed_dir.join("plugin.toml");
        let old_content = fs::read_to_string(&old_manifest_path)
            .map_err(|e| NaviError::Config(e.to_string()))?;
        let old_manifest =
            parse_manifest(&old_content).map_err(|e| NaviError::Config(e.to_string()))?;
        let plugins_root = installed_plugins_dir(&loaded.data_dir);
        let lockfile =
            Lockfile::load(&aggregate_lockfile_path(&plugins_root)).unwrap_or_default();
        let old_entry = lockfile.find(&plugin_id).cloned().ok_or_else(|| {
            NaviError::Config(format!(
                "plugin '{plugin_id}' has no lockfile entry; reinstall"
            ))
        })?;

        let reconsent = check_update_reconsent(&old_entry, &new_manifest, &old_manifest);
        match reconsent.action {
            ReconsentAction::Block if !force => {
                return Err(NaviError::Config(
                    "update blocked (publisher change); pass force=true to override".into(),
                ));
            }
            ReconsentAction::RequireReconsent if !confirm => {
                return Err(NaviError::Config(
                    "update requires reconsent; pass confirm=true".into(),
                ));
            }
            _ => {}
        }

        install_files(path, &new_manifest, &loaded.data_dir)?;
        let mut approved_caps: std::collections::BTreeSet<String> =
            old_entry.approved_capabilities.into_iter().collect();
        for cap in &new_manifest.capabilities {
            approved_caps.insert(cap.id().to_string());
        }
        write_lockfile_with_approved(
            &loaded.data_dir,
            &new_manifest,
            approved_caps.into_iter().collect(),
        )?;

        Ok(PluginInstallResult {
            id: plugin_id,
            version: new_manifest.plugin.version.clone(),
            path: installed_dir.display().to_string(),
            tools: new_manifest.tools.len(),
            capabilities: new_manifest.capabilities.len(),
        })
    }

    /// Update from marketplace. `force` overrides publisher-change block.
    pub async fn plugin_update_marketplace(
        &self,
        plugin_id: &str,
        force: bool,
        confirm: bool,
    ) -> Result<PluginInstallResult> {
        let loaded = self.loaded_config();
        let registry = self.plugin_registry_url();
        let (_, staging) = stage_plugin_by_id(&registry, plugin_id, &loaded.data_dir)
            .await
            .map_err(|e| NaviError::Config(e.to_string()))?;
        self.plugin_update_path(&staging, force, confirm)
    }

    /// Remove an installed plugin by id.
    pub fn plugin_remove(&self, plugin_id: &str) -> Result<()> {
        let loaded = self.loaded_config();
        let plugin_dir = loaded.data_dir.join("plugins").join(plugin_id);
        if !plugin_dir.exists() {
            return Err(NaviError::Config(format!(
                "plugin '{plugin_id}' not found"
            )));
        }
        fs::remove_dir_all(&plugin_dir)
            .map_err(|e| NaviError::Config(format!("remove plugin: {e}")))?;
        let plugins_root = installed_plugins_dir(&loaded.data_dir);
        if let Err(e) = remove_aggregate_lock_entry(&plugins_root, plugin_id) {
            tracing::warn!(plugin = plugin_id, error = %e, "lockfile update failed on remove");
        }
        Ok(())
    }
}

fn load_and_validate_manifest(path: &Path) -> Result<navi_plugin_manifest::PluginManifest> {
    if !path.exists() {
        return Err(NaviError::Config(format!(
            "plugin directory not found: {}",
            path.display()
        )));
    }
    let manifest_path = path.join("plugin.toml");
    if !manifest_path.exists() {
        return Err(NaviError::Config(format!(
            "no plugin.toml in {}",
            path.display()
        )));
    }
    let manifest_content =
        fs::read_to_string(&manifest_path).map_err(|e| NaviError::Config(e.to_string()))?;
    let manifest =
        parse_manifest(&manifest_content).map_err(|e| NaviError::Config(e.to_string()))?;
    validate(&manifest, TrustLevel::Community)
        .map_err(|e| NaviError::Config(e.to_string()))?;
    let wasm_path = path.join(&manifest.plugin.entry);
    if !wasm_path.exists() {
        return Err(NaviError::Config(format!(
            "WASM binary not found: {}",
            wasm_path.display()
        )));
    }
    let wasm_bytes =
        fs::read(&wasm_path).map_err(|e| NaviError::Config(e.to_string()))?;
    let actual_hash = compute_wasm_hash(&wasm_bytes);
    if actual_hash != manifest.plugin.wasm_hash {
        return Err(NaviError::Config(format!(
            "WASM hash mismatch: declared {} actual {actual_hash}",
            manifest.plugin.wasm_hash
        )));
    }
    navi_plugin_manifest::verify_plugin_signature(&manifest, &wasm_bytes, TrustLevel::Community)
        .map_err(|reason| NaviError::Config(format!("signature verification failed: {reason}")))?;
    Ok(manifest)
}

fn install_files(
    source_path: &Path,
    manifest: &navi_plugin_manifest::PluginManifest,
    data_dir: &Path,
) -> Result<PathBuf> {
    let plugin_dir = data_dir.join("plugins").join(&manifest.plugin.id);
    if plugin_dir.exists() {
        fs::remove_dir_all(&plugin_dir)
            .map_err(|e| NaviError::Config(format!("remove existing plugin: {e}")))?;
    }
    copy_dir_recursive(source_path, &plugin_dir)
        .map_err(|e| NaviError::Config(format!("copy plugin: {e}")))?;
    Ok(plugin_dir)
}

fn write_lockfile(data_dir: &Path, manifest: &navi_plugin_manifest::PluginManifest) -> Result<()> {
    let approved = manifest
        .capabilities
        .iter()
        .map(|c| c.id().to_string())
        .collect();
    write_lockfile_with_approved(data_dir, manifest, approved)
}

fn write_lockfile_with_approved(
    data_dir: &Path,
    manifest: &navi_plugin_manifest::PluginManifest,
    approved_capabilities: Vec<String>,
) -> Result<()> {
    let plugins_root = installed_plugins_dir(data_dir);
    let entry = lock_entry_from_manifest(manifest, approved_capabilities);
    upsert_aggregate_lock_entry(&plugins_root, entry)
        .map_err(|e| NaviError::Config(format!("save lockfile: {e}")))?;
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dest)?;
        } else {
            fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}
