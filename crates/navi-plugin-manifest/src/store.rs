//! Paths and lockfile helpers for the installed WASM plugin store under NAVI's data dir.

use std::path::{Path, PathBuf};

use crate::lockfile::{LockEntry, Lockfile};
use crate::types::PluginManifest;

/// Subdirectory of `data_dir` that holds installed WASM plugin trees.
pub const INSTALLED_PLUGINS_SUBDIR: &str = "plugins";

/// Aggregate lockfile filename at the plugin store root.
pub const AGGREGATE_LOCKFILE_NAME: &str = "navi-plugins.lock";

/// Legacy per-plugin lockfile (pre-aggregate); migrated on load.
pub const LEGACY_LOCKFILE_NAME: &str = "navi-plugins.lock";

/// Root directory for installed WASM plugins: `<data_dir>/plugins/`.
pub fn installed_plugins_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(INSTALLED_PLUGINS_SUBDIR)
}

/// Per-plugin install directory: `<data_dir>/plugins/<plugin_id>/`.
pub fn installed_plugin_dir(data_dir: &Path, plugin_id: &str) -> PathBuf {
    installed_plugins_dir(data_dir).join(plugin_id)
}

/// Aggregate lockfile path: `<data_dir>/plugins/navi-plugins.lock`.
pub fn aggregate_lockfile_path(plugins_root: &Path) -> PathBuf {
    plugins_root.join(AGGREGATE_LOCKFILE_NAME)
}

/// Hash of declared capabilities (IDs joined).
pub fn capabilities_hash_from_manifest(manifest: &PluginManifest) -> String {
    let caps: Vec<&str> = manifest.capabilities.iter().map(|c| c.id()).collect();
    crate::compute_content_hash(&caps.join(","))
}

/// Hash of tool definitions.
pub fn tools_hash_from_manifest(manifest: &PluginManifest) -> String {
    let tools: Vec<String> = manifest
        .tools
        .iter()
        .map(|t| format!("{}:{:?}:{}", t.id, t.risk, t.summary))
        .collect();
    crate::compute_content_hash(&tools.join("\n"))
}

/// Build a lockfile entry from a manifest and approved capability IDs.
pub fn lock_entry_from_manifest(
    manifest: &PluginManifest,
    approved_capabilities: Vec<String>,
) -> LockEntry {
    LockEntry {
        id: manifest.plugin.id.clone(),
        version: manifest.plugin.version.clone(),
        publisher: manifest.plugin.publisher.clone(),
        wasm_hash: manifest.plugin.wasm_hash.clone(),
        capabilities_hash: capabilities_hash_from_manifest(manifest),
        tools_hash: tools_hash_from_manifest(manifest),
        approved_capabilities,
        approved_at: unix_timestamp_now(),
    }
}

/// Upsert a plugin entry into the aggregate lockfile at the store root.
pub fn upsert_aggregate_lock_entry(plugins_root: &Path, entry: LockEntry) -> Result<(), String> {
    std::fs::create_dir_all(plugins_root)
        .map_err(|e| format!("failed to create plugins directory: {e}"))?;
    let lockfile_path = aggregate_lockfile_path(plugins_root);
    let mut lockfile = Lockfile::load(&lockfile_path)?;
    lockfile.upsert(entry);
    lockfile.save(&lockfile_path)
}

/// Remove a plugin from the aggregate lockfile (e.g. on uninstall).
pub fn remove_aggregate_lock_entry(plugins_root: &Path, plugin_id: &str) -> Result<(), String> {
    let lockfile_path = aggregate_lockfile_path(plugins_root);
    let mut lockfile = Lockfile::load(&lockfile_path)?;
    lockfile.plugins.retain(|p| p.id != plugin_id);
    if lockfile_path.exists() || !lockfile.plugins.is_empty() {
        lockfile.save(&lockfile_path)?;
    }
    Ok(())
}

/// Merge legacy per-plugin `navi-plugins.lock` files into the aggregate lockfile.
///
/// Returns the number of legacy files migrated.
pub fn migrate_legacy_per_plugin_lockfiles(plugins_root: &Path) -> Result<usize, String> {
    if !plugins_root.is_dir() {
        return Ok(0);
    }

    let aggregate_path = aggregate_lockfile_path(plugins_root);
    let mut lockfile = Lockfile::load(&aggregate_path)?;
    let mut migrated = 0usize;

    for entry in std::fs::read_dir(plugins_root).map_err(|e| format!("read plugins dir: {e}"))? {
        let entry = entry.map_err(|e| format!("read dir entry: {e}"))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let legacy_path = path.join(LEGACY_LOCKFILE_NAME);
        if !legacy_path.is_file() {
            continue;
        }
        if !path.join("plugin.toml").is_file() {
            continue;
        }
        let legacy = Lockfile::load(&legacy_path)?;
        for plugin_entry in legacy.plugins {
            lockfile.upsert(plugin_entry);
        }
        std::fs::remove_file(&legacy_path)
            .map_err(|e| format!("remove legacy lockfile {}: {e}", legacy_path.display()))?;
        migrated += 1;
    }

    if migrated > 0 {
        lockfile.save(&aggregate_path)?;
    }

    Ok(migrated)
}

fn unix_timestamp_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PluginMeta, RuntimeKind};
    use tempfile::tempdir;

    fn sample_manifest(id: &str) -> PluginManifest {
        PluginManifest {
            plugin: PluginMeta {
                id: id.into(),
                name: id.into(),
                version: "1.0.0".into(),
                publisher: "gh:test".into(),
                runtime: RuntimeKind::WasmComponent,
                entry: "plugin.wasm".into(),
                wasm_hash: format!("sha256:{}", "a".repeat(64)),
                signature: "ed25519:00".into(),
                public_key: None,
                minimum_navi: "0.1.0".into(),
            },
            capabilities: vec![],
            tools: vec![],
        }
    }

    #[test]
    fn aggregate_lockfile_lives_at_plugins_root() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("plugins");
        assert_eq!(
            aggregate_lockfile_path(&root),
            root.join("navi-plugins.lock")
        );
    }

    #[test]
    fn migrate_legacy_per_plugin_lockfile() {
        let tmp = tempdir().unwrap();
        let root = installed_plugins_dir(tmp.path());
        let plugin_dir = root.join("demo");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(plugin_dir.join("plugin.toml"), "").unwrap();

        let mut legacy = Lockfile::default();
        legacy.upsert(lock_entry_from_manifest(&sample_manifest("demo"), vec![]));
        legacy.save(&plugin_dir.join(LEGACY_LOCKFILE_NAME)).unwrap();

        let n = migrate_legacy_per_plugin_lockfiles(&root).unwrap();
        assert_eq!(n, 1);
        assert!(!plugin_dir.join(LEGACY_LOCKFILE_NAME).exists());
        let aggregate = Lockfile::load(&aggregate_lockfile_path(&root)).unwrap();
        assert!(aggregate.find("demo").is_some());
    }
}
