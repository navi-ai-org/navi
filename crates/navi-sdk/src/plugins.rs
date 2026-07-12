//! Plugin lifecycle APIs on [`NaviEngine`] (install / list / remove / marketplace).
//!
//! Non-interactive: callers pass `confirm = true` to approve install/update.
//! Interactive prompts stay in the CLI only.

use std::fs;
use std::path::{Path, PathBuf};

use navi_plugin_broker::{ReconsentAction, check_update_reconsent, prepare_install_approval};
use navi_plugin_manifest::{
    Lockfile, PluginCatalogKind, TrustLevel, aggregate_lockfile_path, compute_wasm_hash,
    installed_plugins_dir, lock_entry_from_manifest_with_meta, parse_manifest, registry_url,
    remove_aggregate_lock_entry, search_catalog, upsert_aggregate_lock_entry, validate,
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
    /// Marketplace kind when known (`plugin` / `skill` / `mcp` / `integration`).
    #[serde(default)]
    pub kind: String,
    /// Install trust level (`community` / `local-dev` / …).
    #[serde(default)]
    pub trust_level: String,
}

/// Marketplace catalog entry (search hit).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMarketplaceEntry {
    pub id: String,
    pub version: String,
    pub name: String,
    pub publisher: String,
    pub description: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Result of an install/update operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInstallResult {
    pub id: String,
    pub version: String,
    pub path: String,
    pub tools: usize,
    pub capabilities: usize,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub trust_level: String,
    /// Human-readable note for kind-specific follow-up (MCP config, skill activation, …).
    #[serde(default)]
    pub kind_hint: String,
}

impl NaviEngine {
    fn plugin_registry_url(&self) -> String {
        let loaded = self.loaded_config();
        registry_url(loaded.config.plugin_marketplace.registry_url.as_deref()).to_string()
    }

    /// List installed plugins under `{data_dir}/plugins`.
    pub fn plugin_list(&self) -> Result<Vec<PluginInfo>> {
        let loaded = self.loaded_config();
        let plugin_dir = loaded.data_dir.join("plugins");
        if !plugin_dir.exists() {
            return Ok(Vec::new());
        }
        let lock = Lockfile::load(&aggregate_lockfile_path(&plugin_dir)).unwrap_or_default();
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
            let content =
                fs::read_to_string(&manifest_path).map_err(|e| NaviError::Config(e.to_string()))?;
            if let Ok(manifest) = parse_manifest(&content) {
                let lock_meta = lock.find(&manifest.plugin.id);
                out.push(PluginInfo {
                    id: manifest.plugin.id.clone(),
                    version: manifest.plugin.version.clone(),
                    publisher: manifest.plugin.publisher.clone(),
                    name: manifest.plugin.name.clone(),
                    description: String::new(),
                    tools: manifest.tools.iter().map(|t| t.id.clone()).collect(),
                    path: path.display().to_string(),
                    kind: lock_meta
                        .map(|e| catalog_kind_label(e.kind).to_string())
                        .unwrap_or_else(|| "plugin".into()),
                    trust_level: lock_meta
                        .map(|e| trust_label(e.trust_level).to_string())
                        .unwrap_or_else(|| "community".into()),
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
            return Err(NaviError::Config(format!("plugin '{plugin_id}' not found")));
        }
        let content =
            fs::read_to_string(&manifest_path).map_err(|e| NaviError::Config(e.to_string()))?;
        let manifest = parse_manifest(&content).map_err(|e| NaviError::Config(e.to_string()))?;
        let plugins_root = installed_plugins_dir(&loaded.data_dir);
        let lock = Lockfile::load(&aggregate_lockfile_path(&plugins_root)).unwrap_or_default();
        let lock_meta = lock.find(&manifest.plugin.id);
        Ok(PluginInfo {
            id: manifest.plugin.id.clone(),
            version: manifest.plugin.version.clone(),
            publisher: manifest.plugin.publisher.clone(),
            name: manifest.plugin.name.clone(),
            description: String::new(),
            tools: manifest.tools.iter().map(|t| t.id.clone()).collect(),
            path: path.display().to_string(),
            kind: lock_meta
                .map(|e| catalog_kind_label(e.kind).to_string())
                .unwrap_or_else(|| "plugin".into()),
            trust_level: lock_meta
                .map(|e| trust_label(e.trust_level).to_string())
                .unwrap_or_else(|| "community".into()),
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
                kind: catalog_kind_label(e.kind).to_string(),
                tags: e.tags.clone(),
            })
            .collect())
    }

    /// Install from a local plugin directory (LocalDev trust — signature optional).
    /// Requires `confirm = true`.
    pub fn plugin_install_path(&self, path: &Path, confirm: bool) -> Result<PluginInstallResult> {
        self.plugin_install_path_with_meta(
            path,
            confirm,
            TrustLevel::LocalDev,
            PluginCatalogKind::Plugin,
        )
    }

    /// Install from a local directory with explicit trust and marketplace kind.
    pub fn plugin_install_path_with_meta(
        &self,
        path: &Path,
        confirm: bool,
        trust: TrustLevel,
        kind: PluginCatalogKind,
    ) -> Result<PluginInstallResult> {
        if !confirm {
            return Err(NaviError::Config(
                "plugin install requires confirm=true (non-interactive approval)".into(),
            ));
        }
        let loaded = self.loaded_config();
        let manifest = load_and_validate_manifest(path, trust)?;
        let _approval = prepare_install_approval(&manifest);
        install_files(path, &manifest, &loaded.data_dir)?;
        write_lockfile_with_meta(&loaded.data_dir, &manifest, trust, kind)?;
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
            kind: catalog_kind_label(kind).to_string(),
            trust_level: trust_label(trust).to_string(),
            kind_hint: kind_install_hint(kind).to_string(),
        })
    }

    /// Install from marketplace by plugin id. Requires `confirm = true`.
    /// Marketplace packages always use Community trust (signed WASM).
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
        let catalog = navi_plugin_manifest::fetch_catalog(&registry)
            .await
            .map_err(|e| NaviError::Config(e.to_string()))?;
        let entry = navi_plugin_manifest::find_catalog_entry(&catalog, plugin_id)
            .map_err(|e| NaviError::Config(e.to_string()))?;
        let kind = entry.kind;
        let staging = navi_plugin_manifest::plugin_staging_dir(&loaded.data_dir, plugin_id);
        navi_plugin_manifest::stage_plugin_from_catalog(&registry, entry, &staging)
            .await
            .map_err(|e| NaviError::Config(e.to_string()))?;
        self.plugin_install_path_with_meta(&staging, true, TrustLevel::Community, kind)
    }

    /// Update from a local directory. `force` overrides publisher-change block.
    pub fn plugin_update_path(
        &self,
        path: &Path,
        force: bool,
        confirm: bool,
    ) -> Result<PluginInstallResult> {
        let loaded = self.loaded_config();
        let plugins_root = installed_plugins_dir(&loaded.data_dir);
        let lockfile = Lockfile::load(&aggregate_lockfile_path(&plugins_root)).unwrap_or_default();
        // Peek id from manifest path before full validate (trust from existing lock entry).
        let peek = {
            let content = fs::read_to_string(path.join("plugin.toml"))
                .map_err(|e| NaviError::Config(e.to_string()))?;
            parse_manifest(&content).map_err(|e| NaviError::Config(e.to_string()))?
        };
        let old_entry = lockfile.find(&peek.plugin.id).cloned();
        let trust = old_entry
            .as_ref()
            .map(|e| e.trust_level)
            .unwrap_or(TrustLevel::LocalDev);
        let kind = old_entry
            .as_ref()
            .map(|e| e.kind)
            .unwrap_or(PluginCatalogKind::Plugin);

        let new_manifest = load_and_validate_manifest(path, trust)?;
        let plugin_id = new_manifest.plugin.id.clone();
        let installed_dir = loaded.data_dir.join("plugins").join(&plugin_id);
        if !installed_dir.exists() {
            return Err(NaviError::Config(format!(
                "plugin '{plugin_id}' is not installed; use plugin_install_path first"
            )));
        }

        let old_manifest_path = installed_dir.join("plugin.toml");
        let old_content =
            fs::read_to_string(&old_manifest_path).map_err(|e| NaviError::Config(e.to_string()))?;
        let old_manifest =
            parse_manifest(&old_content).map_err(|e| NaviError::Config(e.to_string()))?;
        let old_entry = old_entry.ok_or_else(|| {
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
            trust,
            kind,
        )?;

        Ok(PluginInstallResult {
            id: plugin_id,
            version: new_manifest.plugin.version.clone(),
            path: installed_dir.display().to_string(),
            tools: new_manifest.tools.len(),
            capabilities: new_manifest.capabilities.len(),
            kind: catalog_kind_label(kind).to_string(),
            trust_level: trust_label(trust).to_string(),
            kind_hint: kind_install_hint(kind).to_string(),
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
        let catalog = navi_plugin_manifest::fetch_catalog(&registry)
            .await
            .map_err(|e| NaviError::Config(e.to_string()))?;
        let entry = navi_plugin_manifest::find_catalog_entry(&catalog, plugin_id)
            .map_err(|e| NaviError::Config(e.to_string()))?;
        let kind = entry.kind;
        let staging = navi_plugin_manifest::plugin_staging_dir(&loaded.data_dir, plugin_id);
        navi_plugin_manifest::stage_plugin_from_catalog(&registry, entry, &staging)
            .await
            .map_err(|e| NaviError::Config(e.to_string()))?;
        let mut result = self.plugin_update_path(&staging, force, confirm)?;
        // Marketplace updates keep Community trust and catalog kind.
        result.kind = catalog_kind_label(kind).to_string();
        result.trust_level = trust_label(TrustLevel::Community).to_string();
        result.kind_hint = kind_install_hint(kind).to_string();
        // Re-write lock meta with Community + catalog kind (update_path may have kept LocalDev).
        if let Ok(content) = fs::read_to_string(staging.join("plugin.toml"))
            && let Ok(manifest) = parse_manifest(&content)
        {
            let approved = manifest
                .capabilities
                .iter()
                .map(|c| c.id().to_string())
                .collect();
            let _ = write_lockfile_with_approved(
                &loaded.data_dir,
                &manifest,
                approved,
                TrustLevel::Community,
                kind,
            );
        }
        Ok(result)
    }

    /// Remove an installed plugin by id.
    pub fn plugin_remove(&self, plugin_id: &str) -> Result<()> {
        let loaded = self.loaded_config();
        let plugin_dir = loaded.data_dir.join("plugins").join(plugin_id);
        if !plugin_dir.exists() {
            return Err(NaviError::Config(format!("plugin '{plugin_id}' not found")));
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

fn load_and_validate_manifest(
    path: &Path,
    trust: TrustLevel,
) -> Result<navi_plugin_manifest::PluginManifest> {
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
    validate(&manifest, trust).map_err(|e| NaviError::Config(e.to_string()))?;
    let wasm_path = path.join(&manifest.plugin.entry);
    if !wasm_path.exists() {
        return Err(NaviError::Config(format!(
            "WASM binary not found: {}",
            wasm_path.display()
        )));
    }
    let wasm_bytes = fs::read(&wasm_path).map_err(|e| NaviError::Config(e.to_string()))?;
    let actual_hash = compute_wasm_hash(&wasm_bytes);
    if actual_hash != manifest.plugin.wasm_hash {
        return Err(NaviError::Config(format!(
            "WASM hash mismatch: declared {} actual {actual_hash}",
            manifest.plugin.wasm_hash
        )));
    }
    navi_plugin_manifest::verify_plugin_signature(&manifest, &wasm_bytes, trust)
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

fn write_lockfile_with_meta(
    data_dir: &Path,
    manifest: &navi_plugin_manifest::PluginManifest,
    trust: TrustLevel,
    kind: PluginCatalogKind,
) -> Result<()> {
    let approved = manifest
        .capabilities
        .iter()
        .map(|c| c.id().to_string())
        .collect();
    write_lockfile_with_approved(data_dir, manifest, approved, trust, kind)
}

fn write_lockfile_with_approved(
    data_dir: &Path,
    manifest: &navi_plugin_manifest::PluginManifest,
    approved_capabilities: Vec<String>,
    trust: TrustLevel,
    kind: PluginCatalogKind,
) -> Result<()> {
    let plugins_root = installed_plugins_dir(data_dir);
    let entry =
        lock_entry_from_manifest_with_meta(manifest, approved_capabilities, trust, kind);
    upsert_aggregate_lock_entry(&plugins_root, entry)
        .map_err(|e| NaviError::Config(format!("save lockfile: {e}")))?;
    Ok(())
}

fn catalog_kind_label(kind: PluginCatalogKind) -> &'static str {
    match kind {
        PluginCatalogKind::Plugin => "plugin",
        PluginCatalogKind::Skill => "skill",
        PluginCatalogKind::Mcp => "mcp",
        PluginCatalogKind::Integration => "integration",
    }
}

fn trust_label(trust: TrustLevel) -> &'static str {
    match trust {
        TrustLevel::Core => "core",
        TrustLevel::Signed => "signed",
        TrustLevel::Community => "community",
        TrustLevel::LocalDev => "local-dev",
    }
}

/// Follow-up guidance after install based on marketplace package kind.
fn kind_install_hint(kind: PluginCatalogKind) -> &'static str {
    match kind {
        PluginCatalogKind::Plugin => {
            "WASM tools register on next session (or reload plugins)."
        }
        PluginCatalogKind::Skill => {
            "Skill pack installed as WASM plugin; activate related skills in session if provided."
        }
        PluginCatalogKind::Mcp => {
            "MCP adapter package installed as WASM. If the package ships mcp.json, merge that server into ~/.config/navi/config.toml [[mcp.servers]] (or global MCP config)."
        }
        PluginCatalogKind::Integration => {
            "Integration package installed as WASM plugin (messaging bots / sidecars may need env secrets)."
        }
    }
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
