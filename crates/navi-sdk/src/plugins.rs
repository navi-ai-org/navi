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
use serde_json;

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
        for entry in fs::read_dir(&plugin_dir).map_err(|e| {
            NaviError::Config(format!("read plugins dir {}: {e}", plugin_dir.display()))
        })? {
            let entry = entry.map_err(|e| {
                NaviError::Config(format!(
                    "read plugins dir entry under {}: {e}",
                    plugin_dir.display()
                ))
            })?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let manifest_path = path.join("plugin.toml");
            if !manifest_path.is_file() {
                continue;
            }
            let content = fs::read_to_string(&manifest_path).map_err(|e| {
                NaviError::Config(format!(
                    "read plugin manifest {}: {e}",
                    manifest_path.display()
                ))
            })?;
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
        let content = fs::read_to_string(&manifest_path).map_err(|e| {
            NaviError::Config(format!(
                "read plugin manifest {}: {e}",
                manifest_path.display()
            ))
        })?;
        let manifest = parse_manifest(&content)
            .map_err(|e| NaviError::Config(format!("parse plugin.toml for '{plugin_id}': {e}")))?;
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
            .map_err(|e| NaviError::Config(format!("fetch plugin marketplace catalog: {e}")))?;
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
    ///
    /// Package kind is auto-detected from optional side-car files (`mcp.json`,
    /// `SKILL.md` / `skill.toml`), defaulting to `plugin`.
    pub fn plugin_install_path(&self, path: &Path, confirm: bool) -> Result<PluginInstallResult> {
        let kind = detect_package_kind(path);
        self.plugin_install_path_with_meta(path, confirm, TrustLevel::LocalDev, kind)
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
        let installed = install_files(path, &manifest, &loaded.data_dir)?;
        write_lockfile_with_meta(&loaded.data_dir, &manifest, trust, kind)?;
        let mut kind_hint = kind_install_hint(kind).to_string();
        if let Some(extra) =
            apply_kind_side_effects_at(&loaded.data_dir, &self.inner.project_dir, &installed, kind)
        {
            kind_hint = extra;
        }
        Ok(PluginInstallResult {
            id: manifest.plugin.id.clone(),
            version: manifest.plugin.version.clone(),
            path: installed.display().to_string(),
            tools: manifest.tools.len(),
            capabilities: manifest.capabilities.len(),
            kind: catalog_kind_label(kind).to_string(),
            trust_level: trust_label(trust).to_string(),
            kind_hint,
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
            .map_err(|e| NaviError::Config(format!("fetch plugin marketplace catalog: {e}")))?;
        let entry = navi_plugin_manifest::find_catalog_entry(&catalog, plugin_id).map_err(|e| {
            NaviError::Config(format!("find marketplace plugin '{plugin_id}': {e}"))
        })?;
        let kind = entry.kind;
        let staging = navi_plugin_manifest::plugin_staging_dir(&loaded.data_dir, plugin_id);
        navi_plugin_manifest::stage_plugin_from_catalog(&registry, entry, &staging)
            .await
            .map_err(|e| {
                NaviError::Config(format!("stage marketplace plugin '{plugin_id}': {e}"))
            })?;
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
            let manifest_path = path.join("plugin.toml");
            let content = fs::read_to_string(&manifest_path).map_err(|e| {
                NaviError::Config(format!(
                    "read plugin manifest {}: {e}",
                    manifest_path.display()
                ))
            })?;
            parse_manifest(&content).map_err(|e| {
                NaviError::Config(format!(
                    "parse plugin.toml at {}: {e}",
                    manifest_path.display()
                ))
            })?
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
        let old_content = fs::read_to_string(&old_manifest_path).map_err(|e| {
            NaviError::Config(format!(
                "read installed plugin manifest {}: {e}",
                old_manifest_path.display()
            ))
        })?;
        let old_manifest = parse_manifest(&old_content).map_err(|e| {
            NaviError::Config(format!(
                "parse installed plugin.toml for '{plugin_id}': {e}"
            ))
        })?;
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
            .map_err(|e| NaviError::Config(format!("fetch plugin marketplace catalog: {e}")))?;
        let entry = navi_plugin_manifest::find_catalog_entry(&catalog, plugin_id).map_err(|e| {
            NaviError::Config(format!("find marketplace plugin '{plugin_id}': {e}"))
        })?;
        let kind = entry.kind;
        let staging = navi_plugin_manifest::plugin_staging_dir(&loaded.data_dir, plugin_id);
        navi_plugin_manifest::stage_plugin_from_catalog(&registry, entry, &staging)
            .await
            .map_err(|e| {
                NaviError::Config(format!(
                    "stage marketplace plugin '{plugin_id}' for update: {e}"
                ))
            })?;
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
    let manifest_content = fs::read_to_string(&manifest_path).map_err(|e| {
        NaviError::Config(format!(
            "read plugin manifest {}: {e}",
            manifest_path.display()
        ))
    })?;
    let manifest = parse_manifest(&manifest_content).map_err(|e| {
        NaviError::Config(format!(
            "parse plugin.toml at {}: {e}",
            manifest_path.display()
        ))
    })?;
    validate(&manifest, trust)
        .map_err(|e| NaviError::Config(format!("validate plugin at {}: {e}", path.display())))?;
    let wasm_path = path.join(&manifest.plugin.entry);
    if !wasm_path.exists() {
        return Err(NaviError::Config(format!(
            "WASM binary not found: {}",
            wasm_path.display()
        )));
    }
    let wasm_bytes = fs::read(&wasm_path)
        .map_err(|e| NaviError::Config(format!("read WASM binary {}: {e}", wasm_path.display())))?;
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
    copy_dir_recursive(source_path, &plugin_dir).map_err(|e| {
        NaviError::Config(format!(
            "copy plugin from {} to {}: {e}",
            source_path.display(),
            plugin_dir.display()
        ))
    })?;
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
    let entry = lock_entry_from_manifest_with_meta(manifest, approved_capabilities, trust, kind);
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
        PluginCatalogKind::Plugin => "WASM tools register on next session (or reload plugins).",
        PluginCatalogKind::Skill => "Skill pack installed as WASM; SKILL.md imported when present.",
        PluginCatalogKind::Mcp => {
            "MCP package installed as WASM; mcp.json merged into global MCP config when present."
        }
        PluginCatalogKind::Integration => {
            "Integration package installed as WASM plugin (messaging bots / sidecars may need env secrets)."
        }
    }
}

/// Infer marketplace kind from package side-car files.
pub fn detect_package_kind(path: &Path) -> PluginCatalogKind {
    if path.join("mcp.json").is_file() {
        PluginCatalogKind::Mcp
    } else if path.join("SKILL.md").is_file() || path.join("skill.toml").is_file() {
        PluginCatalogKind::Skill
    } else {
        PluginCatalogKind::Plugin
    }
}

/// Options for kind-specific install side effects.
#[derive(Debug, Clone, Copy)]
pub struct KindSideEffectOptions {
    /// When true, merge `mcp.json` into the global MCP config immediately.
    /// When false (default), only report that a merge is pending confirmation.
    pub apply_mcp: bool,
}

impl Default for KindSideEffectOptions {
    fn default() -> Self {
        Self { apply_mcp: false }
    }
}

/// Apply kind-specific post-install actions (skill store / MCP config / etc.).
/// Returns an updated human-readable hint when something was applied.
pub fn apply_kind_side_effects_at(
    data_dir: &Path,
    project_dir: &Path,
    installed_dir: &Path,
    kind: PluginCatalogKind,
) -> Option<String> {
    apply_kind_side_effects_with_options(
        data_dir,
        project_dir,
        installed_dir,
        kind,
        KindSideEffectOptions::default(),
    )
}

/// Same as [`apply_kind_side_effects_at`] with explicit options.
pub fn apply_kind_side_effects_with_options(
    data_dir: &Path,
    project_dir: &Path,
    installed_dir: &Path,
    kind: PluginCatalogKind,
    options: KindSideEffectOptions,
) -> Option<String> {
    match kind {
        PluginCatalogKind::Skill => import_skill_from_package(data_dir, project_dir, installed_dir),
        PluginCatalogKind::Mcp => {
            if options.apply_mcp {
                merge_mcp_from_package(data_dir, installed_dir)
            } else if installed_dir.join("mcp.json").is_file() {
                Some(format!(
                    "MCP package installed. Confirm merge of mcp.json into global config (pending: {}).",
                    installed_dir.join("mcp.json").display()
                ))
            } else {
                Some("MCP kind package installed; no mcp.json found to merge.".into())
            }
        }
        PluginCatalogKind::Plugin | PluginCatalogKind::Integration => {
            // tui.json is loaded lazily via plugin TUI extension API.
            None
        }
    }
}

/// Merge `mcp.json` from an installed package into the global NAVI config.
/// Call only after explicit user confirmation.
pub fn merge_mcp_from_package(data_dir: &Path, installed_dir: &Path) -> Option<String> {
    import_mcp_from_package(data_dir, installed_dir)
}

/// Whether an installed plugin directory has a pending `mcp.json` merge.
pub fn package_has_mcp_json(installed_dir: &Path) -> bool {
    installed_dir.join("mcp.json").is_file()
}

fn import_skill_from_package(
    data_dir: &Path,
    project_dir: &Path,
    installed_dir: &Path,
) -> Option<String> {
    let skill_md = installed_dir.join("SKILL.md");
    let skill_toml = installed_dir.join("skill.toml");
    let parsed = if skill_md.is_file() {
        let raw = fs::read_to_string(&skill_md).ok()?;
        navi_core::parse_skill_file(&skill_md, &raw, "Imported Skill").ok()?
    } else if skill_toml.is_file() {
        let raw = fs::read_to_string(&skill_toml).ok()?;
        navi_core::parse_skill_file(&skill_toml, &raw, "Imported Skill").ok()?
    } else {
        return Some(
            "Skill kind package installed; no SKILL.md/skill.toml found to import.".into(),
        );
    };

    let request = navi_core::SkillWriteRequest {
        id: parsed.id.unwrap_or_default(),
        name: parsed.name,
        description: parsed.description,
        version: parsed.version,
        author: parsed.author,
        tags: parsed.tags,
        requires: vec![],
        allow_tools: parsed.allow_tools,
        deny_tools: parsed.deny_tools,
        harness: false,
        pool: None,
        instructions: parsed.instructions,
        scope: navi_core::SkillWriteScope::User,
    };
    match navi_core::write_skill(&request, project_dir, data_dir) {
        Ok(result) => Some(format!(
            "Imported skill '{}' into skills/ (id={}, path={}).",
            result.skill.name,
            result.skill.id,
            result.path.display()
        )),
        Err(e) => Some(format!("Skill package installed; skill import failed: {e}")),
    }
}

fn import_mcp_from_package(data_dir: &Path, installed_dir: &Path) -> Option<String> {
    let mcp_path = installed_dir.join("mcp.json");
    if !mcp_path.is_file() {
        return Some("MCP kind package installed; no mcp.json found to merge.".into());
    }
    let raw = match fs::read_to_string(&mcp_path) {
        Ok(s) => s,
        Err(e) => {
            return Some(format!(
                "MCP package installed; failed to read mcp.json: {e}"
            ));
        }
    };
    let server: navi_core::McpServerConfig = match serde_json::from_str(&raw) {
        Ok(s) => s,
        Err(e) => {
            return Some(format!("MCP package installed; invalid mcp.json: {e}"));
        }
    };
    // Merge into the global user config TOML when present; otherwise write a
    // minimal global config with this MCP server.
    let global_path = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|base| base.join("navi").join("config.toml"));
    let Some(global_path) = global_path else {
        return Some(format!(
            "MCP package installed under {}; could not resolve global config path.",
            data_dir.display()
        ));
    };
    let mut config = if global_path.is_file() {
        std::fs::read_to_string(&global_path)
            .ok()
            .and_then(|s| toml::from_str::<navi_core::NaviConfig>(&s).ok())
            .unwrap_or_default()
    } else {
        navi_core::NaviConfig::default()
    };
    config.mcp.enabled = true;
    if let Some(existing) = config.mcp.servers.iter_mut().find(|s| s.id == server.id) {
        *existing = server.clone();
    } else {
        config.mcp.servers.push(server.clone());
    }
    match navi_core::save_global_config(&global_path, &config) {
        Ok(path) => Some(format!(
            "Merged MCP server '{}' into global config ({}).",
            server.id,
            path.display()
        )),
        Err(e) => Some(format!(
            "MCP package installed under {}; config merge failed: {e}",
            data_dir.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use navi_core::{LoadedConfig, NaviConfig};
    use navi_plugin_manifest::{PluginManifest, PluginMeta, RuntimeKind, ToolDef, ToolRisk};

    #[test]
    fn local_dev_install_accepts_unsigned_wasm_package() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = NaviConfig::default();
        config.registry.update_enabled = false;
        let loaded = LoadedConfig {
            config,
            global_config_path: Some(temp.path().join("config.toml")),
            project_config_path: None,
            data_dir: temp.path().to_path_buf(),
        };
        let engine = crate::NaviEngineBuilder::from_project(temp.path())
            .loaded_config(loaded)
            .build()
            .expect("engine");

        let pkg = temp.path().join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        let wasm = b"\0asm\x01\x00\x00\x00"; // minimal wasm magic (not a valid module for load)
        let hash = compute_wasm_hash(wasm);
        let manifest = PluginManifest {
            plugin: PluginMeta {
                id: "local-echo".into(),
                name: "Local Echo".into(),
                version: "0.1.0".into(),
                publisher: "gh:dev".into(),
                runtime: RuntimeKind::WasmComponent,
                entry: "plugin.wasm".into(),
                wasm_hash: hash,
                signature: "local-dev".into(),
                public_key: None,
                minimum_navi: "0.1.0".into(),
            },
            capabilities: vec![],
            tools: vec![ToolDef {
                id: "echo".into(),
                summary: "echo".into(),
                risk: ToolRisk::ReadOnly,
                input_schema: None,
                capabilities: vec![],
            }],
        };
        fs::write(
            pkg.join("plugin.toml"),
            toml::to_string(&manifest).expect("toml"),
        )
        .unwrap();
        fs::write(pkg.join("plugin.wasm"), wasm).unwrap();

        let result = engine
            .plugin_install_path(&pkg, true)
            .expect("LocalDev install");
        assert_eq!(result.id, "local-echo");
        assert_eq!(result.trust_level, "local-dev");
        assert!(temp.path().join("plugins/local-echo/plugin.wasm").is_file());

        let list = engine.plugin_list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].trust_level, "local-dev");
    }

    #[test]
    fn marketplace_discord_installs_as_mcp_kind_with_pending_merge() {
        let artifact = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../marketplace/artifacts/discord/0.1.0");
        if !artifact.join("plugin.wasm").is_file() || !artifact.join("mcp.json").is_file() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let mut config = NaviConfig::default();
        config.registry.update_enabled = false;
        let loaded = LoadedConfig {
            config,
            global_config_path: Some(temp.path().join("config.toml")),
            project_config_path: None,
            data_dir: temp.path().to_path_buf(),
        };
        let engine = crate::NaviEngineBuilder::from_project(temp.path())
            .loaded_config(loaded)
            .build()
            .expect("engine");

        assert_eq!(detect_package_kind(&artifact), PluginCatalogKind::Mcp);

        let result = engine
            .plugin_install_path_with_meta(
                &artifact,
                true,
                TrustLevel::Community,
                PluginCatalogKind::Mcp,
            )
            .expect("discord install");
        assert_eq!(result.id, "discord");
        assert_eq!(result.kind, "mcp");
        assert_eq!(result.trust_level, "community");
        // Default side effects leave MCP merge pending (apply_mcp: false).
        assert!(
            result.kind_hint.to_lowercase().contains("mcp")
                || result.kind_hint.to_lowercase().contains("pending")
                || result.kind_hint.to_lowercase().contains("confirm"),
            "hint={}",
            result.kind_hint
        );
        assert!(temp.path().join("plugins/discord/mcp.json").is_file());
        assert!(temp.path().join("plugins/discord/SKILL.md").is_file());
        assert!(package_has_mcp_json(&temp.path().join("plugins/discord")));

        // Explicit confirm path merges without requiring real Discord token.
        let msg = merge_mcp_from_package(temp.path(), &temp.path().join("plugins/discord"))
            .expect("merge hint");
        assert!(
            msg.contains("discord") || msg.to_lowercase().contains("merged"),
            "merge msg={msg}"
        );
    }

    #[test]
    fn marketplace_hello_echo_installs_and_registers_tool() {
        use crate::tooling::build_local_tooling;
        use navi_core::RuntimeComponents;

        // Vendored signed artifact (relative to crate → workspace).
        let artifact = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../marketplace/artifacts/hello-echo/0.1.0");
        if !artifact.join("plugin.wasm").is_file() {
            // Skip when running outside the monorepo layout.
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let mut config = NaviConfig::default();
        config.registry.update_enabled = false;
        let loaded = LoadedConfig {
            config,
            global_config_path: Some(temp.path().join("config.toml")),
            project_config_path: None,
            data_dir: temp.path().to_path_buf(),
        };
        let engine = crate::NaviEngineBuilder::from_project(temp.path())
            .loaded_config(loaded.clone())
            .build()
            .expect("engine");

        let result = engine
            .plugin_install_path_with_meta(
                &artifact,
                true,
                TrustLevel::Community,
                PluginCatalogKind::Plugin,
            )
            .expect("signed install");
        assert_eq!(result.id, "hello-echo");
        assert_eq!(result.trust_level, "community");
        assert!(temp.path().join("plugins/hello-echo/plugin.wasm").is_file());

        // After install, tooling load must register the namespaced echo tool.
        let tooling = build_local_tooling(
            &LoadedConfig {
                config: NaviConfig {
                    registry: {
                        let mut r = navi_core::config::types::RegistryConfig::default();
                        r.update_enabled = false;
                        r
                    },
                    ..Default::default()
                },
                global_config_path: Some(temp.path().join("config.toml")),
                project_config_path: None,
                data_dir: temp.path().to_path_buf(),
            },
            temp.path().to_path_buf(),
            &RuntimeComponents::default(),
        )
        .expect("tooling");

        let names: Vec<String> = tooling
            .tool_executor
            .definitions()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert!(
            names
                .iter()
                .any(|n| n == "plugin__hello-echo__echo" || n.contains("hello-echo")),
            "expected namespaced hello-echo tool, got {names:?}; warnings={:?}",
            tooling.warnings
        );

        // Invoke the WASM tool — proves runtime + brokers path works.
        let tool_name = names
            .iter()
            .find(|n| n.contains("hello-echo") && n.contains("echo"))
            .cloned()
            .expect("tool name");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(tooling.tool_executor.invoke_approved_with_event_tx(
            navi_core::ToolInvocation {
                id: "test-1".into(),
                tool_name: tool_name.clone(),
                input: serde_json::json!({"text": "ping"}),
            },
            None,
        ));
        assert!(result.ok, "tool invoke failed: {:?}", result.output);
        let out = result.output.to_string();
        assert!(
            out.contains("ping") || out.contains("text"),
            "unexpected tool output: {out}"
        );

        // tui.json extension commands surface
        let cmds = engine.list_tui_extension_commands().expect("ext cmds");
        assert!(
            cmds.iter().any(|c| c.id.contains("hello")
                || c.title.contains("Echo")
                || c.title.contains("Ping")),
            "expected tui.json command, got {cmds:?}"
        );
    }

    #[test]
    fn skill_package_install_imports_skill_md() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = NaviConfig::default();
        config.registry.update_enabled = false;
        config.skills.enabled = true;
        let loaded = LoadedConfig {
            config,
            global_config_path: Some(temp.path().join("config.toml")),
            project_config_path: None,
            data_dir: temp.path().to_path_buf(),
        };
        let engine = crate::NaviEngineBuilder::from_project(temp.path())
            .loaded_config(loaded)
            .build()
            .expect("engine");

        let pkg = temp.path().join("skill-pkg");
        fs::create_dir_all(&pkg).unwrap();
        let wasm = b"\0asm\x01\x00\x00\x00";
        let hash = compute_wasm_hash(wasm);
        let manifest = PluginManifest {
            plugin: PluginMeta {
                id: "hello-skill".into(),
                name: "Hello Skill Pack".into(),
                version: "0.1.0".into(),
                publisher: "gh:test".into(),
                runtime: RuntimeKind::WasmComponent,
                entry: "plugin.wasm".into(),
                wasm_hash: hash,
                signature: "local-dev".into(),
                public_key: None,
                minimum_navi: "0.1.0".into(),
            },
            capabilities: vec![],
            tools: vec![],
        };
        fs::write(pkg.join("plugin.toml"), toml::to_string(&manifest).unwrap()).unwrap();
        fs::write(pkg.join("plugin.wasm"), wasm).unwrap();
        fs::write(
            pkg.join("SKILL.md"),
            "---\nname: Hello Skill\ndescription: test\nversion: 0.1.0\ntags: [example]\n---\n\n# Body\nBe helpful.\n",
        )
        .unwrap();

        let result = engine.plugin_install_path(&pkg, true).expect("install");
        assert_eq!(result.kind, "skill");
        assert!(
            result.kind_hint.contains("Imported skill")
                || result.kind_hint.contains("skills/"),
            "hint={}",
            result.kind_hint
        );
        let skills = engine.list_skills().expect("list skills");
        assert!(
            skills
                .iter()
                .any(|s| s.name.contains("Hello") || s.id.contains("hello")),
            "skills={skills:?}"
        );
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
