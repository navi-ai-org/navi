//! HTTP fetcher for the remote registry files hosted in the NAVI registry DB repo.
//!
//! The registry database lives at <https://github.com/navi-ai-org/navi-registry>.
//! This fetcher pulls the manifest and per-provider JSON files from GitHub raw
//! content, verifying SHA-256 integrity hashes against the manifest.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::Path;

use super::types::{RegistryManifest, RegistryProvider};

/// Base URL for the NAVI registry database on GitHub. Uses `raw.githubusercontent.com`
/// for direct file access without the GitHub API rate limits.
const REGISTRY_BASE_URL: &str = "https://raw.githubusercontent.com/navi-ai-org/navi-registry/main";

/// Timeout for individual HTTP requests.
const FETCH_TIMEOUT_SECS: u64 = 15;

/// Fetches registry data from the remote NAVI repo.
pub struct RegistryFetcher {
    client: reqwest::Client,
}

impl RegistryFetcher {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
            .user_agent("navi-registry-fetcher/1.0")
            .build()
            .expect("failed to build HTTP client");
        Self { client }
    }

    /// Fetches the manifest from the remote registry.
    pub async fn fetch_manifest(&self) -> Result<RegistryManifest> {
        let url = format!("{REGISTRY_BASE_URL}/manifest.json");
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("failed to fetch manifest from {url}"))?;

        resp.error_for_status()
            .with_context(|| format!("manifest request failed: {url}"))?
            .json::<RegistryManifest>()
            .await
            .context("failed to parse manifest JSON")
    }

    /// Fetches a single provider JSON by id.
    pub async fn fetch_provider(
        &self,
        provider_id: &str,
        manifest: &RegistryManifest,
    ) -> Result<RegistryProvider> {
        let entry = manifest
            .providers
            .get(provider_id)
            .with_context(|| format!("provider '{provider_id}' not in manifest"))?;

        let url = format!("{REGISTRY_BASE_URL}/{}", entry.file);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("failed to fetch provider '{provider_id}' from {url}"))?;

        let text = resp
            .error_for_status()
            .with_context(|| format!("provider '{provider_id}' request failed: {url}"))?
            .text()
            .await
            .context("failed to read provider response body")?;

        // SHA-256 integrity check against the manifest hash.
        let hash = hex::encode(Sha256::digest(text.as_bytes()));
        if hash != entry.sha256 {
            anyhow::bail!(
                "provider '{provider_id}' integrity check failed: expected {}, got {}",
                entry.sha256,
                hash
            );
        }

        serde_json::from_str::<RegistryProvider>(&text)
            .with_context(|| format!("failed to parse provider '{provider_id}' JSON"))
    }

    /// Fetches all providers listed in the manifest.
    pub async fn fetch_all_providers(
        &self,
        manifest: &RegistryManifest,
    ) -> Result<Vec<RegistryProvider>> {
        let mut providers = Vec::new();
        for provider_id in manifest.providers.keys() {
            let provider = self.fetch_provider(provider_id, manifest).await?;
            providers.push(provider);
        }
        Ok(providers)
    }

    /// Returns the raw base URL (useful for tests).
    pub fn base_url(&self) -> &str {
        REGISTRY_BASE_URL
    }
}

impl Default for RegistryFetcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Loads all providers from a local registry directory.
///
/// The directory must contain `manifest.json` and the provider files referenced
/// by that manifest, matching the repository `registry/` layout.
pub fn load_local_registry(
    registry_dir: &Path,
) -> Result<(RegistryManifest, Vec<RegistryProvider>)> {
    let manifest_path = registry_dir.join("manifest.json");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let manifest = serde_json::from_str::<RegistryManifest>(&manifest_text)
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;

    let mut providers = Vec::new();
    for (provider_id, entry) in &manifest.providers {
        let provider_path = registry_dir.join(&entry.file);
        let provider_text = std::fs::read_to_string(&provider_path)
            .with_context(|| format!("failed to read {}", provider_path.display()))?;
        let provider = serde_json::from_str::<RegistryProvider>(&provider_text)
            .with_context(|| format!("failed to parse provider '{provider_id}' JSON"))?;
        providers.push(provider);
    }

    Ok((manifest, providers))
}

/// Syncs a local `registry/` directory into the SQLite store.
pub fn sync_local_registry(
    store: &super::store::RegistryStore,
    registry_dir: &Path,
) -> Result<bool> {
    let (manifest, providers) = load_local_registry(registry_dir)?;

    store.replace_all(&providers)?;
    store.save_manifest_meta(&manifest)?;

    tracing::info!(
        version = manifest.version,
        providers = providers.len(),
        models = providers.iter().map(|p| p.models.len()).sum::<usize>(),
        path = %registry_dir.display(),
        "registry cache updated from local directory"
    );

    Ok(true)
}

/// Syncs the remote registry into the local SQLite store.
///
/// Returns `true` if the store was updated.
pub async fn sync_registry(
    store: &super::store::RegistryStore,
    fetcher: &RegistryFetcher,
    force: bool,
) -> Result<bool> {
    // Check if we need to update.
    if !force {
        if let Some(updated_at) = store.manifest_updated_at()? {
            if let Ok(parsed) = parse_iso_timestamp(&updated_at) {
                if parsed.hours_ago < 24 && !store.is_empty()? {
                    tracing::debug!("registry cache is fresh, skipping fetch");
                    return Ok(false);
                }
            }
        }
    }

    tracing::info!("fetching remote registry manifest");
    let manifest = fetcher.fetch_manifest().await?;

    // Compare with stored manifest version.
    if !force {
        if let Some(stored_version) = store.manifest_version()? {
            if stored_version >= manifest.version && !store.is_empty()? {
                tracing::debug!(
                    stored = stored_version,
                    remote = manifest.version,
                    "registry manifest is up-to-date"
                );
                return Ok(false);
            }
        }
    }

    tracing::info!(
        version = manifest.version,
        providers = manifest.providers.len(),
        "fetching all provider definitions"
    );

    let providers = fetcher.fetch_all_providers(&manifest).await?;

    store.replace_all(&providers)?;
    store.save_manifest_meta(&manifest)?;

    tracing::info!(
        providers = providers.len(),
        models = providers.iter().map(|p| p.models.len()).sum::<usize>(),
        "registry cache updated"
    );

    Ok(true)
}

/// Minimal timestamp diff — no chrono dependency.
/// Parses ISO 8601 `YYYY-MM-DDTHH:MM:SSZ` and returns approximate hours since.
fn parse_iso_timestamp(iso: &str) -> Result<TimestampDiff> {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let date_part = iso.get(..10).context("invalid date format")?;
    let time_part = iso.get(11..19).context("invalid time format")?;

    let parts: Vec<u64> = date_part
        .split('-')
        .filter_map(|s| s.parse().ok())
        .collect();
    let time_parts: Vec<u64> = time_part
        .split(':')
        .filter_map(|s| s.parse().ok())
        .collect();

    if parts.len() < 3 || time_parts.len() < 3 {
        anyhow::bail!("invalid timestamp format: {iso}");
    }

    // Approximate epoch seconds (good enough for 24h staleness).
    let year = parts[0];
    let month = parts[1];
    let day = parts[2];
    let hours = time_parts[0];
    let minutes = time_parts[1];
    let seconds = time_parts[2];

    let days_since_epoch = (year - 1970) * 365 + (month - 1) * 30 + (day - 1);
    let epoch = days_since_epoch * 86400 + hours * 3600 + minutes * 60 + seconds;

    let diff_secs = now_secs.saturating_sub(epoch);
    Ok(TimestampDiff {
        hours_ago: diff_secs / 3600,
    })
}

struct TimestampDiff {
    hours_ago: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_iso_timestamp_recent() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        // Construct a timestamp ~1 hour ago.
        let hours = (now % 86400) / 3600;
        let days = now / 86400;
        let year = 1970 + days / 365;
        let month = 1 + (days % 365) / 30;
        let day = 1 + (days % 365) % 30;
        let iso = format!("{:04}-{:02}-{:02}T{:02}:00:00Z", year, month, day, hours);
        let diff = parse_iso_timestamp(&iso).expect("parse");
        assert!(
            diff.hours_ago <= 2,
            "expected <= 2h, got {}",
            diff.hours_ago
        );
    }

    #[test]
    fn parse_iso_timestamp_old() {
        let diff = parse_iso_timestamp("2020-01-01T00:00:00Z").expect("parse");
        assert!(diff.hours_ago > 1000);
    }

    #[test]
    fn load_local_registry_reads_manifest_and_providers() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let registry_dir = tempdir.path();
        std::fs::create_dir(registry_dir.join("providers")).expect("providers dir");
        std::fs::write(
            registry_dir.join("manifest.json"),
            r#"{
              "version": 1,
              "updated_at": "2026-01-01T00:00:00Z",
              "providers": {
                "local": {
                  "file": "providers/local.json",
                  "sha256": "",
                  "model_count": 1
                }
              }
            }"#,
        )
        .expect("manifest");
        std::fs::write(
            registry_dir.join("providers/local.json"),
            r#"{
              "id": "local",
              "label": "Local",
              "kind": "openai-chat-completions",
              "api_key_env": "LOCAL_API_KEY",
              "base_url": null,
              "models": [
                {
                  "name": "local-model",
                  "task_size": "large",
                  "context_window_tokens": 123456
                }
              ]
            }"#,
        )
        .expect("provider");

        let (manifest, providers) = load_local_registry(registry_dir).expect("load");

        assert_eq!(manifest.version, 1);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, "local");
        assert_eq!(providers[0].models[0].context_window_tokens, Some(123_456));
    }
}
