//! HTTP fetcher for the remote registry files hosted in the NAVI registry DB repo.
//!
//! The registry database lives at <https://github.com/navi-ai-org/navi-registry>.
//! This fetcher pulls the manifest and per-provider JSON files from GitHub raw
//! content, verifying SHA-256 integrity hashes against the manifest.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::Path;

use super::types::{
    CanonicalModel, RegistryManifest, RegistryProvider, RegistryTranscriptionProvider,
};

/// Base URL for the NAVI registry database on GitHub. Uses `raw.githubusercontent.com`
/// for direct file access without the GitHub API rate limits.
const REGISTRY_BASE_URL: &str = "https://raw.githubusercontent.com/navi-ai-org/navi-registry/main";

/// Timeout for individual HTTP requests.
const FETCH_TIMEOUT_SECS: u64 = 15;

/// Attempts per registry request before giving up.
///
/// `navi registry sync` used to abort on the first DNS/TLS hiccup
/// (`error sending request`) even though an immediate retry succeeds.
const FETCH_ATTEMPTS: u32 = 3;

/// Base backoff between attempts; multiplied by the attempt number.
const FETCH_RETRY_BACKOFF_MS: u64 = 300;

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
            .unwrap_or_else(|err| {
                // Builder fails only for rare TLS/backend misconfig; fall back so
                // registry sync still attempts with a default client.
                tracing::warn!(
                    error = %err,
                    "failed to build registry HTTP client with custom settings; using default client"
                );
                reqwest::Client::new()
            });
        Self { client }
    }

    /// GETs `url` as text, retrying transient failures (connect/TLS/timeout,
    /// 5xx/408/425/429) with a bounded backoff. Permanent 4xx failures fail fast.
    async fn fetch_text_with_retry(&self, url: &str, label: &str) -> Result<String> {
        fetch_with_retry(url, label, || async {
            let response = self.client.get(url).send().await?;
            response.error_for_status()?.text().await
        })
        .await
    }

    /// Fetches the manifest from the remote registry.
    pub async fn fetch_manifest(&self) -> Result<RegistryManifest> {
        let url = format!("{REGISTRY_BASE_URL}/manifest.json");
        let text = self.fetch_text_with_retry(&url, "manifest").await?;
        serde_json::from_str::<RegistryManifest>(&text).context("failed to parse manifest JSON")
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
        let text = self
            .fetch_text_with_retry(&url, &format!("provider '{provider_id}'"))
            .await?;

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
        let bases =
            super::extends::base_map_from_embedded(super::embedded::embedded_base_files(), &[])
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
        let text = self
            .fetch_text_with_retry(&url, &format!("canonical model '{model_id}'"))
            .await?;

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
        let text = self
            .fetch_text_with_retry(&url, &format!("transcription provider '{provider_id}'"))
            .await?;

        let hash = hex::encode(Sha256::digest(text.as_bytes()));
        if hash != entry.sha256 {
            anyhow::bail!(
                "transcription provider '{provider_id}' integrity check failed: expected {}, got {}",
                entry.sha256,
                hash
            );
        }

        serde_json::from_str::<RegistryTranscriptionProvider>(&text)
            .with_context(|| format!("failed to parse transcription provider '{provider_id}' JSON"))
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

/// True when an HTTP status is worth another attempt: request timeout, rate
/// limiting, or any server-side error. 4xx client errors are permanent.
pub(crate) fn is_transient_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429) || (500..=599).contains(&status)
}

/// True when a request failure is worth another attempt.
///
/// Failures with a status come from `error_for_status()`; everything else is a
/// transport-level failure (DNS, connect, TLS, timeout, truncated body) and is
/// retried.
fn is_transient_fetch_error(err: &reqwest::Error) -> bool {
    match err.status() {
        Some(status) => is_transient_status(status.as_u16()),
        None => true,
    }
}

/// Runs `op` up to [`FETCH_ATTEMPTS`] times, retrying only transient failures
/// with a bounded backoff.
///
/// Extracted so the retry policy is testable without the network: `op` is any
/// closure returning a `reqwest::Error` on failure.
pub(crate) async fn fetch_with_retry<T, F, Fut>(url: &str, label: &str, mut op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, reqwest::Error>>,
{
    let mut attempts = 0u32;
    let mut last_error: Option<reqwest::Error> = None;

    for attempt in 1..=FETCH_ATTEMPTS {
        attempts = attempt;
        match op().await {
            Ok(value) => {
                if attempt > 1 {
                    tracing::info!(url, label, attempt, "registry fetch succeeded after retry");
                }
                return Ok(value);
            }
            Err(err) => {
                let transient = is_transient_fetch_error(&err);
                tracing::debug!(
                    url,
                    label,
                    attempt,
                    transient,
                    error = %err,
                    "registry fetch attempt failed"
                );
                last_error = Some(err);
                if !transient || attempt == FETCH_ATTEMPTS {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(
                    FETCH_RETRY_BACKOFF_MS * u64::from(attempt),
                ))
                .await;
            }
        }
    }

    let err = last_error.expect("the loop always records an error before exiting");
    let note = match (attempts, err.status()) {
        (n, _) if n > 1 => format!(" after {n} attempts"),
        (_, Some(status)) => format!(" (HTTP {status}; not retried)"),
        _ => String::new(),
    };
    Err(err).with_context(|| {
        format!(
            "failed to fetch {label} from {url}{note} — check network access to raw.githubusercontent.com"
        )
    })
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
fn load_local_model_catalog(models_dir: &Path) -> Result<super::resolve::ModelCatalog> {
    let mut catalog = std::collections::HashMap::new();
    if !models_dir.is_dir() {
        return Ok(catalog);
    }
    for entry in std::fs::read_dir(models_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let text = std::fs::read_to_string(&path)?;
            let model: super::types::CanonicalModel =
                serde_json::from_str(&text).with_context(|| {
                    format!("failed to parse canonical model from {}", path.display())
                })?;
            // Prefer JSON `id` (may contain `:`). Filenames use `__` for Windows.
            let id = if !model.id.is_empty() {
                model.id.clone()
            } else {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .replace("__", ":")
            };
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
    if !force
        && let Some(updated_at) = store.manifest_updated_at()?
        && let Ok(parsed) = parse_iso_timestamp(&updated_at)
        && parsed.hours_ago < 24
        && !store.is_empty()?
    {
        tracing::debug!("registry cache is fresh, skipping fetch");
        return Ok(false);
    }

    tracing::info!("fetching remote registry manifest");
    let manifest = std::sync::Arc::new(fetcher.fetch_manifest().await?);

    // Compare with stored manifest version.
    if !force
        && let Some(stored_version) = store.manifest_version()?
        && stored_version >= manifest.version
        && !store.is_empty()?
    {
        // Same (or newer) version can still leave orphan rows when a
        // provider was deleted from the remote catalog but the cache
        // already advanced its manifest metadata via embedded merge
        // without pruning. Always drop providers not in the remote set.
        let keep_ids: std::collections::HashSet<&str> =
            manifest.providers.keys().map(|s| s.as_str()).collect();
        let tx_keep: std::collections::HashSet<&str> = manifest
            .transcription_providers
            .keys()
            .map(|s| s.as_str())
            .collect();
        let model_keep: std::collections::HashSet<&str> =
            manifest.models.keys().map(|s| s.as_str()).collect();
        store.delete_providers_not_in(&keep_ids)?;
        store.delete_transcription_providers_not_in(&tx_keep)?;
        store.delete_canonical_models_not_in(&model_keep)?;
        tracing::debug!(
            stored = stored_version,
            remote = manifest.version,
            "registry manifest is up-to-date"
        );
        return Ok(false);
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
        let is_local_api_sync = cached_sha.as_deref() == Some(crate::registry::LOCAL_API_SYNC_SHA);
        if force || (!is_local_api_sync && cached_sha.as_deref() != Some(&entry.sha256)) {
            to_fetch.push(provider_id.clone());
        }
    }

    // Diff transcription providers even when LLM providers are unchanged.
    let mut tx_to_fetch = Vec::new();
    let mut tx_keep: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (provider_id, entry) in &manifest.transcription_providers {
        tx_keep.insert(provider_id.as_str());
        let cached_sha = store.transcription_provider_sha256(provider_id)?;
        if force || cached_sha.as_deref() != Some(&entry.sha256) {
            tx_to_fetch.push(provider_id.clone());
        }
    }

    // Diff canonical model catalog (models/<id>.json).
    let mut model_to_fetch = Vec::new();
    let mut model_keep: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (model_id, entry) in &manifest.models {
        model_keep.insert(model_id.as_str());
        let cached_sha = store.canonical_model_sha256(model_id)?;
        if force || cached_sha.as_deref() != Some(&entry.sha256) {
            model_to_fetch.push(model_id.clone());
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

    // Sync canonical models first (parallel) so provider ref resolution sees the latest catalog.
    let mut models_updated = 0;
    if !model_to_fetch.is_empty() {
        let model_futs: Vec<_> = model_to_fetch
            .iter()
            .map(|model_id| {
                let mid = model_id.clone();
                let m = std::sync::Arc::clone(&manifest);
                async move {
                    let model = fetcher.fetch_canonical_model(&mid, &m).await?;
                    Ok::<_, anyhow::Error>((mid, model))
                }
            })
            .collect();
        let results = futures_util::future::try_join_all(model_futs).await?;
        for (model_id, model) in results {
            let sha = &manifest.models[&model_id].sha256;
            store.upsert_canonical_model(&model_id, &model, Some(sha))?;
            models_updated += 1;
        }
    }
    store.delete_canonical_models_not_in(&model_keep)?;

    // Prefer cached catalog (now refreshed); fall back to embedded snapshot.
    let mut catalog = store.load_canonical_model_catalog().unwrap_or_default();
    if catalog.is_empty() {
        catalog = super::embedded::embedded_model_catalog().unwrap_or_default();
    }

    // Parallel fetch provider JSON, then apply sequentially (SQLite writer).
    let mut updated = 0;
    if !to_fetch.is_empty() {
        let provider_futs: Vec<_> = to_fetch
            .iter()
            .map(|provider_id| {
                let pid = provider_id.clone();
                let m = std::sync::Arc::clone(&manifest);
                async move {
                    let provider = fetcher.fetch_provider(&pid, &m).await?;
                    Ok::<_, anyhow::Error>((pid, provider))
                }
            })
            .collect();
        let fetched = futures_util::future::try_join_all(provider_futs).await?;
        for (provider_id, mut provider) in fetched {
            super::resolve::resolve_provider_refs(&mut provider, &catalog);
            let sha = &manifest.providers[&provider_id].sha256;
            // Check the sha marker *before* the union-merge overwrites it, so we
            // know whether this provider's models came from live API sync.
            let is_local_api_sync = store.provider_sha256(&provider_id)?.as_deref()
                == Some(crate::registry::LOCAL_API_SYNC_SHA);
            // Union-merge so remote catalog refresh cannot wipe API-synced models.
            store.upsert_provider_union_models(&provider, Some(sha))?;
            // Prune stale models removed from the remote catalog for pure
            // catalog providers (not local-api-sync).
            if !is_local_api_sync {
                let keep: std::collections::HashSet<String> =
                    provider.models.iter().map(|m| m.name.clone()).collect();
                store.prune_provider_stale_models(&provider_id, &keep)?;
            }
            updated += 1;
        }
    }

    // Remove providers that were deleted from the remote registry.
    store.delete_providers_not_in(&keep_ids)?;

    // Sync remote transcription / dictation providers (parallel fetch).
    let mut tx_updated = 0;
    if !tx_to_fetch.is_empty() {
        let tx_futs: Vec<_> = tx_to_fetch
            .iter()
            .map(|provider_id| {
                let pid = provider_id.clone();
                let m = std::sync::Arc::clone(&manifest);
                async move {
                    let provider = fetcher.fetch_transcription_provider(&pid, &m).await?;
                    Ok::<_, anyhow::Error>((pid, provider))
                }
            })
            .collect();
        let fetched = futures_util::future::try_join_all(tx_futs).await?;
        for (provider_id, provider) in fetched {
            let entry = &manifest.transcription_providers[&provider_id];
            store.upsert_transcription_provider(&provider, Some(&entry.sha256))?;
            tx_updated += 1;
        }
    }
    store.delete_transcription_providers_not_in(&tx_keep)?;

    store.save_manifest_meta(&manifest)?;
    // Catalog contents changed — drop in-memory base catalog used by TUI/modals.
    crate::config::providers::invalidate_registry_catalog_cache();
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
    use std::io::{Read, Write};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    // ── retry policy ─────────────────────────────────────────────────────

    const OK_BODY: &str = "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 2\r\n\r\nok";
    const ERR_500: &str =
        "HTTP/1.1 500 Internal Server Error\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";
    const ERR_429: &str =
        "HTTP/1.1 429 Too Many Requests\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";
    const ERR_404: &str =
        "HTTP/1.1 404 Not Found\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";

    /// Loopback HTTP server that answers each accepted connection with the next
    /// canned response. Bounded by the response count and a deadline so a test
    /// that stops early cannot leave a blocked thread behind.
    fn spawn_canned_server(responses: Vec<&'static str>) -> (String, Arc<AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let addr = listener.local_addr().expect("local addr");
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut served = 0usize;
            while served < responses.len() && std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut buf = [0u8; 2048];
                        let _ = stream.read(&mut buf);
                        let body = responses[served];
                        served += 1;
                        counter.fetch_add(1, Ordering::SeqCst);
                        let _ = stream.write_all(body.as_bytes());
                        let _ = stream.flush();
                    }
                    Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        (format!("http://{addr}/manifest.json"), hits)
    }

    #[test]
    fn transient_status_classifier_matches_retryable_codes() {
        for status in [408, 425, 429, 500, 502, 503, 504, 599] {
            assert!(is_transient_status(status), "{status} should be retried");
        }
        for status in [200, 301, 400, 401, 403, 404, 410, 422] {
            assert!(!is_transient_status(status), "{status} must not be retried");
        }
    }

    #[tokio::test]
    async fn fetch_retries_server_errors_then_succeeds() {
        let (url, hits) = spawn_canned_server(vec![ERR_500, ERR_500, OK_BODY]);
        let fetcher = RegistryFetcher::new();
        let text = fetcher
            .fetch_text_with_retry(&url, "manifest")
            .await
            .expect("third attempt should succeed");
        assert_eq!(text, "ok");
        assert_eq!(hits.load(Ordering::SeqCst), 3, "expected two retries");
    }

    #[tokio::test]
    async fn fetch_retries_rate_limit_then_succeeds() {
        let (url, hits) = spawn_canned_server(vec![ERR_429, OK_BODY]);
        let fetcher = RegistryFetcher::new();
        let text = fetcher
            .fetch_text_with_retry(&url, "manifest")
            .await
            .expect("second attempt should succeed");
        assert_eq!(text, "ok");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn fetch_gives_up_after_max_attempts_with_recovery_hint() {
        let (url, hits) = spawn_canned_server(vec![ERR_500, ERR_500, ERR_500]);
        let fetcher = RegistryFetcher::new();
        let err = fetcher
            .fetch_text_with_retry(&url, "manifest")
            .await
            .expect_err("all attempts fail");
        let msg = format!("{err:#}");
        assert!(hits.load(Ordering::SeqCst) >= 3, "expected 3 attempts");
        assert!(
            msg.contains("failed to fetch manifest from") && msg.contains("after 3 attempts"),
            "error should name the resource and the attempt count: {msg}"
        );
        assert!(
            msg.contains("check network access"),
            "error should carry a recovery hint: {msg}"
        );
    }

    #[tokio::test]
    async fn fetch_does_not_retry_client_errors() {
        let (url, hits) = spawn_canned_server(vec![ERR_404, OK_BODY]);
        let fetcher = RegistryFetcher::new();
        let err = fetcher
            .fetch_text_with_retry(&url, "provider 'demo'")
            .await
            .expect_err("404 is permanent");
        let msg = format!("{err:#}");
        assert_eq!(hits.load(Ordering::SeqCst), 1, "404 must not be retried");
        assert!(
            msg.contains("404") && msg.contains("not retried"),
            "error should explain why it was not retried: {msg}"
        );
    }

    #[tokio::test]
    async fn fetch_retries_transport_failures() {
        // Port 1 is never bound by an unprivileged test process, so the client
        // sees a deterministic connect error (a retryable transport failure).
        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&attempts);
        let client = reqwest::Client::new();
        let err = fetch_with_retry("http://127.0.0.1:1/manifest.json", "manifest", || {
            counter.fetch_add(1, Ordering::SeqCst);
            let client = client.clone();
            async move { client.get("http://127.0.0.1:1/manifest.json").send().await }
        })
        .await
        .expect_err("connect errors never succeed");
        assert_eq!(attempts.load(Ordering::SeqCst), 3, "transport errors retry");
        assert!(format!("{err:#}").contains("after 3 attempts"));
    }

    #[tokio::test]
    async fn fetch_with_retry_returns_first_success_without_extra_calls() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let value = fetch_with_retry("https://example.invalid/manifest.json", "manifest", || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { Ok::<_, reqwest::Error>(42u32) }
        })
        .await
        .expect("first attempt succeeds");
        assert_eq!(value, 42);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
