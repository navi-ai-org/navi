//! HTTP fetcher for the remote registry files hosted in the NAVI registry DB repo.
//!
//! The registry database lives at <https://github.com/navi-ai-org/navi-registry>.
//! This fetcher pulls the manifest and per-provider JSON files from GitHub raw
//! content, verifying SHA-256 integrity hashes against the manifest.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::Path;

use super::types::{CanonicalModel, RegistryManifest, RegistryProvider, RegistryTranscriptionProvider};

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

        // Resolve `extends` using the embedded base catalog (remote bases/ is not
        // fetched independently). Overlay provider files from the same fetch set are
        // not available here; region variants rely on embedded bases.
        let bases = super::extends::base_map_from_embedded(
            super::embedded::embedded_base_files(),
            &[],
        )
        .unwrap_or_default();
        super::extends::parse_provider_json(&text, &bases)
            .with_context(|| format!("failed to parse provider '{provider_id}' JSON"))
    }

    /// Fetches a single canonical model JSON by id from `models/`.
    pub async fn fetch_canonical_model(
        &self,
        model_id: &str,
        manifest: &RegistryManifest,
    ) -> Result<CanonicalModel> {
        let entry = manifest
            .models
            .get(model_id)
            .with_context(|| format!("canonical model '{model_id}' not in manifest"))?;

        let url = format!("{REGISTRY_BASE_URL}/{}", entry.file);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("failed to fetch canonical model '{model_id}' from {url}"))?;

        let text = resp
            .error_for_status()
            .with_context(|| format!("canonical model '{model_id}' request failed: {url}"))?
            .text()
            .await
            .context("failed to read canonical model response body")?;

        let hash = hex::encode(Sha256::digest(text.as_bytes()));
        if hash != entry.sha256 {
            anyhow::bail!(
                "canonical model '{model_id}' integrity check failed: expected {}, got {}",
                entry.sha256,
                hash
            );
        }

        serde_json::from_str::<CanonicalModel>(&text)
            .with_context(|| format!("failed to parse canonical model '{model_id}' JSON"))
    }

    /// Fetches a single transcription provider JSON by id.
    pub async fn fetch_transcription_provider(
        &self,
        provider_id: &str,
        manifest: &RegistryManifest,
    ) -> Result<RegistryTranscriptionProvider> {
        let entry = manifest
            .transcription_providers
            .get(provider_id)
            .with_context(|| format!("transcription provider '{provider_id}' not in manifest"))?;

        let url = format!("{REGISTRY_BASE_URL}/{}", entry.file);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| {
                format!("failed to fetch transcription provider '{provider_id}' from {url}")
            })?;

        let text = resp
            .error_for_status()
            .with_context(|| {
                format!("transcription provider '{provider_id}' request failed: {url}")
            })?
            .text()
            .await
            .context("failed to read transcription provider response body")?;

        let hash = hex::encode(Sha256::digest(text.as_bytes()));
        if hash != entry.sha256 {
            anyhow::bail!(
                "transcription provider '{provider_id}' integrity check failed: expected {}, got {}",
                entry.sha256,
                hash
            );
        }

        serde_json::from_str::<RegistryTranscriptionProvider>(&text).with_context(|| {
            format!("failed to parse transcription provider '{provider_id}' JSON")
        })
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

    let bases = super::extends::load_local_base_map(registry_dir).unwrap_or_default();
    let mut providers = Vec::new();
    for (provider_id, entry) in &manifest.providers {
        let provider_path = registry_dir.join(&entry.file);
        let provider_text = std::fs::read_to_string(&provider_path)
            .with_context(|| format!("failed to read {}", provider_path.display()))?;
        let provider = super::extends::parse_provider_json(&provider_text, &bases)
            .with_context(|| format!("failed to parse provider '{provider_id}' JSON"))?;
        providers.push(provider);
    }

    // Resolve ref-based models against the local canonical model catalog.
    let models_dir = registry_dir.join("models");
    let catalog = load_local_model_catalog(&models_dir).unwrap_or_default();
    for provider in &mut providers {
        super::resolve::resolve_provider_refs(provider, &catalog);
    }

    Ok((manifest, providers))
}

/// Loads canonical models from a local `models/` directory.
fn load_local_model_catalog(
    models_dir: &Path,
) -> Result<super::resolve::ModelCatalog> {
    let mut catalog = std::collections::HashMap::new();
    if !models_dir.is_dir() {
        return Ok(catalog);
    }
    for entry in std::fs::read_dir(models_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let text = std::fs::read_to_string(&path)?;
            let model: super::types::CanonicalModel = serde_json::from_str(&text)
                .with_context(|| {
                    format!("failed to parse canonical model from {}", path.display())
                })?;
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            catalog.insert(id, model);
        }
    }
    Ok(catalog)
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
/// Fetches only providers whose SHA-256 hash differs from the cached version
/// (diff-based sync). Returns `true` if the store was updated.
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

    // Diff: figure out which providers actually changed.
    let mut to_fetch = Vec::new();
    let mut keep_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for (provider_id, entry) in &manifest.providers {
        keep_ids.insert(provider_id.as_str());
        let cached_sha = store.provider_sha256(provider_id)?;
        // Providers filled by live API sync keep their model lists. Remote
        // catalog refresh still runs on `force` and uses union-merge so new
        // metadata can land without wiping API-discovered models.
        let is_local_api_sync =
            cached_sha.as_deref() == Some(crate::registry::LOCAL_API_SYNC_SHA);
        if force || (!is_local_api_sync && cached_sha.as_deref() != Some(&entry.sha256)) {
            to_fetch.push(provider_id.as_str());
        }
    }

    // Diff transcription providers even when LLM providers are unchanged.
    let mut tx_to_fetch = Vec::new();
    let mut tx_keep: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (provider_id, entry) in &manifest.transcription_providers {
        tx_keep.insert(provider_id.as_str());
        let cached_sha = store.transcription_provider_sha256(provider_id)?;
        if force || cached_sha.as_deref() != Some(&entry.sha256) {
            tx_to_fetch.push(provider_id.as_str());
        }
    }

    // Diff canonical model catalog (models/<id>.json).
    let mut model_to_fetch = Vec::new();
    let mut model_keep: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (model_id, entry) in &manifest.models {
        model_keep.insert(model_id.as_str());
        let cached_sha = store.canonical_model_sha256(model_id)?;
        if force || cached_sha.as_deref() != Some(&entry.sha256) {
            model_to_fetch.push(model_id.as_str());
        }
    }

    if to_fetch.is_empty() && tx_to_fetch.is_empty() && model_to_fetch.is_empty() {
        // All hashes match — just update manifest meta and clean up stale providers.
        store.delete_providers_not_in(&keep_ids)?;
        store.delete_transcription_providers_not_in(&tx_keep)?;
        store.delete_canonical_models_not_in(&model_keep)?;
        store.save_manifest_meta(&manifest)?;
        tracing::debug!("all providers up-to-date, no fetch needed");
        return Ok(false);
    }

    tracing::info!(
        version = manifest.version,
        providers = manifest.providers.len(),
        changed = to_fetch.len(),
        models_changed = model_to_fetch.len(),
        "fetching changed provider/model definitions"
    );

    // Sync canonical models first so provider ref resolution sees the latest catalog.
    let mut models_updated = 0;
    for model_id in &model_to_fetch {
        let model = fetcher.fetch_canonical_model(model_id, &manifest).await?;
        let sha = &manifest.models[*model_id].sha256;
        store.upsert_canonical_model(model_id, &model, Some(sha))?;
        models_updated += 1;
    }
    store.delete_canonical_models_not_in(&model_keep)?;

    // Prefer cached catalog (now refreshed); fall back to embedded snapshot.
    let mut catalog = store
        .load_canonical_model_catalog()
        .unwrap_or_default();
    if catalog.is_empty() {
        catalog = super::embedded::embedded_model_catalog().unwrap_or_default();
    }

    let mut updated = 0;
    for provider_id in &to_fetch {
        let mut provider = fetcher.fetch_provider(provider_id, &manifest).await?;
        super::resolve::resolve_provider_refs(&mut provider, &catalog);
        let sha = &manifest.providers[*provider_id].sha256;
        // Union-merge so remote catalog refresh cannot wipe API-synced models.
        store.upsert_provider_union_models(&provider, Some(sha))?;
        updated += 1;
    }

    // Remove providers that were deleted from the remote registry.
    store.delete_providers_not_in(&keep_ids)?;

    // Sync remote transcription / dictation providers (diff by sha256).
    let mut tx_updated = 0;
    for provider_id in &tx_to_fetch {
        let entry = &manifest.transcription_providers[*provider_id];
        let provider = fetcher
            .fetch_transcription_provider(provider_id, &manifest)
            .await?;
        store.upsert_transcription_provider(&provider, Some(&entry.sha256))?;
        tx_updated += 1;
    }
    store.delete_transcription_providers_not_in(&tx_keep)?;

    store.save_manifest_meta(&manifest)?;
    // Persist the full manifest JSON so load_cached_registry and
    // check_registry_manifest see the correct version and hashes.
    super::update::save_registry_metadata(
        store,
        &manifest,
        Some(super::update::current_timestamp_secs()),
        Some(super::update::current_timestamp_secs()),
    )?;
    tracing::info!(
        version = manifest.version,
        providers_updated = updated,
        models_updated = models_updated,
        transcription_updated = tx_updated,
        "registry sync complete"
    );


    tracing::info!(
        updated = updated,
        total = manifest.providers.len(),
        transcription_updated = tx_updated,
        transcription_total = manifest.transcription_providers.len(),
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
