//! Plugin marketplace: fetch a catalog from a registry repository and stage artifacts for install.

use crate::types::PluginManifest;
use crate::{TrustLevel, compute_wasm_hash, parse_manifest, validate};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Default registry catalog URL (official NAVI marketplace on GitHub).
///
/// All marketplace packages (tools, skills, MCP adapters, messaging bots)
/// install as WASM plugin packages — nothing is hardcoded in the binary.
pub const DEFAULT_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/navi-ai-org/navi-marketplace/main/catalog.json";

/// Top-level marketplace catalog served from a registry repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCatalog {
    pub version: u32,
    #[serde(default)]
    pub updated_at: Option<String>,
    /// Optional human description of this catalog snapshot.
    #[serde(default)]
    pub description: Option<String>,
    pub plugins: Vec<PluginCatalogEntry>,
}

/// Package kind for marketplace UX (all kinds install as WASM plugins).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginCatalogKind {
    #[default]
    Plugin,
    Skill,
    Mcp,
    Integration,
}

/// One installable plugin published in the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCatalogEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub version: String,
    pub publisher: String,
    /// Directory path relative to the catalog URL (contains `plugin.toml` and WASM entry).
    pub artifact_dir: String,
    #[serde(default)]
    pub wasm_hash: Option<String>,
    /// Marketplace category: `plugin` | `skill` | `mcp` | `integration`.
    #[serde(default)]
    pub kind: PluginCatalogKind,
    /// Discovery tags (e.g. `discord`, `search`, `mcp`).
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Errors from marketplace operations.
#[derive(Debug, thiserror::Error)]
pub enum MarketplaceError {
    #[error("HTTP request failed: {0}")]
    Http(String),
    #[error("invalid catalog: {0}")]
    InvalidCatalog(String),
    #[error("plugin '{0}' not found in catalog")]
    NotFound(String),
    #[error("manifest error: {0}")]
    Manifest(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Resolve the registry URL from config override or default.
pub fn registry_url(configured: Option<&str>) -> &str {
    configured
        .filter(|url| !url.trim().is_empty())
        .unwrap_or(DEFAULT_REGISTRY_URL)
}

/// Parse catalog JSON.
pub fn parse_catalog(bytes: &[u8]) -> Result<PluginCatalog, MarketplaceError> {
    serde_json::from_slice(bytes).map_err(|e| MarketplaceError::InvalidCatalog(e.to_string()))
}

/// Fetch the plugin catalog from a registry URL (`https://` or `file://`).
pub async fn fetch_catalog(registry_url: &str) -> Result<PluginCatalog, MarketplaceError> {
    let bytes = fetch_bytes(registry_url).await?;
    parse_catalog(&bytes)
}

/// Find a catalog entry by plugin id.
pub fn find_catalog_entry<'a>(
    catalog: &'a PluginCatalog,
    plugin_id: &str,
) -> Result<&'a PluginCatalogEntry, MarketplaceError> {
    catalog
        .plugins
        .iter()
        .find(|p| p.id == plugin_id)
        .ok_or_else(|| MarketplaceError::NotFound(plugin_id.to_string()))
}

/// Search catalog entries (case-insensitive substring on id, name, description,
/// kind, and tags — so "mcp", "discord", "skill" find the right packages).
pub fn search_catalog<'a>(catalog: &'a PluginCatalog, query: &str) -> Vec<&'a PluginCatalogEntry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return catalog.plugins.iter().collect();
    }
    catalog
        .plugins
        .iter()
        .filter(|p| {
            let kind = match p.kind {
                PluginCatalogKind::Plugin => "plugin",
                PluginCatalogKind::Skill => "skill",
                PluginCatalogKind::Mcp => "mcp",
                PluginCatalogKind::Integration => "integration",
            };
            p.id.to_lowercase().contains(&q)
                || p.name.to_lowercase().contains(&q)
                || p.description.to_lowercase().contains(&q)
                || kind.contains(&q)
                || p.tags.iter().any(|t| t.to_lowercase().contains(&q))
        })
        .collect()
}

/// Staging directory for a plugin download: `<data_dir>/plugins/.staging/<id>/`.
pub fn plugin_staging_dir(data_dir: &Path, plugin_id: &str) -> PathBuf {
    data_dir.join("plugins").join(".staging").join(plugin_id)
}

/// Download `plugin.toml` and the WASM artifact into a staging directory.
pub async fn stage_plugin_from_catalog(
    registry_url: &str,
    entry: &PluginCatalogEntry,
    staging_dir: &Path,
) -> Result<(PluginManifest, PathBuf), MarketplaceError> {
    if staging_dir.exists() {
        std::fs::remove_dir_all(staging_dir)?;
    }
    std::fs::create_dir_all(staging_dir)?;

    let artifact_base = resolve_artifact_base(registry_url, &entry.artifact_dir);
    let manifest_url = join_url(&artifact_base, "plugin.toml");
    let manifest_bytes = fetch_bytes(&manifest_url).await?;
    let manifest_content = String::from_utf8(manifest_bytes)
        .map_err(|e| MarketplaceError::Manifest(format!("plugin.toml is not UTF-8: {e}")))?;
    let manifest =
        parse_manifest(&manifest_content).map_err(|e| MarketplaceError::Manifest(e.to_string()))?;
    validate(&manifest, TrustLevel::Community)
        .map_err(|e| MarketplaceError::Manifest(e.to_string()))?;

    if manifest.plugin.id != entry.id {
        return Err(MarketplaceError::Manifest(format!(
            "catalog id '{}' does not match manifest id '{}'",
            entry.id, manifest.plugin.id
        )));
    }

    let wasm_name = manifest.plugin.entry.clone();
    let wasm_url = join_url(&artifact_base, &wasm_name);
    let wasm_bytes = fetch_bytes(&wasm_url).await?;
    let actual_hash = compute_wasm_hash(&wasm_bytes);
    if actual_hash != manifest.plugin.wasm_hash {
        return Err(MarketplaceError::Manifest(format!(
            "WASM hash mismatch: declared {}, actual {}",
            manifest.plugin.wasm_hash, actual_hash
        )));
    }
    if let Some(expected) = &entry.wasm_hash
        && expected != &manifest.plugin.wasm_hash
    {
        return Err(MarketplaceError::Manifest(format!(
            "catalog wasm_hash {} does not match manifest {}",
            expected, manifest.plugin.wasm_hash
        )));
    }

    std::fs::write(staging_dir.join("plugin.toml"), manifest_content)?;
    std::fs::write(staging_dir.join(&wasm_name), wasm_bytes)?;

    Ok((manifest, staging_dir.to_path_buf()))
}

/// Download and stage a plugin by id from the registry.
pub async fn stage_plugin_by_id(
    registry_url: &str,
    plugin_id: &str,
    data_dir: &Path,
) -> Result<(PluginManifest, PathBuf), MarketplaceError> {
    let catalog = fetch_catalog(registry_url).await?;
    let entry = find_catalog_entry(&catalog, plugin_id)?;
    let staging = plugin_staging_dir(data_dir, plugin_id);
    stage_plugin_from_catalog(registry_url, entry, &staging).await
}

fn resolve_artifact_base(registry_url: &str, artifact_dir: &str) -> String {
    let artifact_dir = artifact_dir.trim_start_matches('/');
    if artifact_dir.starts_with("http://") || artifact_dir.starts_with("https://") {
        return artifact_dir.trim_end_matches('/').to_string();
    }
    let base = registry_parent_url(registry_url);
    join_url(&base, artifact_dir)
}

fn registry_parent_url(registry_url: &str) -> String {
    if let Some(stripped) = registry_url.strip_prefix("file://") {
        let path = Path::new(stripped);
        return path
            .parent()
            .map(|p| format!("file://{}", p.display()))
            .unwrap_or_else(|| "file://".to_string());
    }
    registry_url
        .rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_else(|| registry_url.to_string())
}

fn join_url(base: &str, segment: &str) -> String {
    let segment = segment.trim_start_matches('/');
    if base.ends_with('/') {
        format!("{base}{segment}")
    } else {
        format!("{base}/{segment}")
    }
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>, MarketplaceError> {
    if let Some(path) = url.strip_prefix("file://") {
        return std::fs::read(path).map_err(MarketplaceError::Io);
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| MarketplaceError::Http(e.to_string()))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| MarketplaceError::Http(e.to_string()))?;
    if !response.status().is_success() {
        return Err(MarketplaceError::Http(format!(
            "GET {} failed: {}",
            url,
            response.status()
        )));
    }
    response
        .bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| MarketplaceError::Http(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PluginMeta, RuntimeKind, sign_plugin_manifest_for_tests};
    use std::fs;
    use tempfile::tempdir;

    fn sample_catalog_json(artifact_dir: &str) -> String {
        format!(
            r#"{{
  "version": 1,
  "plugins": [{{
    "id": "echo",
    "name": "Echo",
    "description": "Echo test plugin",
    "version": "1.0.0",
    "publisher": "gh:test",
    "artifact_dir": "{artifact_dir}",
    "wasm_hash": null
  }}]
}}"#
        )
    }

    fn write_staged_plugin(dir: &Path, wasm: &[u8]) {
        use crate::PluginManifest;

        let mut manifest = PluginManifest {
            plugin: PluginMeta {
                id: "echo".into(),
                name: "Echo".into(),
                version: "1.0.0".into(),
                publisher: "gh:test".into(),
                runtime: RuntimeKind::WasmComponent,
                entry: "plugin.wasm".into(),
                wasm_hash: String::new(),
                signature: String::new(),
                public_key: None,
                minimum_navi: "0.1.0".into(),
            },
            capabilities: vec![],
            tools: vec![],
        };
        sign_plugin_manifest_for_tests(&mut manifest, wasm);
        fs::write(dir.join("plugin.toml"), toml::to_string(&manifest).unwrap()).unwrap();
        fs::write(dir.join("plugin.wasm"), wasm).unwrap();
    }

    #[test]
    fn parse_and_search_catalog() {
        let catalog = parse_catalog(sample_catalog_json("artifacts/echo").as_bytes()).unwrap();
        assert_eq!(catalog.plugins.len(), 1);
        let hits = search_catalog(&catalog, "echo");
        assert_eq!(hits.len(), 1);
        assert!(search_catalog(&catalog, "missing").is_empty());
    }

    #[tokio::test]
    async fn stage_plugin_from_file_registry() {
        let tmp = tempdir().unwrap();
        let artifact = tmp.path().join("artifacts/echo");
        fs::create_dir_all(&artifact).unwrap();
        write_staged_plugin(&artifact, b"wasm-echo");

        let catalog_path = tmp.path().join("catalog.json");
        fs::write(&catalog_path, sample_catalog_json("artifacts/echo")).unwrap();

        let catalog = fetch_catalog(&format!("file://{}", catalog_path.display()))
            .await
            .unwrap();
        let entry = find_catalog_entry(&catalog, "echo").unwrap();
        let staging = tmp.path().join("staging");
        let registry_url = format!("file://{}", catalog_path.display());
        let (manifest, path) = stage_plugin_from_catalog(&registry_url, entry, &staging)
            .await
            .unwrap();
        assert_eq!(manifest.plugin.id, "echo");
        assert!(path.join("plugin.toml").exists());
        assert!(path.join("plugin.wasm").exists());
    }
}
