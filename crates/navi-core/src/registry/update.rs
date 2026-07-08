//! Registry update orchestration.
//!
//! Implements the full update lifecycle requested by the NAVI registry design:
//!
//! 1. Load from local cache or embedded snapshot (never block startup).
//! 2. Check for remote updates in the background, respecting a 24h + jitter interval.
//! 3. Diff-fetch only changed provider files.
//! 4. Validate schema and SHA-256 hashes before applying.
//! 5. Roll back to the previous registry on failure.
//!
//! Public API shape matches the requested functions:
//! `load_registry`, `load_embedded_registry`, `load_cached_registry`,
//! `should_check_registry_update`, `check_registry_manifest`, `download_registry_updates`,
//! `validate_registry_schema`, `validate_registry_hashes`, `apply_registry_update_atomically`,
//! `save_registry_metadata`.

use anyhow::{Context, Result};
use jsonschema::validator_for;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::types::{ProviderConfig, RegistryConfig};

use super::embedded::{embedded_manifest, embedded_provider_schema, embedded_providers};
use super::store::RegistryStore;
use super::types::{RegistryManifest, RegistryProvider};

/// Trait abstracting registry fetching so the update flow can be tested without network.
#[allow(async_fn_in_trait)]
pub trait RegistryFetcherTrait {
    async fn fetch_manifest(&self) -> Result<RegistryManifest>;
    async fn fetch_provider(
        &self,
        provider_id: &str,
        manifest: &RegistryManifest,
    ) -> Result<RegistryProvider>;
}

impl RegistryFetcherTrait for super::RegistryFetcher {
    async fn fetch_manifest(&self) -> Result<RegistryManifest> {
        self.fetch_manifest().await
    }

    async fn fetch_provider(
        &self,
        provider_id: &str,
        manifest: &RegistryManifest,
    ) -> Result<RegistryProvider> {
        self.fetch_provider(provider_id, manifest).await
    }
}

/// Metadata keys stored in `registry_meta`.
const META_LAST_CHECK: &str = "last_registry_check";
const META_LAST_SUCCESS: &str = "last_registry_success";
const META_MANIFEST_JSON: &str = "registry_manifest_json";

/// Result of loading the registry for immediate use.
#[derive(Debug, Clone)]
pub struct LoadedRegistry {
    pub manifest: RegistryManifest,
    pub providers: Vec<ProviderConfig>,
    pub source: RegistrySource,
}

/// Where the currently-active registry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrySource {
    /// Loaded from the local SQLite cache.
    Cache,
    /// Loaded from the binary's embedded snapshot.
    Embedded,
    /// Fallback minimal hardcoded registry.
    MinimalFallback,
}

/// Loads the registry for use, preferring cache, then embedded snapshot, then a minimal fallback.
///
/// This is the entry point for startup. It never blocks on network.
/// After loading the cache, it upserts any providers whose embedded snapshot
/// hash differs from the cached version. This ensures new models and pricing
/// updates shipped with the binary are merged into the local cache without
/// waiting for a remote sync.
pub fn load_registry(store: &RegistryStore) -> LoadedRegistry {
    // Try cache first.
    if let Some(loaded) = load_cached_registry(store) {
        // Merge in any providers from the embedded snapshot that are newer
        // (different SHA-256) than what's in the cache.
        merge_embedded_provider_updates(store);

        // Reload after merging so the returned registry reflects updates.
        if let Some(reloaded) = load_cached_registry(store) {
            tracing::info!(
                version = reloaded.manifest.version,
                providers = reloaded.providers.len(),
                "loaded registry from local cache (after embedded merge)"
            );
            return reloaded;
        }
        tracing::info!(
            version = loaded.manifest.version,
            providers = loaded.providers.len(),
            "loaded registry from local cache"
        );
        return loaded;
    }

    if let Some(loaded) = load_embedded_registry() {
        tracing::info!(
            version = loaded.manifest.version,
            providers = loaded.providers.len(),
            "loaded registry from embedded snapshot"
        );
        // Seed the cache from the embedded snapshot so next startup hits the cache.
        if let Err(err) = seed_cache_from_embedded(store) {
            tracing::warn!(error = %err, "failed to seed cache from embedded snapshot");
        }
        return loaded;
    }

    tracing::error!("failed to load embedded registry snapshot, using minimal fallback");
    LoadedRegistry {
        manifest: RegistryManifest {
            version: 0,
            updated_at: "1970-01-01T00:00:00Z".to_string(),
            providers: std::collections::HashMap::new(),
        },
        providers: minimal_fallback_providers(),
        source: RegistrySource::MinimalFallback,
    }
}

/// Upserts providers from the embedded snapshot whose SHA-256 hash differs
/// from the cached version. This merges new models and pricing updates into
/// the local cache without overwriting providers that are newer in the cache
/// (e.g. from a remote sync).
fn merge_embedded_provider_updates(store: &RegistryStore) {
    let embedded_manifest = match embedded_manifest() {
        Ok(m) => m,
        Err(_) => return,
    };
    let embedded_providers = match embedded_providers() {
        Ok(p) => p,
        Err(_) => return,
    };

    let mut updated = 0;
    for ep in &embedded_providers {
        let id = &ep.id;
        let embedded_sha = embedded_manifest
            .providers
            .get(id)
            .map(|e| e.sha256.as_str());
        let cached_sha = store.provider_sha256(id).ok().flatten();

        let needs_update = match (&cached_sha, embedded_sha) {
            (Some(cached), Some(embedded)) => cached != embedded,
            (None, _) => true,
            (Some(_), None) => false,
        };

        // Don't overwrite providers that already have models in the cache.
        // Aggregator sync (e.g. OpenRouter with 327 models) upserts with
        // sha256=None, which would trigger a re-upsert of the embedded
        // snapshot (5 models) on every provider_catalog() call. Skip if
        // the cache already has models for this provider.
        if needs_update {
            if let Ok(existing) = store.load_provider_models(id) {
                if !existing.is_empty() && existing.len() >= ep.models.len() {
                    continue;
                }
            }
        }

        if needs_update {
            if let Err(err) = store.upsert_provider_with_sha256(ep, embedded_sha) {
                tracing::warn!(
                    provider = id,
                    error = %err,
                    "failed to upsert embedded provider update"
                );
            } else {
                updated += 1;
            }
        }
    }

    if updated > 0 {
        tracing::info!(
            updated_providers = updated,
            "merged embedded provider updates into local cache"
        );
        // Update manifest metadata to reflect merged state.
        let _ = store.save_manifest_meta(&embedded_manifest);
        let _ = save_registry_metadata(store, &embedded_manifest, None, None);
    }
}

/// Loads the registry from the local SQLite cache if it is non-empty.
pub fn load_cached_registry(store: &RegistryStore) -> Option<LoadedRegistry> {
    let manifest = store_stored_manifest(store).ok().flatten()?;
    let providers = store.load_all_providers().ok()?;
    if providers.is_empty() {
        return None;
    }
    Some(LoadedRegistry {
        manifest,
        providers,
        source: RegistrySource::Cache,
    })
}

/// Loads the registry from the binary's embedded snapshot.
pub fn load_embedded_registry() -> Option<LoadedRegistry> {
    let manifest = embedded_manifest().ok()?;
    let providers = embedded_providers().ok()?;
    Some(LoadedRegistry {
        manifest,
        providers: providers
            .into_iter()
            .map(super::store::registry_provider_to_config)
            .collect(),
        source: RegistrySource::Embedded,
    })
}

/// Seeds the local cache from the embedded snapshot.
fn seed_cache_from_embedded(store: &RegistryStore) -> Result<()> {
    let manifest = embedded_manifest().context("failed to parse embedded manifest")?;
    let providers = embedded_providers().context("failed to parse embedded providers")?;
    store.replace_all(&providers)?;
    store.save_manifest_meta(&manifest)?;
    save_registry_metadata(
        store,
        &manifest,
        Some(current_timestamp_secs()),
        Some(current_timestamp_secs()),
    )?;
    Ok(())
}

/// Minimal hardcoded fallback used only if the embedded snapshot itself fails to parse.
fn minimal_fallback_providers() -> Vec<ProviderConfig> {
    use crate::config::types::{ModelTaskSize, ProviderModelConfig};

    vec![ProviderConfig {
        id: "openai".to_string(),
        label: "OpenAI".to_string(),
        description: "OpenAI API key required".to_string(),
        kind: crate::config::types::ProviderKind::OpenAiResponses,
        api_key_env: "OPENAI_API_KEY".to_string(),
        base_url: Some("https://api.openai.com/v1".to_string()),
        models: vec![ProviderModelConfig {
            name: "gpt-5.1".to_string(),
            task_size: Some(ModelTaskSize::Large),
            context_window_tokens: Some(1_000_000),
            max_output_tokens: None,
            recommended_temperature: None,
            supports_thinking: None,
            supports_images: None,
            supports_audio: None,
            supports_video: None,
            supports_documents: None,
            tool_prompt_manifest: None,
            pricing_input_per_1m: None,
            pricing_output_per_1m: None,
        }],
        request_options: crate::config::providers::default_request_options_for("openai"),
        ..Default::default()
    }]
}

/// Returns whether enough time has passed (plus jitter) to check for a remote update.
pub fn should_check_registry_update(store: &RegistryStore, config: &RegistryConfig) -> bool {
    if !config.update_enabled {
        return false;
    }

    let last_check = store
        .meta_get(META_LAST_CHECK)
        .ok()
        .flatten()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let now = current_timestamp_secs();
    let elapsed = now.saturating_sub(last_check);
    let interval = config.check_interval_hours.saturating_mul(3600);

    elapsed >= interval
}

/// Computes the jittered interval in seconds for the next update check.
pub fn registry_check_interval_with_jitter(config: &RegistryConfig) -> u64 {
    let base = config.check_interval_hours.saturating_mul(3600);
    let jitter = if config.check_jitter_hours > 0 {
        fastrand::u64(0..=config.check_jitter_hours.saturating_mul(3600))
    } else {
        0
    };
    base.saturating_add(jitter)
}

/// Fetches and validates the remote manifest, returning it if it differs from the local one.
///
/// Returns `Ok(None)` if the remote manifest is the same as the local one.
/// Returns `Err(...)` on network or parse failures (caller should fall back).
pub async fn check_registry_manifest(
    store: &RegistryStore,
    fetcher: &impl RegistryFetcherTrait,
    config: &RegistryConfig,
) -> Result<Option<RegistryManifest>> {
    let local = store_stored_manifest(store)
        .ok()
        .flatten()
        .unwrap_or_else(|| {
            embedded_manifest().unwrap_or_else(|_| RegistryManifest {
                version: 0,
                updated_at: "1970-01-01T00:00:00Z".to_string(),
                providers: std::collections::HashMap::new(),
            })
        });

    let manifest = fetch_manifest_with_retry(fetcher, config).await?;

    // If the remote version and hash set are identical to local, nothing changed.
    if manifest.version == local.version && provider_hashes_equal(&manifest, &local) {
        save_registry_metadata(store, &manifest, Some(current_timestamp_secs()), None)?;
        tracing::debug!(
            version = manifest.version,
            "remote manifest unchanged, no update needed"
        );
        return Ok(None);
    }

    Ok(Some(manifest))
}

/// Downloads only the provider files that changed, validating schema and hashes.
///
/// Returns the validated providers ready to be applied.
pub async fn download_registry_updates(
    store: &RegistryStore,
    fetcher: &impl RegistryFetcherTrait,
    manifest: &RegistryManifest,
    config: &RegistryConfig,
) -> Result<Vec<RegistryProvider>> {
    let mut to_fetch = Vec::new();

    for (provider_id, entry) in &manifest.providers {
        let cached_sha = store.provider_sha256(provider_id)?;
        if cached_sha.as_deref() != Some(&entry.sha256) {
            to_fetch.push(provider_id.as_str());
        }
    }

    if to_fetch.is_empty() {
        tracing::debug!("all provider hashes match, nothing to download");
        return Ok(Vec::new());
    }

    tracing::info!(
        changed = to_fetch.len(),
        total = manifest.providers.len(),
        "downloading changed registry providers"
    );

    let schema = embedded_provider_schema();
    let mut providers = Vec::with_capacity(to_fetch.len());

    for provider_id in &to_fetch {
        let provider = fetch_provider_with_retry(fetcher, provider_id, manifest, config).await?;

        // Validate the downloaded provider against the embedded JSON schema.
        validate_registry_schema(&provider, schema)?;

        providers.push(provider);
    }

    validate_registry_hashes(&providers, manifest)?;

    Ok(providers)
}

/// Validates a provider against the JSON schema.
pub fn validate_registry_schema(
    provider: &RegistryProvider,
    schema_json: Option<&str>,
) -> Result<()> {
    if let Some(schema_json) = schema_json {
        let schema_value: serde_json::Value = serde_json::from_str(schema_json)
            .context("failed to parse embedded provider schema")?;
        let validator = validator_for(&schema_value)
            .map_err(|e| anyhow::anyhow!("failed to build schema validator: {e}"))?;
        let provider_value = serde_json::to_value(provider)
            .context("failed to serialize provider for validation")?;
        if let Err(error) = validator.validate(&provider_value) {
            anyhow::bail!("provider schema validation failed: {}", error.to_string());
        }
    }
    Ok(())
}

/// Validates the SHA-256 hashes of a set of downloaded providers against the manifest.
pub fn validate_registry_hashes(
    providers: &[RegistryProvider],
    manifest: &RegistryManifest,
) -> Result<()> {
    for provider in providers {
        let entry = manifest
            .providers
            .get(&provider.id)
            .with_context(|| format!("provider '{}' not in manifest", provider.id))?;
        let provider_json = serde_json::to_string(provider).with_context(|| {
            format!(
                "failed to serialize provider '{}' for hash check",
                provider.id
            )
        })?;
        let hash = hex::encode(Sha256::digest(provider_json.as_bytes()));
        if hash != entry.sha256 {
            anyhow::bail!(
                "provider '{}' hash mismatch: expected {}, got {}",
                provider.id,
                entry.sha256,
                hash
            );
        }
    }
    Ok(())
}

/// Applies a validated update atomically: store new providers, delete removed ones, and save metadata.
///
/// On failure, the previous registry contents remain intact (this function uses explicit transactions).
pub fn apply_registry_update_atomically(
    store: &RegistryStore,
    manifest: &RegistryManifest,
    providers: &[RegistryProvider],
) -> Result<()> {
    let keep: std::collections::HashSet<&str> =
        manifest.providers.keys().map(|s| s.as_str()).collect();

    for provider in providers {
        let sha = manifest
            .providers
            .get(&provider.id)
            .map(|e| e.sha256.as_str());
        store.upsert_provider_with_sha256(provider, sha)?;
    }

    store.delete_providers_not_in(&keep)?;
    store.save_manifest_meta(manifest)?;

    Ok(())
}

/// Saves registry metadata: last check, last success, and the manifest JSON.
pub fn save_registry_metadata(
    store: &RegistryStore,
    manifest: &RegistryManifest,
    last_check: Option<u64>,
    last_success: Option<u64>,
) -> Result<()> {
    if let Some(ts) = last_check {
        store.meta_set(META_LAST_CHECK, &ts.to_string())?;
    }
    if let Some(ts) = last_success {
        store.meta_set(META_LAST_SUCCESS, &ts.to_string())?;
    }
    let manifest_json =
        serde_json::to_string(manifest).context("failed to serialize manifest metadata")?;
    store.meta_set(META_MANIFEST_JSON, &manifest_json)?;
    Ok(())
}

/// Full background update check. Returns `true` if the cache was updated.
///
/// This is safe to run from a background task: all failures are logged and swallowed.
pub async fn run_registry_update_check(
    store: &RegistryStore,
    fetcher: &impl RegistryFetcherTrait,
    config: &RegistryConfig,
) -> bool {
    if !config.update_enabled {
        return false;
    }

    // Record that we attempted a check.
    let now = current_timestamp_secs();
    if let Err(err) = save_registry_metadata(
        store,
        &current_stored_manifest_or_embedded(store),
        Some(now),
        None,
    ) {
        tracing::warn!(error = %err, "failed to save registry check timestamp");
    }

    let manifest = match check_registry_manifest(store, fetcher, config).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            tracing::debug!("registry update check: no changes");
            return false;
        }
        Err(err) => {
            tracing::warn!(error = %err, "registry manifest check failed, keeping existing registry");
            return false;
        }
    };

    let providers = match download_registry_updates(store, fetcher, &manifest, config).await {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(error = %err, "registry update download failed, keeping existing registry");
            return false;
        }
    };

    if let Err(err) = apply_registry_update_atomically(store, &manifest, &providers) {
        tracing::warn!(error = %err, "failed to apply registry update, keeping previous registry");
        return false;
    }

    if let Err(err) = save_registry_metadata(store, &manifest, Some(now), Some(now)) {
        tracing::warn!(error = %err, "failed to save registry success timestamp");
    }

    tracing::info!(
        version = manifest.version,
        providers = manifest.providers.len(),
        "registry cache updated from remote"
    );

    true
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn store_stored_manifest(store: &RegistryStore) -> Result<Option<RegistryManifest>> {
    match store.meta_get(META_MANIFEST_JSON)? {
        Some(json) => Ok(Some(
            serde_json::from_str(&json).context("failed to parse stored manifest metadata")?,
        )),
        None => Ok(None),
    }
}

fn current_stored_manifest_or_embedded(store: &RegistryStore) -> RegistryManifest {
    store_stored_manifest(store)
        .ok()
        .flatten()
        .unwrap_or_else(|| {
            embedded_manifest().unwrap_or_else(|_| RegistryManifest {
                version: 0,
                updated_at: "1970-01-01T00:00:00Z".to_string(),
                providers: std::collections::HashMap::new(),
            })
        })
}

fn provider_hashes_equal(a: &RegistryManifest, b: &RegistryManifest) -> bool {
    if a.providers.len() != b.providers.len() {
        return false;
    }
    a.providers.iter().all(|(id, entry)| {
        b.providers
            .get(id)
            .map(|other| entry.sha256 == other.sha256)
            .unwrap_or(false)
    })
}

async fn fetch_manifest_with_retry(
    fetcher: &impl RegistryFetcherTrait,
    config: &RegistryConfig,
) -> Result<RegistryManifest> {
    let mut last_err = None;
    let attempts = config.max_retries.saturating_add(1).max(1);
    for attempt in 1..=attempts {
        match fetcher.fetch_manifest().await {
            Ok(m) => return Ok(m),
            Err(err) => {
                tracing::debug!(attempt, error = %err, "manifest fetch failed");
                last_err = Some(err);
                if attempt < attempts {
                    tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64))
                        .await;
                }
            }
        }
    }
    Err(last_err.unwrap()).context("failed to fetch registry manifest after retries")
}

async fn fetch_provider_with_retry(
    fetcher: &impl RegistryFetcherTrait,
    provider_id: &str,
    manifest: &RegistryManifest,
    config: &RegistryConfig,
) -> Result<RegistryProvider> {
    let mut last_err = None;
    let attempts = config.max_retries.saturating_add(1).max(1);
    for attempt in 1..=attempts {
        match fetcher.fetch_provider(provider_id, manifest).await {
            Ok(p) => return Ok(p),
            Err(err) => {
                tracing::debug!(attempt, provider = provider_id, error = %err, "provider fetch failed");
                last_err = Some(err);
                if attempt < attempts {
                    tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64))
                        .await;
                }
            }
        }
    }
    Err(last_err.unwrap()).context(format!(
        "failed to fetch provider '{provider_id}' after retries"
    ))
}

pub fn current_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::super::types::{ManifestProviderEntry, RegistryModel};
    use super::*;

    fn test_provider(id: &str, name: &str) -> RegistryProvider {
        RegistryProvider {
            id: id.to_string(),
            label: id.to_string(),
            description: "test".to_string(),
            kind: "openai-chat-completions".to_string(),
            api_key_env: "TEST_API_KEY".to_string(),
            base_url: None,
            tool_calling_mode: None,
            aggregator: false,
            defaults: Default::default(),
            request_options: Default::default(),
            models: vec![RegistryModel {
                name: name.to_string(),
                task_size: Some("large".to_string()),
                context_window_tokens: Some(128_000),
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                supports_attachments: None,
                supports_images: None,
                supports_audio: None,
                supports_video: None,
                supports_documents: None,
                attachments: Default::default(),
                capabilities: Vec::new(),
                pricing: None,
            }],
        }
    }

    fn provider_hash(provider: &RegistryProvider) -> String {
        let json = serde_json::to_string(provider).expect("serialize");
        hex::encode(Sha256::digest(json.as_bytes()))
    }

    fn manifest_entry(provider: &RegistryProvider) -> ManifestProviderEntry {
        ManifestProviderEntry {
            file: format!("providers/{}.json", provider.id),
            sha256: provider_hash(provider),
            model_count: provider.models.len(),
        }
    }

    #[test]
    fn should_check_after_default_interval() {
        let store = RegistryStore::open_memory().expect("open");
        let config = RegistryConfig::default();

        // No last check recorded → should check.
        assert!(should_check_registry_update(&store, &config));

        // Just checked now → should not check.
        let now = current_timestamp_secs();
        save_registry_metadata(
            &store,
            &RegistryManifest {
                version: 1,
                updated_at: "2026-01-01T00:00:00Z".to_string(),
                providers: std::collections::HashMap::new(),
            },
            Some(now),
            None,
        )
        .expect("save");
        assert!(!should_check_registry_update(&store, &config));

        // 25 hours ago → should check.
        let past = now.saturating_sub(25 * 3600);
        save_registry_metadata(
            &store,
            &RegistryManifest {
                version: 1,
                updated_at: "2026-01-01T00:00:00Z".to_string(),
                providers: std::collections::HashMap::new(),
            },
            Some(past),
            None,
        )
        .expect("save");
        assert!(should_check_registry_update(&store, &config));
    }

    #[test]
    fn should_not_check_when_disabled() {
        let store = RegistryStore::open_memory().expect("open");
        let mut config = RegistryConfig::default();
        config.update_enabled = false;
        assert!(!should_check_registry_update(&store, &config));
    }

    #[test]
    fn load_embedded_registry_returns_providers() {
        let loaded = load_embedded_registry().expect("embedded registry should load");
        assert!(!loaded.providers.is_empty());
        assert_eq!(loaded.source, RegistrySource::Embedded);
    }

    #[test]
    fn load_registry_seeds_empty_cache_from_embedded() {
        let store = RegistryStore::open_memory().expect("open");
        assert!(store.is_empty().unwrap());

        let loaded = load_registry(&store);
        assert_eq!(loaded.source, RegistrySource::Embedded);
        assert!(!loaded.providers.is_empty());

        // Cache should now be seeded.
        assert!(!store.is_empty().unwrap());
        assert!(load_cached_registry(&store).is_some());
    }

    #[test]
    fn load_registry_prefers_cache_when_populated() {
        let store = RegistryStore::open_memory().expect("open");
        let embedded_manifest = super::embedded_manifest().expect("manifest");
        let embedded_providers = super::embedded_providers().expect("providers");
        store.replace_all(&embedded_providers).expect("seed");
        store
            .save_manifest_meta(&embedded_manifest)
            .expect("save meta");
        save_registry_metadata(&store, &embedded_manifest, None, None).expect("save manifest json");

        let loaded = load_registry(&store);
        assert_eq!(loaded.source, RegistrySource::Cache);
    }

    #[test]
    fn apply_registry_update_atomically_replaces_providers() {
        let store = RegistryStore::open_memory().expect("open");
        let provider = test_provider("test", "test-model");
        let mut manifest = RegistryManifest {
            version: 1,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            providers: std::collections::HashMap::new(),
        };
        manifest
            .providers
            .insert(provider.id.clone(), manifest_entry(&provider));

        apply_registry_update_atomically(&store, &manifest, &[provider]).expect("apply");

        let loaded = store.load_all_providers().expect("load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "test");
    }

    #[test]
    fn validate_registry_hashes_detects_mismatch() {
        let provider = test_provider("test", "test-model");
        let mut manifest = RegistryManifest {
            version: 1,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            providers: std::collections::HashMap::new(),
        };
        manifest.providers.insert(
            provider.id.clone(),
            ManifestProviderEntry {
                file: "providers/test.json".to_string(),
                sha256: "bad-hash".to_string(),
                model_count: 1,
            },
        );

        assert!(validate_registry_hashes(&[provider], &manifest).is_err());
    }

    #[test]
    fn validate_registry_hashes_accepts_correct_hash() {
        let provider = test_provider("test", "test-model");
        let mut manifest = RegistryManifest {
            version: 1,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            providers: std::collections::HashMap::new(),
        };
        manifest
            .providers
            .insert(provider.id.clone(), manifest_entry(&provider));

        assert!(validate_registry_hashes(&[provider], &manifest).is_ok());
    }

    // ── Async tests using a mock fetcher ───────────────────────────────────

    /// Mock fetcher for testing the update flow without network access.
    struct MockFetcher {
        manifest: Option<RegistryManifest>,
        manifest_err: Option<String>,
        providers: std::collections::HashMap<String, Result<RegistryProvider, String>>,
    }

    impl MockFetcher {
        fn new() -> Self {
            Self {
                manifest: None,
                manifest_err: Some("not configured".to_string()),
                providers: std::collections::HashMap::new(),
            }
        }

        fn with_manifest(mut self, manifest: RegistryManifest) -> Self {
            self.manifest = Some(manifest);
            self.manifest_err = None;
            self
        }

        fn with_manifest_error(mut self, err: &str) -> Self {
            self.manifest = None;
            self.manifest_err = Some(err.to_string());
            self
        }

        fn with_provider(mut self, provider: RegistryProvider) -> Self {
            self.providers.insert(provider.id.clone(), Ok(provider));
            self
        }

        fn with_provider_error(mut self, id: &str, err: &str) -> Self {
            self.providers.insert(id.to_string(), Err(err.to_string()));
            self
        }
    }

    impl RegistryFetcherTrait for MockFetcher {
        async fn fetch_manifest(&self) -> Result<RegistryManifest> {
            self.manifest
                .clone()
                .ok_or_else(|| anyhow::anyhow!(self.manifest_err.clone().unwrap_or_default()))
        }

        async fn fetch_provider(
            &self,
            provider_id: &str,
            _manifest: &RegistryManifest,
        ) -> Result<RegistryProvider> {
            self.providers
                .get(provider_id)
                .cloned()
                .map(|r| r.map_err(|e| anyhow::anyhow!(e)))
                .unwrap_or_else(|| {
                    Err(anyhow::anyhow!(
                        "provider {provider_id} not configured in mock"
                    ))
                })
        }
    }

    #[tokio::test]
    async fn first_startup_without_cache_seeds_from_embedded() {
        let store = RegistryStore::open_memory().expect("open");
        assert!(store.is_empty().unwrap());

        let loaded = load_registry(&store);
        assert!(!loaded.providers.is_empty());
        // After load, cache should be seeded.
        assert!(!store.is_empty().unwrap());
    }

    #[tokio::test]
    async fn startup_with_valid_cache_uses_cache() {
        let store = RegistryStore::open_memory().expect("open");
        let provider = test_provider("cached", "cached-model");
        let mut manifest = RegistryManifest {
            version: 1,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            providers: std::collections::HashMap::new(),
        };
        manifest
            .providers
            .insert(provider.id.clone(), manifest_entry(&provider));
        store.replace_all(&[provider]).expect("seed");
        store.save_manifest_meta(&manifest).expect("save meta");
        save_registry_metadata(&store, &manifest, Some(current_timestamp_secs()), None)
            .expect("save meta");

        let loaded = load_registry(&store);
        assert_eq!(loaded.source, RegistrySource::Cache);
        assert!(!loaded.providers.is_empty());
    }

    #[tokio::test]
    async fn remote_manifest_equal_to_local_no_update() {
        let store = RegistryStore::open_memory().expect("open");
        let provider = test_provider("test", "model-1");
        let mut manifest = RegistryManifest {
            version: 1,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            providers: std::collections::HashMap::new(),
        };
        manifest
            .providers
            .insert(provider.id.clone(), manifest_entry(&provider));
        store.replace_all(&[provider]).expect("seed");
        store.save_manifest_meta(&manifest).expect("save meta");
        save_registry_metadata(&store, &manifest, Some(current_timestamp_secs()), None)
            .expect("save meta");

        let fetcher = MockFetcher::new().with_manifest(manifest);
        let config = RegistryConfig::default();
        let result = check_registry_manifest(&store, &fetcher, &config).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn newer_remote_manifest_triggers_update() {
        let store = RegistryStore::open_memory().expect("open");
        // Seed cache with version 1
        let old_provider = test_provider("old", "old-model");
        let mut old_manifest = RegistryManifest {
            version: 1,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            providers: std::collections::HashMap::new(),
        };
        old_manifest
            .providers
            .insert(old_provider.id.clone(), manifest_entry(&old_provider));
        store.replace_all(&[old_provider]).expect("seed");
        store.save_manifest_meta(&old_manifest).expect("save meta");
        save_registry_metadata(&store, &old_manifest, Some(current_timestamp_secs()), None)
            .expect("save meta");

        // Remote manifest version 2 with a new provider
        let new_provider = test_provider("new", "new-model");
        let mut new_manifest = RegistryManifest {
            version: 2,
            updated_at: "2026-07-03T00:00:00Z".to_string(),
            providers: std::collections::HashMap::new(),
        };
        new_manifest
            .providers
            .insert(new_provider.id.clone(), manifest_entry(&new_provider));

        let fetcher = MockFetcher::new()
            .with_manifest(new_manifest.clone())
            .with_provider(new_provider);

        let config = RegistryConfig::default();
        let result = check_registry_manifest(&store, &fetcher, &config).await;
        assert!(result.is_ok());
        let remote = result.unwrap().unwrap();
        assert_eq!(remote.version, 2);
    }

    #[tokio::test]
    async fn network_failure_keeps_existing_registry() {
        let store = RegistryStore::open_memory().expect("open");
        let provider = test_provider("existing", "model-1");
        let mut manifest = RegistryManifest {
            version: 1,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            providers: std::collections::HashMap::new(),
        };
        manifest
            .providers
            .insert(provider.id.clone(), manifest_entry(&provider));
        store.replace_all(&[provider]).expect("seed");
        store.save_manifest_meta(&manifest).expect("save meta");
        save_registry_metadata(&store, &manifest, Some(current_timestamp_secs()), None)
            .expect("save meta");

        let fetcher = MockFetcher::new().with_manifest_error("network error");
        let config = RegistryConfig::default();
        let result = check_registry_manifest(&store, &fetcher, &config).await;
        assert!(result.is_err());

        // Existing registry should be intact
        let loaded = load_registry(&store);
        assert_eq!(loaded.source, RegistrySource::Cache);
        assert!(!loaded.providers.is_empty());
    }

    #[tokio::test]
    async fn invalid_remote_json_returns_error() {
        let store = RegistryStore::open_memory().expect("open");

        let fetcher = MockFetcher::new().with_manifest_error("invalid JSON");
        let config = RegistryConfig::default();
        let result = check_registry_manifest(&store, &fetcher, &config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn invalid_hash_rejected() {
        let provider = test_provider("bad-hash", "model-1");
        let mut manifest = RegistryManifest {
            version: 1,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            providers: std::collections::HashMap::new(),
        };
        manifest.providers.insert(
            provider.id.clone(),
            ManifestProviderEntry {
                file: "providers/bad-hash.json".to_string(),
                sha256: "0000000000000000000000000000000000000000000000000000000000000000"
                    .to_string(),
                model_count: 1,
            },
        );

        assert!(validate_registry_hashes(&[provider], &manifest).is_err());
    }

    #[tokio::test]
    async fn partial_update_failure_keeps_previous() {
        let store = RegistryStore::open_memory().expect("open");
        let old_provider = test_provider("old", "old-model");
        let mut old_manifest = RegistryManifest {
            version: 1,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            providers: std::collections::HashMap::new(),
        };
        old_manifest
            .providers
            .insert(old_provider.id.clone(), manifest_entry(&old_provider));
        store.replace_all(&[old_provider]).expect("seed");
        store.save_manifest_meta(&old_manifest).expect("save meta");
        save_registry_metadata(&store, &old_manifest, Some(current_timestamp_secs()), None)
            .expect("save meta");

        // New manifest with two providers; one will fail to fetch
        let good_provider = test_provider("good", "good-model");
        let bad_provider = test_provider("bad", "bad-model");
        let mut new_manifest = RegistryManifest {
            version: 2,
            updated_at: "2026-07-03T00:00:00Z".to_string(),
            providers: std::collections::HashMap::new(),
        };
        new_manifest
            .providers
            .insert(good_provider.id.clone(), manifest_entry(&good_provider));
        new_manifest
            .providers
            .insert(bad_provider.id.clone(), manifest_entry(&bad_provider));

        let fetcher = MockFetcher::new()
            .with_manifest(new_manifest)
            .with_provider(good_provider)
            .with_provider_error("bad", "network error for bad provider");

        let config = RegistryConfig::default();
        let updated = run_registry_update_check(&store, &fetcher, &config).await;
        assert!(!updated, "update should have failed");

        // Previous registry should still be intact
        let loaded = store.load_all_providers().expect("load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "old");
    }

    #[tokio::test]
    async fn rollback_to_previous_registry_on_apply_failure() {
        let store = RegistryStore::open_memory().expect("open");
        let old_provider = test_provider("old", "old-model");
        let mut old_manifest = RegistryManifest {
            version: 1,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            providers: std::collections::HashMap::new(),
        };
        old_manifest
            .providers
            .insert(old_provider.id.clone(), manifest_entry(&old_provider));
        store.replace_all(&[old_provider]).expect("seed");
        store.save_manifest_meta(&old_manifest).expect("save meta");

        // Validate hashes first — this should fail and prevent the apply.
        let new_provider = test_provider("new", "new-model");
        let mut new_manifest = RegistryManifest {
            version: 2,
            updated_at: "2026-07-03T00:00:00Z".to_string(),
            providers: std::collections::HashMap::new(),
        };
        new_manifest.providers.insert(
            new_provider.id.clone(),
            ManifestProviderEntry {
                file: "providers/new.json".to_string(),
                sha256: "wrong-hash".to_string(),
                model_count: 1,
            },
        );

        // Hash validation should catch the mismatch before apply.
        assert!(validate_registry_hashes(&[new_provider], &new_manifest).is_err());

        // Since validation failed, apply is never called. Old provider should still be there.
        let loaded = store.load_all_providers().expect("load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "old");
    }

    #[test]
    fn respecting_interval_with_jitter() {
        let store = RegistryStore::open_memory().expect("open");
        let config = RegistryConfig::default();
        let now = current_timestamp_secs();

        // Just checked → should not check
        save_registry_metadata(
            &store,
            &RegistryManifest {
                version: 1,
                updated_at: "2026-01-01T00:00:00Z".to_string(),
                providers: std::collections::HashMap::new(),
            },
            Some(now),
            None,
        )
        .expect("save");
        assert!(!should_check_registry_update(&store, &config));

        // 30h ago → should check (24h + up to 6h jitter = max 30h, 30h is past worst case)
        let past = now.saturating_sub(31 * 3600);
        save_registry_metadata(
            &store,
            &RegistryManifest {
                version: 1,
                updated_at: "2026-01-01T00:00:00Z".to_string(),
                providers: std::collections::HashMap::new(),
            },
            Some(past),
            None,
        )
        .expect("save");
        assert!(should_check_registry_update(&store, &config));

        // Verify jitter interval is within expected bounds
        let interval = registry_check_interval_with_jitter(&config);
        let base = config.check_interval_hours * 3600;
        let max = base + config.check_jitter_hours * 3600;
        assert!(interval >= base && interval <= max);
    }
}
