//! Drop-in provider definitions (`providers.d/`).
//!
//! Custom providers and models no longer belong in `config.toml`: a single file
//! that also carries every other setting becomes unmaintainable once it holds
//! dozens of providers, and NAVI used to rewrite the whole thing whenever a
//! preference changed. Instead, every provider gets its own file:
//!
//! ```text
//! ~/.config/navi/providers.d/
//!   devin.toml            # provider overlay (id defaults to the file stem)
//!   devin-models.toml     # model fragment (provider = "devin")
//!   team/                  # subdirectories are scanned recursively
//!     internal.toml
//! <project>/.navi/providers.d/*.toml
//! ```
//!
//! Both shapes are flat and match a `[[providers]]` entry from `config.toml`,
//! so blocks can be moved between the two by copy/paste. A file that only sets
//! the fields it wants **inherits everything else from the registry entry**
//! (`label`, `kind`, `api_key_env`, models, pricing, …):
//!
//! ```toml
//! # providers.d/devin.toml
//! base_url = "http://127.0.0.1:8787/v1"
//! api_key_env = "DEVIN_PROXY_KEY"
//! kind = "openai-chat-completions"
//!
//! [[models]]
//! name = "devin/swe-2"
//! context_window_tokens = 262000
//! supports_thinking = true
//! ```
//!
//! Rules:
//! * `id` is optional; it defaults to the file stem. `provider = "<id>"` turns
//!   the file into a model fragment for an existing provider.
//! * Drop-in definitions win over a `[[providers]]` entry with the same id.
//! * Parse errors and unknown keys are **diagnostics**, never fatal: the rest of
//!   the directory still loads and `navi provider check` reports the problems.
//! * NAVI writes here only on an explicit `navi provider add|add-model|remove`
//!   (or `migrate`) — never while saving preferences, syncing models or running
//!   a turn. Editing a file while NAVI runs is picked up by the hot-reload path
//!   ([`apply_if_changed`]).

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::types::{
    NaviConfig, ProviderConfig, ProviderKind, ProviderModelConfig, ProviderRequestOptions,
    ToolCallingMode,
};

/// Directory (relative to a config dir) scanned for drop-in provider files.
pub const PROVIDERS_DIR_NAME: &str = "providers.d";

/// Minimum interval between two hot-reload fingerprints.
const SCAN_THROTTLE: Duration = Duration::from_millis(300);

/// Directories that may hold drop-in provider files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DropInRoots {
    /// `~/.config/navi/providers.d` (or the platform equivalent).
    pub global: Option<PathBuf>,
    /// `<project>/.navi/providers.d`.
    pub project: Option<PathBuf>,
}

impl DropInRoots {
    /// Roots in load order (global first, project last so it wins).
    pub fn dirs(&self) -> impl Iterator<Item = &PathBuf> {
        self.global.iter().chain(self.project.iter())
    }

    /// All roots as a slice-like vector.
    pub fn to_vec(&self) -> Vec<PathBuf> {
        self.dirs().cloned().collect()
    }

    /// `true` when no root is configured.
    pub fn is_empty(&self) -> bool {
        self.global.is_none() && self.project.is_none()
    }
}

/// Severity of a drop-in diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticLevel {
    /// The file was skipped or partially applied.
    Warning,
    /// The file could not be parsed at all.
    Error,
}

/// A problem found while scanning the drop-in directory.
#[derive(Debug, Clone)]
pub struct DropInDiagnostic {
    /// Severity.
    pub level: DiagnosticLevel,
    /// File the problem was found in.
    pub path: PathBuf,
    /// Human-readable description (already includes the TOML error).
    pub message: String,
}

impl std::fmt::Display for DropInDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = match self.level {
            DiagnosticLevel::Warning => "warning",
            DiagnosticLevel::Error => "error",
        };
        write!(f, "{}: {}: {}", self.path.display(), level, self.message)
    }
}

/// What a single drop-in file contributed.
#[derive(Debug, Clone)]
pub struct DropInFileInfo {
    /// File path.
    pub path: PathBuf,
    /// Provider the file defines or extends.
    pub provider_id: String,
    /// Models declared in the file.
    pub model_count: usize,
    /// `true` for a provider overlay, `false` for a model fragment.
    pub defines_provider: bool,
}

/// Result of scanning every drop-in root.
#[derive(Debug, Clone, Default)]
pub struct DropInSnapshot {
    /// Resolved provider definitions, in file order.
    pub providers: Vec<ProviderConfig>,
    /// Parse/resolution problems (never fatal).
    pub diagnostics: Vec<DropInDiagnostic>,
    /// Per-file summary (used by `navi provider list|check`).
    pub files: Vec<DropInFileInfo>,
    /// Fingerprint of the scanned tree; changes when a file appears/edits/goes.
    pub fingerprint: u64,
}

impl DropInSnapshot {
    /// Number of providers contributed by drop-in files.
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// Total models declared across drop-in files.
    pub fn model_count(&self) -> usize {
        self.files.iter().map(|file| file.model_count).sum()
    }
}

/// One drop-in file, as authored on disk.
///
/// Every field is optional so the file behaves as an overlay over the registry
/// entry with the same id.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderFile {
    /// Provider id. Defaults to the file stem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    /// Model-fragment target: attach `[[models]]` to this provider id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<ProviderKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stream_idle_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request_max_retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stream_max_retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    websocket_connect_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retry_429: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_prompt_manifest: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_calling_mode: Option<ToolCallingMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request_options: Option<ProviderRequestOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    aggregator: Option<bool>,
    /// Models declared inline.
    #[serde(default)]
    models: Vec<ProviderModelConfig>,
    /// Registry model names to hide for this provider (case-insensitive).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    exclude_models: Vec<String>,
}

impl ProviderFile {
    /// Builds a minimal, human-readable file from a resolved provider.
    fn from_provider(provider: &ProviderConfig) -> Self {
        Self {
            id: Some(provider.id.clone()),
            provider: None,
            label: non_empty(&provider.label),
            description: non_empty(&provider.description),
            kind: Some(provider.kind),
            api_key_env: non_empty(&provider.api_key_env),
            base_url: provider.base_url.clone(),
            request_timeout_ms: provider.request_timeout_ms,
            stream_idle_timeout_ms: provider.stream_idle_timeout_ms,
            request_max_retries: provider.request_max_retries,
            stream_max_retries: provider.stream_max_retries,
            websocket_connect_timeout_ms: provider.websocket_connect_timeout_ms,
            retry_429: provider.retry_429,
            tool_prompt_manifest: provider.tool_prompt_manifest,
            tool_calling_mode: provider.tool_calling_mode,
            request_options: provider.request_options.clone(),
            aggregator: provider.aggregator.then_some(true),
            models: provider.models.clone(),
            exclude_models: Vec::new(),
        }
    }
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| value.to_string())
}

/// Model block used to append `[[models]]` entries without touching the rest of
/// a hand-edited file.
#[derive(Serialize)]
struct ModelsBlock {
    models: Vec<ProviderModelConfig>,
}

/// Computes the drop-in roots for a global config dir and a project directory.
pub fn roots_for(global_config_dir: Option<&Path>, project_dir: &Path) -> DropInRoots {
    DropInRoots {
        global: global_config_dir.map(|dir| dir.join(PROVIDERS_DIR_NAME)),
        project: Some(project_dir.join(".navi").join(PROVIDERS_DIR_NAME)),
    }
}

/// Global root used by callers that only know the config directory.
pub fn global_dir_for(global_config_dir: &Path) -> PathBuf {
    global_config_dir.join(PROVIDERS_DIR_NAME)
}

// ── Scanning ────────────────────────────────────────────────────────────────

/// Scans the drop-in roots, resolving overlays against the registry catalog.
pub fn scan(roots: &DropInRoots) -> DropInSnapshot {
    scan_with_base(roots, &super::base_provider_catalog())
}

/// Scans the drop-in roots, resolving overlays against `base`.
pub fn scan_with_base(roots: &DropInRoots, base: &[ProviderConfig]) -> DropInSnapshot {
    let mut snapshot = DropInSnapshot {
        fingerprint: fingerprint(roots),
        ..Default::default()
    };

    let files = collect_files(roots);
    let mut parsed: Vec<(PathBuf, ProviderFile)> = Vec::with_capacity(files.len());
    for path in files {
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(err) => {
                snapshot.diagnostics.push(DropInDiagnostic {
                    level: DiagnosticLevel::Error,
                    path: path.clone(),
                    message: format!("failed to read file: {err}"),
                });
                continue;
            }
        };
        if raw.trim().is_empty() {
            continue;
        }
        match toml::from_str::<ProviderFile>(&raw) {
            Ok(file) => parsed.push((path, file)),
            Err(err) => snapshot.diagnostics.push(DropInDiagnostic {
                level: DiagnosticLevel::Error,
                path: path.clone(),
                message: format!("invalid provider file: {err}"),
            }),
        }
    }

    // Resolve provider overlays first so fragments can target providers defined
    // in the same directory.
    let mut resolved: BTreeMap<String, ProviderConfig> = BTreeMap::new();
    let mut fragments: Vec<(PathBuf, ProviderFile, String)> = Vec::new();

    for (path, file) in &parsed {
        match (&file.id, &file.provider) {
            (Some(_), Some(_)) => snapshot.diagnostics.push(DropInDiagnostic {
                level: DiagnosticLevel::Error,
                path: path.clone(),
                message: "use either `id` (provider definition) or `provider` (model \
                          fragment), not both"
                    .to_string(),
            }),
            (None, Some(target)) => fragments.push((path.clone(), file.clone(), target.clone())),
            _ => {
                let id = match file.id.clone().or_else(|| file_stem(path)) {
                    Some(id) => id,
                    None => {
                        snapshot.diagnostics.push(DropInDiagnostic {
                            level: DiagnosticLevel::Error,
                            path: path.clone(),
                            message: "cannot derive a provider id from the file name".to_string(),
                        });
                        continue;
                    }
                };
                let id = canonical(id);
                let base_provider = base
                    .iter()
                    .find(|provider| canonical(provider.id.clone()) == id);
                let provider = resolve_overlay(&id, file, base_provider);
                if provider.models.is_empty() {
                    snapshot.diagnostics.push(DropInDiagnostic {
                        level: DiagnosticLevel::Warning,
                        path: path.clone(),
                        message: format!(
                            "provider `{id}` declares no models; add [[models]] entries or run \
                             `navi registry sync`"
                        ),
                    });
                }
                snapshot.files.push(DropInFileInfo {
                    path: path.clone(),
                    provider_id: id.clone(),
                    model_count: file.models.len(),
                    defines_provider: true,
                });
                resolved.insert(id, provider);
            }
        }
    }

    for (path, file, target) in fragments {
        let id = canonical(target);
        let known = resolved.contains_key(&id)
            || base
                .iter()
                .any(|provider| canonical(provider.id.clone()) == id);
        if !known {
            snapshot.diagnostics.push(DropInDiagnostic {
                level: DiagnosticLevel::Warning,
                path: path.clone(),
                message: format!(
                    "model fragment targets unknown provider `{id}`; define it in \
                     providers.d first"
                ),
            });
            continue;
        }
        let entry = resolved.entry(id.clone()).or_insert_with(|| {
            let base_provider = base
                .iter()
                .find(|provider| canonical(provider.id.clone()) == id)
                .cloned();
            base_provider.unwrap_or_else(|| new_provider(&id))
        });
        let before = entry.models.len();
        merge_models(&mut entry.models, &file.models, &file.exclude_models);
        snapshot.files.push(DropInFileInfo {
            path: path.clone(),
            provider_id: id,
            model_count: entry.models.len().saturating_sub(before),
            defines_provider: false,
        });
    }

    snapshot.providers = resolved.into_values().collect();
    snapshot
}

/// Applies an overlay file on top of the registry entry (if any).
fn resolve_overlay(id: &str, file: &ProviderFile, base: Option<&ProviderConfig>) -> ProviderConfig {
    let mut provider = base.cloned().unwrap_or_else(|| new_provider(id));
    provider.id = id.to_string();
    if let Some(label) = &file.label {
        provider.label = label.clone();
    }
    if let Some(description) = &file.description {
        provider.description = description.clone();
    }
    if let Some(kind) = file.kind {
        provider.kind = kind;
    }
    if let Some(api_key_env) = &file.api_key_env {
        provider.api_key_env = api_key_env.clone();
    }
    if let Some(base_url) = &file.base_url {
        provider.base_url = Some(base_url.clone());
    }
    if file.request_timeout_ms.is_some() {
        provider.request_timeout_ms = file.request_timeout_ms;
    }
    if file.stream_idle_timeout_ms.is_some() {
        provider.stream_idle_timeout_ms = file.stream_idle_timeout_ms;
    }
    if file.request_max_retries.is_some() {
        provider.request_max_retries = file.request_max_retries;
    }
    if file.stream_max_retries.is_some() {
        provider.stream_max_retries = file.stream_max_retries;
    }
    if file.websocket_connect_timeout_ms.is_some() {
        provider.websocket_connect_timeout_ms = file.websocket_connect_timeout_ms;
    }
    if file.retry_429.is_some() {
        provider.retry_429 = file.retry_429;
    }
    if file.tool_prompt_manifest.is_some() {
        provider.tool_prompt_manifest = file.tool_prompt_manifest;
    }
    if file.tool_calling_mode.is_some() {
        provider.tool_calling_mode = file.tool_calling_mode;
    }
    if file.request_options.is_some() {
        provider.request_options = file.request_options.clone();
    }
    if let Some(aggregator) = file.aggregator {
        provider.aggregator = aggregator;
    }
    merge_models(&mut provider.models, &file.models, &file.exclude_models);
    provider
}

/// Builds a brand-new provider with sensible defaults for a custom endpoint.
fn new_provider(id: &str) -> ProviderConfig {
    ProviderConfig {
        id: id.to_string(),
        label: id.to_string(),
        description: String::new(),
        // Custom endpoints are overwhelmingly OpenAI-compatible chat APIs; the
        // responses API is opt-in via `kind = "openai-responses"`.
        kind: ProviderKind::OpenAiChatCompletions,
        api_key_env: default_api_key_env(id),
        base_url: None,
        models: Vec::new(),
        ..Default::default()
    }
}

/// `my-provider` → `MY_PROVIDER_API_KEY`.
pub fn default_api_key_env(id: &str) -> String {
    let mut out = String::with_capacity(id.len() + 8);
    for ch in id.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_uppercase());
        } else {
            out.push('_');
        }
    }
    out.push_str("_API_KEY");
    out
}

/// Merges `incoming` models into `target` (overlay wins per field), dropping
/// everything listed in `exclude` first.
fn merge_models(
    target: &mut Vec<ProviderModelConfig>,
    incoming: &[ProviderModelConfig],
    exclude: &[String],
) {
    if !exclude.is_empty() {
        let excluded: HashSet<String> = exclude.iter().map(|name| name.to_lowercase()).collect();
        target.retain(|model| !excluded.contains(&model.name.to_lowercase()));
    }
    for model in incoming {
        match target
            .iter_mut()
            .find(|existing| existing.name.eq_ignore_ascii_case(&model.name))
        {
            Some(existing) => overlay_model(existing, model),
            None => target.push(model.clone()),
        }
    }
}

/// Field-wise overlay: the file wins where it sets a value.
fn overlay_model(target: &mut ProviderModelConfig, overlay: &ProviderModelConfig) {
    target.task_size = overlay.task_size.or(target.task_size);
    target.context_window_tokens = overlay
        .context_window_tokens
        .or(target.context_window_tokens);
    target.max_output_tokens = overlay.max_output_tokens.or(target.max_output_tokens);
    target.recommended_temperature = overlay
        .recommended_temperature
        .or(target.recommended_temperature);
    target.supports_thinking = overlay.supports_thinking.or(target.supports_thinking);
    target.default_reasoning_effort = overlay
        .default_reasoning_effort
        .clone()
        .or_else(|| target.default_reasoning_effort.clone());
    if !overlay.reasoning_levels.is_empty() {
        target.reasoning_levels = overlay.reasoning_levels.clone();
    }
    target.supports_images = overlay.supports_images.or(target.supports_images);
    target.supports_audio = overlay.supports_audio.or(target.supports_audio);
    target.supports_video = overlay.supports_video.or(target.supports_video);
    target.supports_documents = overlay.supports_documents.or(target.supports_documents);
    target.tool_prompt_manifest = overlay.tool_prompt_manifest.or(target.tool_prompt_manifest);
    target.pricing_input_per_1m = overlay.pricing_input_per_1m.or(target.pricing_input_per_1m);
    target.pricing_output_per_1m = overlay.pricing_output_per_1m.or(target.pricing_output_per_1m);
}

fn canonical(id: String) -> String {
    super::canonical_provider_id(&id).to_string()
}

fn file_stem(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .filter(|stem| !stem.trim().is_empty())
}

/// Every `*.toml` file under the roots, sorted for deterministic resolution.
fn collect_files(roots: &DropInRoots) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in roots.dirs() {
        collect_files_into(dir, &mut files);
    }
    files.sort();
    files
}

fn collect_files_into(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    children.sort();
    for child in children {
        if child.is_dir() {
            collect_files_into(&child, files);
            continue;
        }
        let is_toml = child
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"));
        if !is_toml {
            continue;
        }
        // Ignore editor/backup leftovers so a stray `devin.toml~` or
        // `devin.toml.bak` cannot break the load.
        let name = child.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with('.') || name.ends_with('~') {
            continue;
        }
        files.push(child);
    }
}

// ── Fingerprint / hot reload ────────────────────────────────────────────────

/// Cheap content fingerprint: file set + size + mtime, per root.
pub fn fingerprint(roots: &DropInRoots) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for dir in roots.dirs() {
        hash_tree(dir, &mut hasher);
    }
    hasher.finish()
}

fn hash_tree(dir: &Path, hasher: &mut impl Hasher) {
    dir.hash(hasher);
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    children.sort();
    for child in children {
        if child.is_dir() {
            hash_tree(&child, hasher);
            continue;
        }
        child.hash(hasher);
        if let Ok(metadata) = child.metadata() {
            metadata.len().hash(hasher);
            if let Ok(modified) = metadata.modified() {
                modified.hash(hasher);
            }
        }
    }
}

/// Bumped whenever the registry base catalog is invalidated, so overlays that
/// inherit registry values are re-resolved after a registry sync.
static BASE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Marks the registry catalog (and therefore every resolved overlay) stale.
pub fn bump_base_generation() {
    BASE_GENERATION.fetch_add(1, Ordering::Relaxed);
}

fn base_generation() -> u64 {
    BASE_GENERATION.load(Ordering::Relaxed)
}

pub(crate) struct CachedScan {
    roots: DropInRoots,
    snapshot: Arc<DropInSnapshot>,
    /// `config.providers` as it was *before* drop-ins were applied. Keeps
    /// refresh exact: deleting a drop-in file restores the `config.toml` entry.
    config_providers: Vec<ProviderConfig>,
    generation: u64,
    checked_at: Instant,
}

static CACHE: RwLock<Option<CachedScan>> = RwLock::new(None);

fn with_cache<T>(f: impl FnOnce(Option<&CachedScan>) -> T) -> T {
    with_cache_in(&CACHE, f)
}

fn with_cache_in<T>(cache: &RwLock<Option<CachedScan>>, f: impl FnOnce(Option<&CachedScan>) -> T) -> T {
    match cache.read() {
        Ok(guard) => f(guard.as_ref()),
        Err(poisoned) => f(poisoned.into_inner().as_ref()),
    }
}

/// Applies the drop-in providers to `config` (called once per config load).
///
/// Records the pre-drop-in provider list so a later [`apply_if_changed`] can
/// rebuild `config.providers` exactly.
pub fn apply_to_config(config: &mut NaviConfig, roots: DropInRoots) -> Arc<DropInSnapshot> {
    apply_to_config_in(&CACHE, config, roots)
}

/// [`apply_to_config`] against an explicit cache slot (unit tests use a local
/// one so they never share process-global state).
pub(crate) fn apply_to_config_in(
    cache: &RwLock<Option<CachedScan>>,
    config: &mut NaviConfig,
    roots: DropInRoots,
) -> Arc<DropInSnapshot> {
    let config_providers = config.providers.clone();
    let snapshot = Arc::new(scan(&roots));
    let mut providers = config_providers.clone();
    merge_dropins(&mut providers, &snapshot.providers);
    config.providers = providers;
    for diagnostic in &snapshot.diagnostics {
        match diagnostic.level {
            DiagnosticLevel::Error => tracing::warn!(target: "navi::config", "{diagnostic}"),
            DiagnosticLevel::Warning => tracing::debug!(target: "navi::config", "{diagnostic}"),
        }
    }
    store_cache_in(
        cache,
        CachedScan {
            roots,
            snapshot: snapshot.clone(),
            config_providers,
            generation: base_generation(),
            checked_at: Instant::now(),
        },
    );
    snapshot
}

/// Re-scans when the drop-in tree (or the registry base) changed.
///
/// Returns the fresh snapshot only when `config.providers` was updated, which
/// makes this safe to call from a UI tick: unchanged directories cost one
/// `read_dir` per root, throttled to [`SCAN_THROTTLE`].
pub fn apply_if_changed(config: &mut NaviConfig) -> Option<Arc<DropInSnapshot>> {
    apply_if_changed_in(&CACHE, config)
}

/// [`apply_if_changed`] against an explicit cache slot.
pub(crate) fn apply_if_changed_in(
    cache: &RwLock<Option<CachedScan>>,
    config: &mut NaviConfig,
) -> Option<Arc<DropInSnapshot>> {
    let cached = with_cache_in(cache, |cached| match cached {
        Some(cached) => (
            cached.roots.clone(),
            cached.snapshot.clone(),
            cached.config_providers.clone(),
            cached.generation,
            cached.checked_at.elapsed() < SCAN_THROTTLE,
        ),
        None => (DropInRoots::default(), Arc::default(), Vec::new(), 0, true),
    });
    let (roots, previous, config_providers, generation, fresh) = cached;

    if roots.is_empty() {
        return None;
    }
    if fresh && generation == base_generation() {
        return None;
    }

    let generation_now = base_generation();
    let fingerprint_now = fingerprint(&roots);
    if fingerprint_now == previous.fingerprint && generation_now == generation {
        touch_cache_in(cache);
        return None;
    }

    let snapshot = Arc::new(scan(&roots));
    let previous_ids: HashSet<String> = previous
        .providers
        .iter()
        .map(|provider| canonical(provider.id.clone()))
        .collect();

    // Rebuild: config.toml providers + anything added at runtime that is not
    // part of the drop-in slice + the fresh drop-in slice (wins by id).
    let mut providers = config_providers.clone();
    let known: HashSet<String> = providers
        .iter()
        .map(|provider| canonical(provider.id.clone()))
        .collect();
    providers.extend(
        config
            .providers
            .iter()
            .filter(|provider| {
                let id = canonical(provider.id.clone());
                !known.contains(&id) && !previous_ids.contains(&id)
            })
            .cloned(),
    );
    merge_dropins(&mut providers, &snapshot.providers);
    config.providers = providers;

    store_cache_in(
        cache,
        CachedScan {
            roots,
            snapshot: snapshot.clone(),
            config_providers,
            generation: generation_now,
            checked_at: Instant::now(),
        },
    );
    Some(snapshot)
}

fn store_cache_in(cache: &RwLock<Option<CachedScan>>, entry: CachedScan) {
    match cache.write() {
        Ok(mut guard) => *guard = Some(entry),
        Err(poisoned) => *poisoned.into_inner() = Some(entry),
    }
}

fn touch_cache_in(cache: &RwLock<Option<CachedScan>>) {
    match cache.write() {
        Ok(mut guard) => {
            if let Some(entry) = guard.as_mut() {
                entry.checked_at = Instant::now();
            }
        }
        Err(poisoned) => {
            if let Some(entry) = poisoned.into_inner().as_mut() {
                entry.checked_at = Instant::now();
            }
        }
    }
}

/// The most recent snapshot, if the drop-in directory was ever loaded.
pub fn cached_snapshot() -> Option<Arc<DropInSnapshot>> {
    with_cache(|cached| cached.map(|cached| cached.snapshot.clone()))
}

/// Roots of the most recent load.
pub fn cached_roots() -> Option<DropInRoots> {
    with_cache(|cached| cached.map(|cached| cached.roots.clone()))
}

/// Drops the cached snapshot so the next load re-reads the directory.
pub fn invalidate() {
    match CACHE.write() {
        Ok(mut guard) => *guard = None,
        Err(poisoned) => *poisoned.into_inner() = None,
    }
}

/// Removes any previous drop-in entry and appends the current ones.
fn merge_dropins(target: &mut Vec<ProviderConfig>, dropins: &[ProviderConfig]) {
    for dropin in dropins {
        let id = canonical(dropin.id.clone());
        target.retain(|provider| canonical(provider.id.clone()) != id);
        target.push(dropin.clone());
    }
}

// ── Writing (explicit commands only) ────────────────────────────────────────

/// Sanitizes a provider id into a file stem.
pub fn file_stem_for(provider_id: &str) -> String {
    let mut out = String::with_capacity(provider_id.len());
    for ch in provider_id.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    if out.is_empty() {
        out.push_str("provider");
    }
    out
}

/// Writes `provider` to `<dir>/<id>.toml`, replacing an existing file.
pub fn write_provider_file(dir: &Path, provider: &ProviderConfig) -> Result<PathBuf> {
    if provider.id.trim().is_empty() {
        anyhow::bail!("provider id must not be empty");
    }
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(format!("{}.toml", file_stem_for(&provider.id)));
    let file = ProviderFile::from_provider(provider);
    let mut content = String::from(
        "# Provider definition written by `navi provider add`.\n\
         # NAVI only rewrites this file on an explicit `navi provider` command —\n\
         # hand edits (comments included) survive preference saves and model syncs.\n\
         # Unset fields inherit the registry entry with the same id.\n\n",
    );
    content.push_str(&toml::to_string_pretty(&file).context("failed to serialize provider")?);
    crate::config::state::write_atomic(&path, &content)?;
    invalidate();
    Ok(path)
}

/// Writes a model-fragment file (`provider = "<id>"`) into `dir`.
pub fn write_model_fragment(
    dir: &Path,
    provider_id: &str,
    models: &[ProviderModelConfig],
) -> Result<PathBuf> {
    if provider_id.trim().is_empty() {
        anyhow::bail!("provider id must not be empty");
    }
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(format!("{}.models.toml", file_stem_for(provider_id)));
    let mut content = format!(
        "# Model fragment for provider `{provider_id}` — written by `navi provider add-model`.\n\
         # Add or delete [[models]] blocks by hand; NAVI re-reads this file at runtime.\n\n\
         provider = \"{provider_id}\"\n"
    );
    content.push_str(&models_toml(models)?);
    crate::config::state::write_atomic(&path, &content)?;
    invalidate();
    Ok(path)
}

/// Appends `[[models]]` blocks to an existing drop-in file.
///
/// Appending text (instead of re-serializing) keeps the user's comments and
/// formatting intact.
pub fn append_models_to_file(path: &Path, models: &[ProviderModelConfig]) -> Result<()> {
    if models.is_empty() {
        return Ok(());
    }
    let mut raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if !raw.ends_with('\n') {
        raw.push('\n');
    }
    raw.push_str(&models_toml(models)?);
    crate::config::state::write_atomic(path, &raw)?;
    invalidate();
    Ok(())
}

fn models_toml(models: &[ProviderModelConfig]) -> Result<String> {
    let block = ModelsBlock {
        models: models.to_vec(),
    };
    let mut out = String::new();
    for line in toml::to_string(&block)
        .context("failed to serialize models")?
        .lines()
    {
        out.push('\n');
        out.push_str(line);
    }
    out.push('\n');
    Ok(out)
}

/// Every drop-in file that defines or extends `provider_id`, in load order.
pub fn provider_files(roots: &DropInRoots, provider_id: &str) -> Vec<PathBuf> {
    let id = canonical(provider_id.to_string());
    let mut matches = Vec::new();
    for file in collect_files(roots) {
        let Ok(raw) = fs::read_to_string(&file) else {
            continue;
        };
        let Ok(parsed) = toml::from_str::<ProviderFile>(&raw) else {
            continue;
        };
        let target = parsed
            .provider
            .clone()
            .or(parsed.id.clone())
            .or_else(|| file_stem(&file));
        if let Some(target) = target
            && canonical(target) == id
        {
            matches.push(file);
        }
    }
    matches
}

/// File that defines `provider_id` (as opposed to merely extending it).
pub fn provider_file(roots: &DropInRoots, provider_id: &str) -> Option<PathBuf> {
    let id = canonical(provider_id.to_string());
    collect_files(roots).into_iter().find(|file| {
        let Ok(raw) = fs::read_to_string(file) else {
            return false;
        };
        let Ok(parsed) = toml::from_str::<ProviderFile>(&raw) else {
            return false;
        };
        if parsed.provider.is_some() {
            return false;
        }
        parsed
            .id
            .clone()
            .or_else(|| file_stem(file))
            .is_some_and(|target| canonical(target) == id)
    })
}

/// Deletes every drop-in file that defines or extends `provider_id`.
pub fn remove_provider_files(roots: &DropInRoots, provider_id: &str) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for file in provider_files(roots, provider_id) {
        fs::remove_file(&file).with_context(|| format!("failed to delete {}", file.display()))?;
        removed.push(file);
    }
    if !removed.is_empty() {
        invalidate();
    }
    Ok(removed)
}

/// Provider id → defining file, for `navi provider list`.
///
/// A file that defines the provider wins over model fragments for the same id.
pub fn provider_file_map(roots: &DropInRoots) -> BTreeMap<String, PathBuf> {
    let mut map = BTreeMap::new();
    let mut fragments = Vec::new();
    for file in collect_files(roots) {
        let Ok(raw) = fs::read_to_string(&file) else {
            continue;
        };
        let Ok(parsed) = toml::from_str::<ProviderFile>(&raw) else {
            continue;
        };
        if parsed.provider.is_some() {
            if let Some(target) = parsed.provider.clone() {
                fragments.push((canonical(target), file));
            }
            continue;
        }
        let target = parsed.id.clone().or_else(|| file_stem(&file));
        if let Some(target) = target {
            map.insert(canonical(target), file);
        }
    }
    for (id, file) in fragments {
        map.entry(id).or_insert(file);
    }
    map
}

/// Removes every `[[providers]]` block from a `config.toml` document, keeping
/// the rest of the file byte-identical (comments included).
///
/// Returns the new document and the number of removed blocks. Used by
/// `navi provider migrate`; NAVI never runs it implicitly.
pub fn strip_providers_section(raw: &str) -> (String, usize) {
    let mut out = String::with_capacity(raw.len());
    let mut removed = 0usize;
    let mut skipping = false;
    for line in raw.lines() {
        let trimmed = line.trim_start();
        // `[[providers]]` opens a block (counted once, even when the previous
        // block is still being skipped); `[[providers.models]]` and friends
        // only extend the block being skipped.
        if trimmed.starts_with("[[providers]]") {
            removed += 1;
            skipping = true;
            continue;
        }
        if trimmed.starts_with("[[providers.") || trimmed.starts_with("[providers.") {
            skipping = true;
            continue;
        }
        if skipping {
            if trimmed.starts_with('[') {
                skipping = false;
            } else {
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    (out, removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::ProviderModelConfig;

    fn base_catalog() -> Vec<ProviderConfig> {
        vec![ProviderConfig {
            id: "openai".to_string(),
            label: "OpenAI".to_string(),
            description: "registry entry".to_string(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: "OPENAI_API_KEY".to_string(),
            base_url: Some("https://api.openai.com/v1".to_string()),
            models: vec![
                ProviderModelConfig {
                    name: "gpt-5.5".to_string(),
                    context_window_tokens: Some(400_000),
                    supports_thinking: Some(true),
                    ..Default::default()
                },
                ProviderModelConfig {
                    name: "gpt-4o-legacy".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }]
    }

    fn roots_for_dir(dir: &Path) -> DropInRoots {
        DropInRoots {
            global: Some(dir.to_path_buf()),
            project: None,
        }
    }

    #[test]
    fn overlay_inherits_registry_fields_it_does_not_set() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("openai.toml"),
            "base_url = \"https://proxy.internal/v1\"\n",
        )
        .expect("write");

        let snapshot = scan_with_base(&roots_for_dir(dir.path()), &base_catalog());
        assert!(snapshot.diagnostics.is_empty(), "{:?}", snapshot.diagnostics);
        let provider = &snapshot.providers[0];

        assert_eq!(provider.id, "openai");
        assert_eq!(provider.label, "OpenAI", "label inherited");
        assert_eq!(provider.kind, ProviderKind::OpenAiResponses, "kind inherited");
        assert_eq!(provider.api_key_env, "OPENAI_API_KEY");
        assert_eq!(
            provider.base_url.as_deref(),
            Some("https://proxy.internal/v1")
        );
        assert_eq!(provider.models.len(), 2, "registry models kept");
    }

    #[test]
    fn new_provider_gets_defaults_and_inline_models() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("devin.toml"),
            r#"
base_url = "http://127.0.0.1:8787/v1"
api_key_env = "DEVIN_PROXY_KEY"
kind = "openai-chat-completions"

[[models]]
name = "devin/swe-2"
context_window_tokens = 262000
supports_thinking = true
"#,
        )
        .expect("write");

        let snapshot = scan_with_base(&roots_for_dir(dir.path()), &base_catalog());
        assert!(snapshot.diagnostics.is_empty(), "{:?}", snapshot.diagnostics);
        let provider = &snapshot.providers[0];

        assert_eq!(provider.id, "devin", "id comes from the file stem");
        assert_eq!(provider.label, "devin");
        assert_eq!(provider.api_key_env, "DEVIN_PROXY_KEY");
        assert_eq!(provider.kind, ProviderKind::OpenAiChatCompletions);
        assert_eq!(provider.models.len(), 1);
        assert_eq!(provider.models[0].name, "devin/swe-2");
        assert_eq!(provider.models[0].context_window_tokens, Some(262_000));
    }

    #[test]
    fn derived_api_key_env_uppercases_and_replaces_separators() {
        assert_eq!(default_api_key_env("my-provider"), "MY_PROVIDER_API_KEY");
        assert_eq!(default_api_key_env("devin.local"), "DEVIN_LOCAL_API_KEY");
    }

    #[test]
    fn model_fragments_attach_to_a_provider_defined_in_the_same_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("devin.toml"),
            "base_url = \"http://127.0.0.1:8787/v1\"\n\n[[models]]\nname = \"devin/swe-2\"\n",
        )
        .expect("write");
        fs::write(
            dir.path().join("devin-more.toml"),
            "provider = \"devin\"\n\n[[models]]\nname = \"devin/swe-2-max\"\nmax_output_tokens = 128000\n",
        )
        .expect("write");

        let snapshot = scan_with_base(&roots_for_dir(dir.path()), &base_catalog());
        assert!(snapshot.diagnostics.is_empty(), "{:?}", snapshot.diagnostics);
        assert_eq!(snapshot.providers.len(), 1);
        let models: Vec<&str> = snapshot.providers[0]
            .models
            .iter()
            .map(|model| model.name.as_str())
            .collect();
        assert_eq!(models, vec!["devin/swe-2", "devin/swe-2-max"]);
        assert_eq!(snapshot.files.len(), 2);
        assert!(!snapshot.files[1].defines_provider);
    }

    #[test]
    fn fragment_models_merge_into_registry_models() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("openai-extra.toml"),
            "provider = \"openai\"\n\n[[models]]\nname = \"gpt-5.6-preview\"\n",
        )
        .expect("write");

        let snapshot = scan_with_base(&roots_for_dir(dir.path()), &base_catalog());
        assert!(snapshot.diagnostics.is_empty(), "{:?}", snapshot.diagnostics);
        let models: Vec<&str> = snapshot.providers[0]
            .models
            .iter()
            .map(|model| model.name.as_str())
            .collect();
        assert_eq!(models, vec!["gpt-5.5", "gpt-4o-legacy", "gpt-5.6-preview"]);
    }

    #[test]
    fn exclude_models_hides_registry_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("openai.toml"),
            "exclude_models = [\"GPT-4O-Legacy\"]\n",
        )
        .expect("write");

        let snapshot = scan_with_base(&roots_for_dir(dir.path()), &base_catalog());
        let models: Vec<&str> = snapshot.providers[0]
            .models
            .iter()
            .map(|model| model.name.as_str())
            .collect();
        assert_eq!(models, vec!["gpt-5.5"], "exclusion is case-insensitive");
    }

    #[test]
    fn overlay_updates_an_existing_model_without_losing_its_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("openai.toml"),
            "[[models]]\nname = \"gpt-5.5\"\nmax_output_tokens = 64000\n",
        )
        .expect("write");

        let snapshot = scan_with_base(&roots_for_dir(dir.path()), &base_catalog());
        let model = &snapshot.providers[0].models[0];
        assert_eq!(model.max_output_tokens, Some(64_000), "file wins");
        assert_eq!(
            model.context_window_tokens,
            Some(400_000),
            "registry metadata kept"
        );
        assert_eq!(model.supports_thinking, Some(true));
    }

    #[test]
    fn unknown_keys_are_reported_instead_of_being_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("devin.toml"),
            "base_urll = \"http://127.0.0.1:8787/v1\"\n\n[[models]]\nname = \"devin/swe-2\"\n",
        )
        .expect("write");

        let snapshot = scan_with_base(&roots_for_dir(dir.path()), &base_catalog());
        assert_eq!(snapshot.diagnostics.len(), 1);
        assert_eq!(snapshot.diagnostics[0].level, DiagnosticLevel::Error);
        assert!(snapshot.diagnostics[0].message.contains("base_urll"));
    }

    #[test]
    fn id_and_provider_together_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("mixed.toml"),
            "id = \"devin\"\nprovider = \"openai\"\n",
        )
        .expect("write");

        let snapshot = scan_with_base(&roots_for_dir(dir.path()), &base_catalog());
        assert_eq!(snapshot.diagnostics.len(), 1);
        assert!(snapshot.diagnostics[0].message.contains("not both"));
        assert!(snapshot.providers.is_empty());
    }

    #[test]
    fn fragment_without_a_provider_is_a_warning_and_is_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("orphan.toml"),
            "provider = \"nope\"\n\n[[models]]\nname = \"x\"\n",
        )
        .expect("write");

        let snapshot = scan_with_base(&roots_for_dir(dir.path()), &base_catalog());
        assert_eq!(snapshot.diagnostics.len(), 1);
        assert_eq!(snapshot.diagnostics[0].level, DiagnosticLevel::Warning);
        assert!(snapshot.providers.is_empty());
    }

    #[test]
    fn a_provider_without_models_warns_but_still_loads() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("bare.toml"), "base_url = \"http://x/v1\"\n").expect("write");

        let snapshot = scan_with_base(&roots_for_dir(dir.path()), &base_catalog());
        assert_eq!(snapshot.providers.len(), 1);
        assert_eq!(snapshot.diagnostics.len(), 1);
        assert!(snapshot.diagnostics[0].message.contains("no models"));
    }

    #[test]
    fn non_toml_and_editor_leftovers_are_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("devin.toml"),
            "base_url = \"http://x/v1\"\n\n[[models]]\nname = \"devin/swe-2\"\n",
        )
        .expect("write");
        fs::write(dir.path().join("devin.toml.bak"), "not toml at all = =").expect("write");
        fs::write(dir.path().join("README.md"), "# providers").expect("write");

        let snapshot = scan_with_base(&roots_for_dir(dir.path()), &base_catalog());
        assert_eq!(snapshot.files.len(), 1);
        assert!(snapshot.diagnostics.is_empty(), "{:?}", snapshot.diagnostics);
    }

    #[test]
    fn subdirectories_are_scanned_recursively() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("team").join("internal");
        fs::create_dir_all(&nested).expect("mkdir");
        fs::write(
            nested.join("gateway.toml"),
            "base_url = \"http://gw/v1\"\n\n[[models]]\nname = \"gw-large\"\n",
        )
        .expect("write");

        let snapshot = scan_with_base(&roots_for_dir(dir.path()), &base_catalog());
        assert_eq!(snapshot.providers.len(), 1);
        assert_eq!(snapshot.providers[0].id, "gateway");
    }

    #[test]
    fn fingerprint_tracks_file_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let roots = roots_for_dir(dir.path());
        let before = fingerprint(&roots);
        fs::write(dir.path().join("devin.toml"), "base_url = \"http://x/v1\"\n").expect("write");
        let after = fingerprint(&roots);
        assert_ne!(before, after);

        fs::write(
            dir.path().join("devin.toml"),
            "base_url = \"http://x/v1\"\n# a longer comment\n",
        )
        .expect("write");
        assert_ne!(after, fingerprint(&roots));
    }

    #[test]
    fn apply_and_refresh_splice_providers_without_duplicating() {
        // A local cache slot keeps this test off the process-global one, so
        // tests can run in parallel (and a panic cannot wedge a shared lock).
        let cache = RwLock::new(None);
        let dir = tempfile::tempdir().expect("tempdir");
        let roots = roots_for_dir(dir.path());
        fs::write(
            dir.path().join("devin.toml"),
            "base_url = \"http://127.0.0.1:8787/v1\"\n\n[[models]]\nname = \"devin/swe-2\"\n",
        )
        .expect("write");

        let mut config = NaviConfig::default();
        config.providers.push(ProviderConfig {
            id: "devin".to_string(),
            label: "stale config.toml copy".to_string(),
            api_key_env: "DEVIN_PROXY_KEY".to_string(),
            ..Default::default()
        });
        config.providers.push(ProviderConfig {
            id: "hand-written".to_string(),
            label: "Hand written".to_string(),
            api_key_env: "HAND_API_KEY".to_string(),
            ..Default::default()
        });

        apply_to_config_in(&cache, &mut config, roots.clone());
        assert_eq!(config.providers.len(), 2, "no duplicate devin entry");
        let devin = config
            .providers
            .iter()
            .find(|provider| provider.id == "devin")
            .expect("devin present");
        assert_eq!(
            devin.base_url.as_deref(),
            Some("http://127.0.0.1:8787/v1"),
            "drop-in wins over config.toml"
        );
        assert!(config.providers.iter().any(|p| p.id == "hand-written"));

        // Runtime edit: a new file appears and is picked up by the hot-reload path.
        fs::write(
            dir.path().join("extra.toml"),
            "base_url = \"http://extra/v1\"\n\n[[models]]\nname = \"extra-large\"\n",
        )
        .expect("write");
        std::thread::sleep(SCAN_THROTTLE + Duration::from_millis(50));
        let snapshot = apply_if_changed_in(&cache, &mut config).expect("change detected");
        assert_eq!(snapshot.provider_count(), 2);
        assert!(config.providers.iter().any(|p| p.id == "extra"));

        // Removing a file restores the config.toml entry; removing every file
        // leaves only the config.toml provider list.
        fs::remove_file(dir.path().join("devin.toml")).expect("rm");
        fs::remove_file(dir.path().join("extra.toml")).expect("rm");
        std::thread::sleep(SCAN_THROTTLE + Duration::from_millis(50));
        apply_if_changed_in(&cache, &mut config);
        let devin = config
            .providers
            .iter()
            .find(|provider| provider.id == "devin")
            .expect("devin restored");
        assert_eq!(devin.label, "stale config.toml copy");
        assert!(!config.providers.iter().any(|p| p.id == "extra"));
    }

    #[test]
    fn provider_files_are_found_and_removed_by_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let roots = roots_for_dir(dir.path());
        write_provider_file(
            dir.path(),
            &ProviderConfig {
                id: "devin".to_string(),
                label: "Devin".to_string(),
                api_key_env: "DEVIN_PROXY_KEY".to_string(),
                base_url: Some("http://127.0.0.1:8787/v1".to_string()),
                models: vec![ProviderModelConfig {
                    name: "devin/swe-2".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .expect("write provider file");
        write_model_fragment(
            dir.path(),
            "devin",
            &[ProviderModelConfig {
                name: "devin/swe-2-max".to_string(),
                ..Default::default()
            }],
        )
        .expect("write fragment");

        assert!(provider_file(&roots, "devin").is_some());
        assert_eq!(provider_files(&roots, "devin").len(), 2);
        assert_eq!(
            provider_file_map(&roots).get("devin").cloned(),
            provider_file(&roots, "devin")
        );

        let snapshot = scan_with_base(&roots, &base_catalog());
        assert_eq!(snapshot.providers[0].models.len(), 2);
        assert!(snapshot.diagnostics.is_empty(), "{:?}", snapshot.diagnostics);

        let removed = remove_provider_files(&roots, "devin").expect("remove");
        assert_eq!(removed.len(), 2);
        assert!(provider_files(&roots, "devin").is_empty());
    }

    #[test]
    fn appended_models_keep_the_existing_file_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("devin.toml");
        let original = "# my notes\nbase_url = \"http://127.0.0.1:8787/v1\"\n";
        fs::write(&path, original).expect("write");

        append_models_to_file(
            &path,
            &[ProviderModelConfig {
                name: "devin/swe-2".to_string(),
                context_window_tokens: Some(262_000),
                ..Default::default()
            }],
        )
        .expect("append");

        let raw = fs::read_to_string(&path).expect("read");
        assert!(raw.starts_with(original), "hand-written text preserved");
        let snapshot = scan_with_base(&roots_for_dir(dir.path()), &base_catalog());
        assert_eq!(snapshot.providers[0].models[0].name, "devin/swe-2");
        assert_eq!(
            snapshot.providers[0].models[0].context_window_tokens,
            Some(262_000)
        );
    }

    #[test]
    fn strip_providers_section_keeps_the_rest_of_the_document() {
        let raw = "\
# my navi config
[model]
provider = \"devin\"
name = \"devin/swe-2\"

[[providers]]
id = \"devin\"
label = \"Devin\"
kind = \"openai-chat-completions\"
api_key_env = \"DEVIN_PROXY_KEY\"
base_url = \"http://127.0.0.1:8787/v1\"

[[providers.models]]
name = \"devin/swe-2\"

[tui]
theme = \"lain\"
";
        let (stripped, removed) = strip_providers_section(raw);
        assert_eq!(removed, 1);
        assert!(!stripped.contains("[[providers]]"));
        assert!(!stripped.contains("DEVIN_PROXY_KEY"));
        assert!(stripped.contains("# my navi config"));
        assert!(stripped.contains("[model]"));
        assert!(stripped.contains("[tui]"));
        assert!(stripped.contains("theme = \"lain\""));
    }

    #[test]
    fn strip_providers_section_handles_multiple_blocks() {
        let raw = "\
[tui]
theme = \"lain\"

[[providers]]
id = \"a\"

[[providers]]
id = \"b\"
";
        let (stripped, removed) = strip_providers_section(raw);
        assert_eq!(removed, 2);
        assert_eq!(stripped.trim(), "[tui]\ntheme = \"lain\"");
    }
}
