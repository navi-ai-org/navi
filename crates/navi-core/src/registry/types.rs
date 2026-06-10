//! Types shared between the registry store, fetcher, and catalog integration.

use serde::{Deserialize, Serialize};

/// A single model entry in the remote registry JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryModel {
    pub name: String,
    pub task_size: String,
    pub context_window_tokens: Option<u64>,
}

/// A full provider entry as stored in `registry/providers/<id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryProvider {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    pub kind: String,
    pub api_key_env: String,
    pub base_url: Option<String>,
    #[serde(default)]
    pub models: Vec<RegistryModel>,
}

/// Top-level manifest file at `registry/manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryManifest {
    pub version: u32,
    pub updated_at: String,
    pub providers: std::collections::HashMap<String, ManifestProviderEntry>,
}

/// Per-provider entry inside the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestProviderEntry {
    pub file: String,
    pub sha256: String,
    pub model_count: usize,
}
