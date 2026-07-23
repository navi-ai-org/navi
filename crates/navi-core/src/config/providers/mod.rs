mod opencode;
mod registry;

use std::sync::{Arc, RwLock};

use crate::config::types::{
    ModelOption, ModelTaskSize, NaviConfig, ProviderConfig, ProviderKind, ProviderModelConfig,
    ToolCallingMode,
};
use crate::model::AttachmentKind;
use crate::registry::RegistryStore;

pub use opencode::{is_free_model_name, model_can_run_publicly, provider_request_model_name};
pub use registry::default_request_options_for;

// ── Process-global registry store for catalog integration ────────────
// Must be process-global (not thread-local): N-API / Electron invoke engine
// methods across Tokio worker threads and the Node main thread. A TLS store
// made Sync All look fine in-process (in-memory config) while restarts fell
// back to the embedded snapshot on threads that never called set_registry_store.

static REGISTRY_STORE: RwLock<Option<Arc<RegistryStore>>> = RwLock::new(None);

/// In-memory base catalog (registry providers before user overrides).
///
/// Without this, every `provider_catalog()` call re-ran `load_registry()` —
/// SQLite load + embedded merge + full model rehydrate — which made the
/// Providers modal and model lists lag hard.
static BASE_CATALOG_CACHE: RwLock<Option<Arc<Vec<ProviderConfig>>>> = RwLock::new(None);

/// Sets the process-global registry store used by [`provider_catalog`].
/// Typically called once during engine initialization.
pub fn set_registry_store(store: Arc<RegistryStore>) {
    match REGISTRY_STORE.write() {
        Ok(mut guard) => *guard = Some(store),
        Err(poisoned) => *poisoned.into_inner() = Some(store),
    }
    invalidate_registry_catalog_cache();
}

/// Drop the in-memory base catalog so the next [`provider_catalog`] reloads
/// from SQLite. Call after remote registry sync / model API sync mutates the store.
pub fn invalidate_registry_catalog_cache() {
    match BASE_CATALOG_CACHE.write() {
        Ok(mut guard) => *guard = None,
        Err(poisoned) => *poisoned.into_inner() = None,
    }
}

/// Returns the process-global registry store when the engine has initialized it.
///
/// Used by the transcription catalog (and similar) so STT providers can share
/// the same SQLite cache + remote sync path as LLM providers.
pub fn registry_store_for_catalog() -> Option<Arc<RegistryStore>> {
    match REGISTRY_STORE.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// Returns the full provider catalog: SQLite registry cache merged with any
/// user-configured overrides. Falls back to built-in providers if the
/// registry cache is empty or unavailable.
pub fn provider_catalog(config: &NaviConfig) -> Vec<ProviderConfig> {
    let mut providers = base_provider_catalog();
    merge_provider_configs(&mut providers, config.providers.clone());
    apply_default_request_options(&mut providers);
    providers
}

pub(crate) fn base_provider_catalog() -> Vec<ProviderConfig> {
    if let Some(cached) = read_base_catalog_cache() {
        return (*cached).clone();
    }

    let store = match REGISTRY_STORE.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    let providers = match store {
        Some(store) => match crate::registry::load_registry(&store) {
            loaded if !loaded.providers.is_empty() => loaded.providers,
            _ => {
                tracing::debug!("loaded registry is empty, falling back to embedded snapshot");
                load_embedded_or_minimal_fallback()
            }
        },
        None => {
            tracing::debug!("registry store not set, falling back to embedded snapshot");
            load_embedded_or_minimal_fallback()
        }
    };
    write_base_catalog_cache(providers.clone());
    providers
}

fn read_base_catalog_cache() -> Option<Arc<Vec<ProviderConfig>>> {
    match BASE_CATALOG_CACHE.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

fn write_base_catalog_cache(providers: Vec<ProviderConfig>) {
    let arc = Arc::new(providers);
    match BASE_CATALOG_CACHE.write() {
        Ok(mut guard) => *guard = Some(arc),
        Err(poisoned) => *poisoned.into_inner() = Some(arc),
    }
}

fn load_embedded_or_minimal_fallback() -> Vec<ProviderConfig> {
    match crate::registry::load_embedded_registry() {
        Some(loaded) if !loaded.providers.is_empty() => loaded.providers,
        Some(_) => minimal_fallback_providers(),
        None => {
            tracing::error!("failed to parse embedded registry snapshot, using minimal fallback");
            minimal_fallback_providers()
        }
    }
}

/// Minimal hardcoded fallback used only if the embedded snapshot itself fails
/// to parse (should never happen in practice).
fn minimal_fallback_providers() -> Vec<ProviderConfig> {
    vec![ProviderConfig {
        id: "openai".to_string(),
        label: "OpenAI".to_string(),
        description: "OpenAI API key required".to_string(),
        kind: ProviderKind::OpenAiResponses,
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
            reasoning_levels: Vec::new(),
            default_reasoning_effort: None,
        }],
        request_options: default_request_options_for("openai"),
        ..Default::default()
    }]
}

/// Maps provider aliases to their canonical form (e.g. `"opencode-zen"` to `"opencode"`).
pub fn canonical_provider_id(id: &str) -> &str {
    match id {
        "opencode-zen" => "opencode",
        other => other,
    }
}

/// Resolves a provider config by id from the merged catalog, following aliases.
pub fn resolve_provider_config(config: &NaviConfig, id: &str) -> Option<ProviderConfig> {
    let canonical_id = canonical_provider_id(id);
    provider_catalog(config)
        .into_iter()
        .find(|provider| canonical_provider_id(&provider.id) == canonical_id)
}

/// Returns all available model options across all providers in the catalog.
pub fn available_model_options(config: &NaviConfig) -> Vec<ModelOption> {
    provider_catalog(config)
        .into_iter()
        .flat_map(|provider| {
            let desc = provider.description.clone();
            provider
                .models
                .clone()
                .into_iter()
                .map(move |model| ModelOption {
                    name: model.name,
                    provider_id: provider.id.clone(),
                    provider_label: provider.label.clone(),
                    provider_description: desc.clone(),
                    task_size: model.task_size,
                    context_window_tokens: model.context_window_tokens,
                    supports_thinking: model.supports_thinking,
                    reasoning_levels: model.reasoning_levels,
                    default_reasoning_effort: model.default_reasoning_effort,
                })
        })
        .collect()
}

/// Returns the context window size for the selected model, or a default if unknown.
pub fn effective_context_window(config: &NaviConfig) -> u64 {
    let selected_provider = &config.model.provider;
    let selected_model = &config.model.name;
    available_model_options(config)
        .into_iter()
        .find(|m| m.provider_id == *selected_provider && m.name == *selected_model)
        .and_then(|m| m.context_window_tokens)
        .unwrap_or(crate::config::defaults::DEFAULT_CONTEXT_WINDOW)
}

/// List pricing (USD per 1M tokens) for a provider/model.
///
/// Prefers the live catalog (registry + overrides). Falls back to the embedded
/// snapshot so cost estimates still work when SQLite pricing rows are missing.
pub fn model_list_pricing(
    config: &NaviConfig,
    provider_id: &str,
    model_name: &str,
) -> Option<(f64, f64)> {
    let canonical = canonical_provider_id(provider_id);
    let from_catalog = provider_catalog(config).into_iter().find_map(|p| {
        if canonical_provider_id(&p.id) != canonical {
            return None;
        }
        p.models.into_iter().find_map(|m| {
            if m.name != model_name {
                return None;
            }
            match (m.pricing_input_per_1m, m.pricing_output_per_1m) {
                (Some(i), Some(o)) => Some((i, o)),
                (Some(i), None) => Some((i, 0.0)),
                (None, Some(o)) => Some((0.0, o)),
                (None, None) => None,
            }
        })
    });
    if from_catalog.is_some() {
        return from_catalog;
    }
    // Embedded snapshot fallback (e.g. SQLite cache missing model_pricing rows).
    // `load_embedded_registry` already maps to ProviderConfig with pricing fields.
    let embedded = crate::registry::load_embedded_registry()?;
    embedded.providers.into_iter().find_map(|p| {
        if canonical_provider_id(&p.id) != canonical {
            return None;
        }
        p.models.into_iter().find_map(|m| {
            if m.name != model_name {
                return None;
            }
            match (m.pricing_input_per_1m, m.pricing_output_per_1m) {
                (Some(i), Some(o)) => Some((i, o)),
                (Some(i), None) => Some((i, 0.0)),
                (None, Some(o)) => Some((0.0, o)),
                (None, None) => None,
            }
        })
    })
}

/// Estimate USD cost from token counts and list rates.
pub fn estimate_token_cost_usd(
    input_tokens: u64,
    output_tokens: u64,
    input_per_1m: f64,
    output_per_1m: f64,
) -> f64 {
    estimate_token_cost_usd_with_cache(
        input_tokens,
        output_tokens,
        0,
        0,
        input_per_1m,
        output_per_1m,
        None,
        None,
    )
}

/// Split total/prompt tokens into billable non-cached vs cached portions.
///
/// OpenAI-compat (inclusive): `prompt_tokens` includes `cached_tokens`.
/// Anthropic-style (exclusive): `input` is non-cached; add cache fields separately.
pub fn billable_input_split(
    total_or_input: u64,
    cache_read: u64,
    cache_create: u64,
) -> (u64, u64, u64) {
    if cache_read > 0 && total_or_input >= cache_read {
        // Inclusive: only the non-cached tail is full-price.
        (
            total_or_input.saturating_sub(cache_read),
            cache_read,
            cache_create,
        )
    } else {
        // Exclusive or no cache detail.
        (total_or_input, cache_read, cache_create)
    }
}

/// USD cost with cache-aware rates.
///
/// `cache_input_per_1m` / `cache_write_per_1m` default to provider-aware values
/// when `None` (Charm Hyper: cached input free; else ~0 so we don't invent fees).
pub fn estimate_token_cost_usd_with_cache(
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_create_tokens: u64,
    input_per_1m: f64,
    output_per_1m: f64,
    cache_input_per_1m: Option<f64>,
    cache_write_per_1m: Option<f64>,
) -> f64 {
    let (non_cached, cached, created) =
        billable_input_split(input_tokens, cache_read_tokens, cache_create_tokens);
    let cache_in = cache_input_per_1m.unwrap_or(0.0);
    let cache_write = cache_write_per_1m.unwrap_or(input_per_1m);
    (non_cached as f64 / 1_000_000.0) * input_per_1m
        + (cached as f64 / 1_000_000.0) * cache_in
        + (created as f64 / 1_000_000.0) * cache_write
        + (output_tokens as f64 / 1_000_000.0) * output_per_1m
}

/// Optional cache list rates (USD / 1M) for a provider.
/// Charm Hyper model card: `1m_in_cache = 0`, `1m_out_cache = 0.14`.
pub fn model_cache_list_pricing(provider_id: &str) -> Option<(f64, f64)> {
    match canonical_provider_id(provider_id) {
        "charm-hyper" => Some((0.0, 0.0)), // input cache free; write treated as full input
        _ => None,
    }
}

/// Whether this provider bills in prepaid credits (not pure card-to-API).
pub fn provider_uses_credits(provider_id: &str) -> bool {
    matches!(
        canonical_provider_id(provider_id),
        "charm-hyper" | "commandcode"
    )
}

/// Credit unit label for prepaid providers.
pub fn provider_credit_unit(provider_id: &str) -> Option<&'static str> {
    match canonical_provider_id(provider_id) {
        "charm-hyper" => Some("hypercredits"),
        "commandcode" => Some("credits"),
        _ => None,
    }
}

/// Convert USD list-rate spend into the provider's prepaid credit unit.
///
/// Charm Hyper FAQ: **1 Hypercredit = $0.05**.
pub fn usd_to_provider_credits(provider_id: &str, usd: f64) -> Option<f64> {
    match canonical_provider_id(provider_id) {
        "charm-hyper" => Some(usd / 0.05),
        _ => None,
    }
}

/// Whether the tool prompt manifest should be included for the selected model,
/// based on harness config and provider/model settings.
pub(crate) fn effective_tool_prompt_manifest(config: &NaviConfig) -> bool {
    use crate::config::types::ToolPromptManifest;

    match config.harness.tool_prompt_manifest {
        ToolPromptManifest::Always => return true,
        ToolPromptManifest::Never => return false,
        ToolPromptManifest::Auto => {}
    }

    match effective_tool_calling_mode(config) {
        ToolCallingMode::TextExtracted | ToolCallingMode::ManifestOnly => return true,
        ToolCallingMode::Disabled => return false,
        ToolCallingMode::Native => {}
    }

    let selected_provider = &config.model.provider;
    let selected_model = &config.model.name;
    provider_catalog(config)
        .into_iter()
        .find(|provider| {
            canonical_provider_id(&provider.id) == canonical_provider_id(selected_provider)
        })
        .and_then(|provider| {
            provider
                .models
                .iter()
                .find(|model| model.name == *selected_model)
                .and_then(|model| model.tool_prompt_manifest)
                .or(provider.tool_prompt_manifest)
        })
        .unwrap_or(false)
}

/// Returns the selected provider's resolved tool calling compatibility mode.
pub fn effective_tool_calling_mode(config: &NaviConfig) -> ToolCallingMode {
    let selected_provider = &config.model.provider;
    provider_catalog(config)
        .into_iter()
        .find(|provider| {
            canonical_provider_id(&provider.id) == canonical_provider_id(selected_provider)
        })
        .and_then(|provider| provider.tool_calling_mode)
        .unwrap_or(ToolCallingMode::Native)
}

/// Returns whether a configured model can consume the given attachment kind
/// directly.
///
/// Lookup order:
/// 1. exact / alias match on the selected provider
/// 2. cross-provider alias match (e.g. opencode `mimo-v2.5-free` → xiaomi `mimo-v2.5`)
/// 3. family stem match (`grok-4.5` → `grok-4` / `grok-4.3`)
/// 4. provider attachment defaults (xAI/Gemini/Anthropic default to vision)
///
/// Explicit `supports_images = false` always wins over defaults. Only when the
/// registry/SQLite cache has no flag (common for newly listed models like
/// `grok-4.5`) do we fall back — treating unknown as unsupported was stripping
/// multimodal content from models that clearly support it.
pub fn model_supports_attachment(
    config: &NaviConfig,
    provider_id: &str,
    model_name: &str,
    kind: AttachmentKind,
) -> bool {
    let catalog = provider_catalog(config);
    let candidates = model_attachment_name_candidates(model_name);
    let provider_id = canonical_provider_id(provider_id);

    if let Some(flag) = lookup_attachment_support(&catalog, Some(provider_id), &candidates, kind) {
        return flag;
    }

    // Cross-provider: e.g. opencode `mimo-v2.5-free` inherits vision from xiaomi `mimo-v2.5`.
    if let Some(flag) = lookup_attachment_support(&catalog, None, &candidates, kind) {
        return flag;
    }

    // Family stem: `grok-4.5` / `x-ai/grok-4.5` inherit from catalogued `grok-4`.
    let family = model_attachment_family_candidates(model_name);
    if !family.is_empty() {
        if let Some(flag) = lookup_attachment_support(&catalog, Some(provider_id), &family, kind) {
            return flag;
        }
        if let Some(flag) = lookup_attachment_support(&catalog, None, &family, kind) {
            return flag;
        }
    }

    // Provider-level defaults from the registry snapshot (e.g. xAI images=true).
    if let Some(flag) = provider_attachment_default(provider_id, kind) {
        return flag;
    }

    false
}

/// Registry `defaults.attachments` for providers that publish a family-wide default.
///
/// Used when a model is missing from the catalog or has no per-modality flag
/// (dynamically listed models after `sync models`).
fn provider_attachment_default(provider_id: &str, kind: AttachmentKind) -> Option<bool> {
    // Keep in sync with registry-snapshot/providers/*/defaults.attachments.
    let (images, audio, video, documents) = match canonical_provider_id(provider_id) {
        "xai" => (true, false, false, false),
        "google-gemini" | "gemini" => (true, true, true, true),
        "anthropic" => (true, false, false, true),
        "openai" => (true, false, false, false),
        // OpenRouter / aggregators are mixed; never invent a family-wide true.
        _ => return None,
    };
    Some(match kind {
        AttachmentKind::Image => images,
        AttachmentKind::Audio => audio,
        AttachmentKind::Video => video,
        AttachmentKind::Document => documents,
    })
}

fn attachment_flag(model: &ProviderModelConfig, kind: AttachmentKind) -> Option<bool> {
    match kind {
        AttachmentKind::Image => model.supports_images,
        AttachmentKind::Audio => model.supports_audio,
        AttachmentKind::Video => model.supports_video,
        AttachmentKind::Document => model.supports_documents,
    }
}

fn lookup_attachment_support(
    catalog: &[ProviderConfig],
    provider_id: Option<&str>,
    candidates: &[String],
    kind: AttachmentKind,
) -> Option<bool> {
    let mut saw_explicit_false = false;
    for provider in catalog {
        if let Some(want) = provider_id {
            if canonical_provider_id(&provider.id) != want {
                continue;
            }
        }
        for model in &provider.models {
            if !candidates
                .iter()
                .any(|candidate| model_names_equivalent(&model.name, candidate))
            {
                continue;
            }
            match attachment_flag(model, kind) {
                Some(true) => return Some(true),
                Some(false) => saw_explicit_false = true,
                None => {}
            }
        }
    }
    if saw_explicit_false {
        Some(false)
    } else {
        None
    }
}

/// Name variants used when resolving attachment capability across registries.
pub(crate) fn model_attachment_name_candidates(model_name: &str) -> Vec<String> {
    fn push_unique(out: &mut Vec<String>, value: String) {
        if !value.is_empty() && !out.iter().any(|existing| existing == &value) {
            out.push(value);
        }
    }

    let mut out = Vec::new();
    push_unique(&mut out, model_name.to_string());
    push_unique(&mut out, model_name.to_ascii_lowercase());

    if let Some((_, rest)) = model_name.split_once('/') {
        push_unique(&mut out, rest.to_string());
        push_unique(&mut out, rest.to_ascii_lowercase());
    }

    let bases = out.clone();
    for base in bases {
        for suffix in ["-free", "-highspeed", "-high-speed", "-turbo"] {
            let lower = base.to_ascii_lowercase();
            if let Some(stripped) = lower.strip_suffix(suffix) {
                if base.len() >= suffix.len() {
                    push_unique(&mut out, base[..base.len() - suffix.len()].to_string());
                }
                push_unique(&mut out, stripped.to_string());
            }
        }
    }

    // MiniMaxAI/MiniMax-M3 → minimax-m3 / MiniMax-M3
    let bases = out.clone();
    for base in bases {
        let compact = base.replace('_', "-");
        push_unique(&mut out, compact.clone());
        push_unique(&mut out, compact.to_ascii_lowercase());
    }

    out
}

/// Broader family stems for capability inheritance when a specific SKU is
/// missing from the registry (e.g. `grok-4.5` → `grok-4`, `grok-4.3`).
///
/// Does **not** include the original full name — callers already try exact
/// candidates first. Order is longest stem first so a closer sibling wins.
pub(crate) fn model_attachment_family_candidates(model_name: &str) -> Vec<String> {
    fn push_unique(out: &mut Vec<String>, value: String) {
        if !value.is_empty() && !out.iter().any(|existing| existing == &value) {
            out.push(value);
        }
    }

    let leaf = model_name
        .rsplit('/')
        .next()
        .unwrap_or(model_name)
        .replace('_', "-");
    let lower = leaf.to_ascii_lowercase();

    let mut stems = Vec::new();
    // Drop trailing date/build suffixes: grok-2-vision-1212 → grok-2-vision
    let mut cur = lower.as_str();
    loop {
        let next = if let Some((head, tail)) = cur.rsplit_once('-') {
            // Only peel pure numeric / dotted version tails (4.5, 1212, 0309).
            if tail.chars().all(|c| c.is_ascii_digit() || c == '.') && !head.is_empty() {
                Some(head)
            } else if tail
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == 'x')
                && tail.contains('.')
                && !head.is_empty()
            {
                Some(head)
            } else {
                None
            }
        } else {
            None
        };
        // Also peel dotted minor versions glued without hyphen: handled below.
        match next {
            Some(head) => {
                push_unique(&mut stems, head.to_string());
                cur = head;
            }
            None => break,
        }
    }

    // Peel dotted version segments: grok-4.5 → grok-4, glm-5.2 → glm-5
    cur = lower.as_str();
    while let Some((head, tail)) = cur.rsplit_once('.') {
        if !tail.is_empty()
            && tail.chars().all(|c| c.is_ascii_digit())
            && !head.is_empty()
            && head
                .chars()
                .last()
                .is_some_and(|c| c.is_ascii_digit() || c.is_ascii_alphanumeric())
        {
            push_unique(&mut stems, head.to_string());
            // Continue peeling only if head still ends with a version-like token.
            if head.contains('.') || head.chars().last().is_some_and(|c| c.is_ascii_digit()) {
                cur = head;
                continue;
            }
        }
        break;
    }

    // Prefer longer stems first.
    stems.sort_by_key(|s| std::cmp::Reverse(s.len()));
    stems
}

fn model_names_equivalent(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let norm = |value: &str| {
        value
            .rsplit('/')
            .next()
            .unwrap_or(value)
            .replace('_', "-")
            .to_ascii_lowercase()
    };
    norm(left) == norm(right)
}

impl NaviConfig {
    /// Updates the model list for a provider, merging with existing model metadata
    /// from the registry or built-in catalog.
    pub fn update_provider_models(&mut self, provider_id: &str, model_names: &[String]) {
        let mut existing_models = std::collections::HashMap::new();

        let provider_id = canonical_provider_id(provider_id).to_string();

        // Start with user overrides as a fallback for custom models.
        if let Some(existing_override) = self
            .providers
            .iter()
            .find(|p| canonical_provider_id(&p.id) == provider_id)
        {
            for m in &existing_override.models {
                existing_models.insert(m.name.clone(), m.clone());
            }
        }

        // Registry metadata is authoritative for models it knows about. This
        // lets `sync models` refresh stale context windows saved in config.
        if let Some(registry_provider) = base_provider_catalog()
            .into_iter()
            .find(|p| canonical_provider_id(&p.id) == provider_id)
        {
            for m in registry_provider.models {
                existing_models.insert(m.name.clone(), m);
            }
        }

        let mut new_models = Vec::new();
        for name in model_names {
            let mut model = if let Some(model) = existing_models.get(name) {
                let mut model = model.clone();
                model.name = name.clone();
                model
            } else {
                // Family inheritance for config-layer fields (context, thinking)
                // from siblings already known on this provider.
                let family = model_attachment_family_candidates(name);
                let donor = existing_models.values().find(|m| {
                    family.iter().any(|stem| {
                        let leaf = m
                            .name
                            .rsplit('/')
                            .next()
                            .unwrap_or(&m.name)
                            .to_ascii_lowercase();
                        let stem_l = stem.to_ascii_lowercase();
                        leaf == stem_l
                            || leaf.starts_with(&format!("{stem_l}-"))
                            || leaf.starts_with(&format!("{stem_l}."))
                    })
                });
                ProviderModelConfig {
                    name: name.clone(),
                    task_size: donor
                        .and_then(|d| d.task_size)
                        .or_else(|| registry::determine_task_size(name)),
                    context_window_tokens: donor.and_then(|d| d.context_window_tokens),
                    max_output_tokens: donor.and_then(|d| d.max_output_tokens),
                    recommended_temperature: donor.and_then(|d| d.recommended_temperature),
                    supports_thinking: donor.and_then(|d| d.supports_thinking),
                    supports_images: donor.and_then(|d| d.supports_images),
                    supports_audio: donor.and_then(|d| d.supports_audio),
                    supports_video: donor.and_then(|d| d.supports_video),
                    supports_documents: donor.and_then(|d| d.supports_documents),
                    tool_prompt_manifest: donor.and_then(|d| d.tool_prompt_manifest),
                    pricing_input_per_1m: donor.and_then(|d| d.pricing_input_per_1m),
                    pricing_output_per_1m: donor.and_then(|d| d.pricing_output_per_1m),
                    reasoning_levels: donor
                        .map(|d| d.reasoning_levels.clone())
                        .unwrap_or_default(),
                    default_reasoning_effort: donor
                        .and_then(|d| d.default_reasoning_effort.clone()),
                }
            };
            // Always fill remaining None modality flags from provider defaults
            // (covers both brand-new SKUs and stale cache rows with NULL flags).
            crate::registry::apply_provider_attachment_defaults_to_config_model(
                &mut model,
                &provider_id,
            );
            new_models.push(model);
        }

        if let Some(p) = self
            .providers
            .iter_mut()
            .find(|p| canonical_provider_id(&p.id) == provider_id)
        {
            p.id = provider_id.clone();
            p.models = new_models;
        } else {
            if let Some(mut resolved) = resolve_provider_config(self, &provider_id) {
                resolved.models = new_models;
                self.providers.push(resolved);
            } else {
                self.providers.push(ProviderConfig {
                    id: provider_id.to_string(),
                    label: provider_id.to_string(),
                    description: "Synced dynamically".to_string(),
                    kind: ProviderKind::OpenAiChatCompletions,
                    api_key_env: format!(
                        "{}_API_KEY",
                        provider_id.to_uppercase().replace('-', "_")
                    ),
                    base_url: None,
                    models: new_models,
                    ..Default::default()
                });
            }
        }
    }
}

pub(crate) fn merge_provider_configs(
    providers: &mut Vec<ProviderConfig>,
    overrides: Vec<ProviderConfig>,
) {
    for override_config in overrides {
        if let Some(existing) = providers.iter_mut().find(|provider| {
            canonical_provider_id(&provider.id) == canonical_provider_id(&override_config.id)
        }) {
            // Merge models by name: preserve registry metadata (context_window_tokens,
            // max_output_tokens, recommended_temperature, supports_thinking,
            // tool_prompt_manifest) when the user override doesn't specify them.
            let mut existing_models: std::collections::HashMap<String, ProviderModelConfig> =
                existing
                    .models
                    .drain(..)
                    .map(|m| (m.name.clone(), m))
                    .collect();

            let mut merged_models = Vec::new();
            for override_model in override_config.models {
                // Try exact match first, then case-insensitive match so that
                // user config overrides with different casing (e.g. "glm-5.2"
                // vs registry "GLM-5.2") still inherit registry metadata.
                let match_key = override_model.name.clone();
                let matched = existing_models.remove(&match_key).or_else(|| {
                    let lower = match_key.to_lowercase();
                    existing_models
                        .keys()
                        .find(|k| k.to_lowercase() == lower)
                        .cloned()
                        .and_then(|k| existing_models.remove(&k))
                });

                if let Some(registry_model) = matched {
                    merged_models.push(ProviderModelConfig {
                        name: override_model.name,
                        task_size: override_model.task_size.or(registry_model.task_size),
                        context_window_tokens: override_model
                            .context_window_tokens
                            .or(registry_model.context_window_tokens),
                        max_output_tokens: override_model
                            .max_output_tokens
                            .or(registry_model.max_output_tokens),
                        recommended_temperature: override_model
                            .recommended_temperature
                            .or(registry_model.recommended_temperature),
                        supports_thinking: override_model
                            .supports_thinking
                            .or(registry_model.supports_thinking),
                        reasoning_levels: if override_model.reasoning_levels.is_empty() {
                            registry_model.reasoning_levels
                        } else {
                            override_model.reasoning_levels
                        },
                        default_reasoning_effort: override_model
                            .default_reasoning_effort
                            .or(registry_model.default_reasoning_effort),
                        supports_images: override_model
                            .supports_images
                            .or(registry_model.supports_images),
                        supports_audio: override_model
                            .supports_audio
                            .or(registry_model.supports_audio),
                        supports_video: override_model
                            .supports_video
                            .or(registry_model.supports_video),
                        supports_documents: override_model
                            .supports_documents
                            .or(registry_model.supports_documents),
                        tool_prompt_manifest: override_model
                            .tool_prompt_manifest
                            .or(registry_model.tool_prompt_manifest),
                        pricing_input_per_1m: override_model
                            .pricing_input_per_1m
                            .or(registry_model.pricing_input_per_1m),
                        pricing_output_per_1m: override_model
                            .pricing_output_per_1m
                            .or(registry_model.pricing_output_per_1m),
                    });
                } else {
                    merged_models.push(override_model);
                }
            }

            // Preserve registry models that the user override didn't list so
            // that newly added registry models appear even when the config
            // override only specifies a subset of models.
            for (_, registry_model) in existing_models.into_iter() {
                merged_models.push(registry_model);
            }

            // Override provider-level fields, keep merged models.
            existing.id = canonical_provider_id(&existing.id).to_string();
            existing.label = override_config.label;
            existing.description = override_config.description;
            existing.kind = override_config.kind;
            existing.api_key_env = override_config.api_key_env;
            if override_config.base_url.is_some() {
                existing.base_url = override_config.base_url;
            }
            existing.models = merged_models;
            if override_config.request_options.is_some() {
                existing.request_options = override_config.request_options;
            }
            if override_config.request_timeout_ms.is_some() {
                existing.request_timeout_ms = override_config.request_timeout_ms;
            }
            if override_config.request_max_retries.is_some() {
                existing.request_max_retries = override_config.request_max_retries;
            }
            if override_config.stream_idle_timeout_ms.is_some() {
                existing.stream_idle_timeout_ms = override_config.stream_idle_timeout_ms;
            }
            if override_config.stream_max_retries.is_some() {
                existing.stream_max_retries = override_config.stream_max_retries;
            }
            if override_config.websocket_connect_timeout_ms.is_some() {
                existing.websocket_connect_timeout_ms =
                    override_config.websocket_connect_timeout_ms;
            }
            if override_config.retry_429.is_some() {
                existing.retry_429 = override_config.retry_429;
            }
            if override_config.tool_prompt_manifest.is_some() {
                existing.tool_prompt_manifest = override_config.tool_prompt_manifest;
            }
            if override_config.tool_calling_mode.is_some() {
                existing.tool_calling_mode = override_config.tool_calling_mode;
            }
            // Preserve aggregator flag from the registry — user overrides
            // shouldn't disable it since it controls dynamic model sync.
            existing.aggregator = existing.aggregator || override_config.aggregator;
        } else {
            providers.push(override_config);
        }
    }
}

/// Fills in the canonical default [`ProviderRequestOptions`] for any provider
/// whose `request_options` field is `None`. This guarantees that prompt
/// caching stays enabled for known providers (OpenAI, Anthropic) even when:
///   * the local registry cache is stale and ships no `request_options`
///   * a user override in `config.toml` replaces the provider wholesale
///     without setting `request_options`
///
/// Providers that explicitly carry `Some(opts)` keep the user's configuration
/// verbatim — including the empty `ProviderRequestOptions` value that opts
/// out of prompt caching.
fn apply_default_request_options(providers: &mut [ProviderConfig]) {
    for provider in providers {
        let id = canonical_provider_id(&provider.id);
        if provider.request_options.is_none()
            && let Some(defaults) = default_request_options_for(id)
        {
            provider.request_options = Some(defaults);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        billable_input_split, effective_tool_calling_mode, effective_tool_prompt_manifest,
        estimate_token_cost_usd_with_cache, model_cache_list_pricing,
    };
    use crate::config::types::{
        ModelTaskSize, NaviConfig, ProviderConfig, ProviderModelConfig, ToolCallingMode,
    };

    #[test]
    fn billable_input_split_inclusive_openai_style() {
        // prompt_tokens includes cached_tokens
        assert_eq!(billable_input_split(22_000, 21_500, 0), (500, 21_500, 0));
    }

    #[test]
    fn billable_input_split_exclusive_anthropic_style() {
        // input is non-cached only; cache fields separate
        assert_eq!(billable_input_split(500, 21_500, 100), (500, 21_500, 100));
    }

    #[test]
    fn hyper_cache_read_costs_zero_on_list_rates() {
        // glm-5.2 list rates; 99% cache hit should only bill the tail + output.
        let cost = estimate_token_cost_usd_with_cache(
            22_000,
            100,
            21_780, // cache read
            0,
            1.4,       // input / 1M
            4.4,       // output / 1M
            Some(0.0), // Hyper cached input free
            Some(1.4),
        );
        // non-cached 220 * 1.4/1M + output 100 * 4.4/1M
        let expected = (220.0 / 1_000_000.0) * 1.4 + (100.0 / 1_000_000.0) * 4.4;
        assert!(
            (cost - expected).abs() < 1e-12,
            "cost={cost} expected={expected}"
        );
        // Sanity: full-price would be ~0.031; cache-aware is ~0.0007
        assert!(cost < 0.002);
        assert_eq!(model_cache_list_pricing("charm-hyper"), Some((0.0, 0.0)));
    }

    #[test]
    fn update_provider_models_inherits_defaults_for_new_xai_sku() {
        let mut config = NaviConfig::default();
        // Simulate Ctrl+R discovering grok-4.5 before the registry snapshot knows it.
        config.update_provider_models("xai", &["grok-4.5".to_string()]);
        let xai = config
            .providers
            .iter()
            .find(|p| p.id == "xai")
            .expect("xai provider created by sync");
        let grok = xai
            .models
            .iter()
            .find(|m| m.name == "grok-4.5")
            .expect("grok-4.5 listed");
        assert_eq!(
            grok.supports_images,
            Some(true),
            "sync must seed xAI vision default onto new SKUs"
        );
    }

    #[test]
    fn base_catalog_cache_avoids_repeated_reload() {
        use std::sync::Arc;

        use crate::config::providers::{
            base_provider_catalog, invalidate_registry_catalog_cache, set_registry_store,
        };
        use crate::registry::RegistryStore;

        let dir = tempfile::tempdir().expect("tempdir");
        let store = RegistryStore::open(dir.path()).expect("open store");
        // Seed from embedded so cache has content.
        let _ = crate::registry::load_registry(&store);
        set_registry_store(Arc::new(store));
        invalidate_registry_catalog_cache();

        let first = base_provider_catalog();
        assert!(!first.is_empty(), "expected seeded catalog");
        let second = base_provider_catalog();
        assert_eq!(
            first.len(),
            second.len(),
            "cached catalog should return same provider count"
        );
        // Invalidate forces a fresh path without panicking.
        invalidate_registry_catalog_cache();
        let third = base_provider_catalog();
        assert_eq!(first.len(), third.len());
    }

    #[test]
    fn update_provider_models_prefers_registry_metadata_over_stale_override() {
        let mut config = NaviConfig::default();
        config.providers.push(ProviderConfig {
            id: "commandcode".to_string(),
            models: vec![ProviderModelConfig {
                name: "claude-sonnet-4-6".to_string(),
                task_size: Some(ModelTaskSize::Large),
                context_window_tokens: Some(128_000),
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
                reasoning_levels: Vec::new(),
                default_reasoning_effort: None,
            }],
            ..Default::default()
        });

        config.update_provider_models("commandcode", &["claude-sonnet-4-6".to_string()]);

        let provider = config
            .providers
            .iter()
            .find(|provider| provider.id == "commandcode")
            .expect("commandcode override");
        assert_eq!(provider.models[0].context_window_tokens, Some(1_000_000));
    }

    #[test]
    fn commandcode_uses_native_tool_mode() {
        let mut config = NaviConfig::default();
        config.model.provider = "commandcode".to_string();

        assert_eq!(
            effective_tool_calling_mode(&config),
            ToolCallingMode::Native
        );
        assert!(!effective_tool_prompt_manifest(&config));
    }

    #[test]
    fn merge_preserves_registry_models_not_in_override() {
        let registry_provider = ProviderConfig {
            id: "test-merge".to_string(),
            models: vec![
                ProviderModelConfig {
                    name: "model-a".to_string(),
                    task_size: Some(ModelTaskSize::Large),
                    context_window_tokens: Some(128_000),
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
                    reasoning_levels: Vec::new(),
                    default_reasoning_effort: None,
                },
                ProviderModelConfig {
                    name: "model-b".to_string(),
                    task_size: Some(ModelTaskSize::Small),
                    context_window_tokens: Some(64_000),
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
                    reasoning_levels: Vec::new(),
                    default_reasoning_effort: None,
                },
                ProviderModelConfig {
                    name: "model-c".to_string(),
                    task_size: Some(ModelTaskSize::Large),
                    context_window_tokens: Some(200_000),
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
                    reasoning_levels: Vec::new(),
                    default_reasoning_effort: None,
                },
            ],
            ..Default::default()
        };

        let override_provider = ProviderConfig {
            id: "test-merge".to_string(),
            models: vec![ProviderModelConfig {
                name: "model-a".to_string(),
                task_size: Some(ModelTaskSize::Large),
                context_window_tokens: None,
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
                reasoning_levels: Vec::new(),
                default_reasoning_effort: None,
            }],
            ..Default::default()
        };

        let mut providers = vec![registry_provider];
        super::merge_provider_configs(&mut providers, vec![override_provider]);

        let merged = &providers[0];
        let names: Vec<&str> = merged.models.iter().map(|m| m.name.as_str()).collect();
        assert!(
            names.contains(&"model-a"),
            "override model should be present"
        );
        assert!(
            names.contains(&"model-b"),
            "registry-only model should be preserved"
        );
        assert!(
            names.contains(&"model-c"),
            "registry-only model should be preserved"
        );

        let model_a = merged
            .models
            .iter()
            .find(|m| m.name == "model-a")
            .expect("model-a present");
        assert_eq!(
            model_a.context_window_tokens,
            Some(128_000),
            "override should inherit registry metadata for matching models"
        );
    }

    // ── canonical_provider_id ────────────────────────────────────────────

    #[test]
    fn canonical_provider_id_maps_zen_alias() {
        assert_eq!(super::canonical_provider_id("opencode-zen"), "opencode");
    }

    #[test]
    fn canonical_provider_id_passes_through_known() {
        assert_eq!(super::canonical_provider_id("opencode"), "opencode");
        assert_eq!(super::canonical_provider_id("opencode-go"), "opencode-go");
        assert_eq!(super::canonical_provider_id("openai"), "openai");
        assert_eq!(super::canonical_provider_id("anthropic"), "anthropic");
    }

    #[test]
    fn canonical_provider_id_passes_through_unknown() {
        assert_eq!(
            super::canonical_provider_id("custom-provider"),
            "custom-provider"
        );
        assert_eq!(super::canonical_provider_id(""), "");
    }
}
