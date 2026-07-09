//! Dynamic model sync for aggregator providers (e.g. OpenRouter).
//!
//! Aggregator providers expose a `/models` endpoint that returns the full
//! model catalog with metadata. This module fetches that catalog, parses it
//! into [`RegistryModel`] values with capability tags (free, nitro, online),
//! and upserts them into the SQLite registry cache.

use anyhow::{Context, Result};
use std::collections::HashSet;

use crate::config::types::ProviderConfig;
use crate::registry::store::RegistryStore;
use crate::registry::types::{
    RegistryAttachments, RegistryModel, RegistryModelPricing, RegistryProvider,
};

/// OpenRouter `/models` response item.
#[derive(Debug, serde::Deserialize)]
struct OpenRouterModel {
    id: String,
    #[serde(default)]
    context_length: Option<u64>,
    #[serde(default)]
    architecture: Option<OpenRouterArchitecture>,
    #[serde(default)]
    top_provider: Option<OpenRouterTopProvider>,
    #[serde(default)]
    supported_parameters: Vec<String>,
    #[serde(default)]
    reasoning: Option<OpenRouterReasoning>,
    #[serde(default)]
    pricing: Option<OpenRouterPricing>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct OpenRouterPricing {
    /// USD per token (not per 1M) as returned by OpenRouter.
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    completion: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct OpenRouterArchitecture {
    #[serde(default)]
    input_modalities: Vec<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct OpenRouterTopProvider {
    #[serde(default)]
    max_completion_tokens: Option<u64>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct OpenRouterReasoning {
    #[serde(default)]
    mandatory: Option<bool>,
    #[serde(default)]
    default_enabled: Option<bool>,
}

/// Tags extracted from OpenRouter model IDs (e.g. `:free`, `:nitro`).
fn extract_tags(id: &str) -> Vec<String> {
    let mut tags = Vec::new();
    if id.ends_with(":free") {
        tags.push("free".to_string());
    }
    if id.ends_with(":nitro") {
        tags.push("nitro".to_string());
    }
    if id.ends_with(":online") {
        tags.push("online".to_string());
    }
    tags
}

fn openrouter_pricing(model: &OpenRouterModel) -> Option<RegistryModelPricing> {
    let pricing = model.pricing.as_ref()?;
    let parse = |s: &Option<String>| -> Option<f64> {
        let raw = s.as_ref()?;
        let per_token: f64 = raw.parse().ok()?;
        // OpenRouter returns USD per token; convert to per 1M tokens.
        Some(per_token * 1_000_000.0)
    };
    let input = parse(&pricing.prompt);
    let output = parse(&pricing.completion);
    if input.is_none() && output.is_none() {
        return None;
    }
    Some(RegistryModelPricing {
        input_per_1m: input,
        output_per_1m: output,
        currency: Some("USD".into()),
    })
}

/// Strips `:free`, `:nitro`, `:online` suffixes from the model ID.
fn strip_tags(id: &str) -> String {
    for suffix in [":free", ":nitro", ":online"] {
        if let Some(stripped) = id.strip_suffix(suffix) {
            return stripped.to_string();
        }
    }
    id.to_string()
}

/// Parses modalities like "text+image+file->text" into attachment support.
fn parse_attachments(arch: &OpenRouterArchitecture) -> RegistryAttachments {
    let inputs: HashSet<&str> = arch.input_modalities.iter().map(|s| s.as_str()).collect();
    RegistryAttachments {
        images: if inputs.contains("image") {
            Some(true)
        } else {
            None
        },
        audio: if inputs.contains("audio") {
            Some(true)
        } else {
            None
        },
        video: if inputs.contains("video") {
            Some(true)
        } else {
            None
        },
        documents: if inputs.contains("file") {
            Some(true)
        } else {
            None
        },
    }
}

/// Converts an OpenRouter API model into a [`RegistryModel`].
fn openrouter_model_to_registry(model: &OpenRouterModel) -> RegistryModel {
    let tags = extract_tags(&model.id);
    let name = strip_tags(&model.id);

    let supports_thinking = model
        .reasoning
        .as_ref()
        .and_then(|r| r.default_enabled.or(r.mandatory));

    let supports_tools = model
        .supported_parameters
        .iter()
        .any(|p| p == "tools" || p == "tool_choice");

    let mut capabilities = tags;
    if supports_tools {
        capabilities.push("tool_calling".to_string());
    }

    let attachments = model
        .architecture
        .as_ref()
        .map(parse_attachments)
        .unwrap_or_default();

    // OpenRouter wire effort set when reasoning is available.
    let reasoning_levels = if supports_thinking == Some(true) {
        vec![
            "none".into(),
            "low".into(),
            "medium".into(),
            "high".into(),
            "xhigh".into(),
        ]
    } else {
        Vec::new()
    };

    RegistryModel {
        name,
        task_size: None,
        context_window_tokens: model.context_length,
        max_output_tokens: model
            .top_provider
            .as_ref()
            .and_then(|tp| tp.max_completion_tokens),
        recommended_temperature: None,
        supports_thinking,
        reasoning_levels,
        default_reasoning_effort: supports_thinking
            .filter(|v| *v)
            .map(|_| "medium".to_string()),
        supports_attachments: None,
        supports_images: attachments.images,
        supports_audio: attachments.audio,
        supports_video: attachments.video,
        supports_documents: attachments.documents,
        attachments,
        capabilities,
        pricing: openrouter_pricing(model),
    }
}

/// Fetches the model list from an aggregator provider's `/models` endpoint
/// and upserts the results into the SQLite registry cache.
///
/// Returns the number of models synced.
pub async fn sync_aggregator_models(
    store: &RegistryStore,
    provider_config: &ProviderConfig,
    api_key: &str,
) -> Result<usize> {
    let base_url = provider_config
        .base_url
        .as_deref()
        .context("aggregator provider missing base_url")?
        .trim_end_matches('/');

    let url = format!("{base_url}/models");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent("navi-registry-fetcher/1.0")
        .build()
        .context("failed to build HTTP client for aggregator sync")?;

    let mut req = client.get(&url).bearer_auth(api_key);

    // OpenRouter requires extra headers.
    if provider_config.id == "openrouter" {
        req = req
            .header("HTTP-Referer", "https://github.com/enrell/navi")
            .header("X-Title", "Navi");
    }

    let resp = req
        .send()
        .await
        .with_context(|| format!("aggregator models request failed: {url}"))?;

    let resp = resp
        .error_for_status()
        .with_context(|| format!("aggregator models HTTP error: {url}"))?;

    #[derive(serde::Deserialize)]
    struct ModelsResponse {
        data: Vec<OpenRouterModel>,
    }

    let body: ModelsResponse = resp
        .json()
        .await
        .context("failed to parse aggregator models JSON")?;

    let models: Vec<RegistryModel> = body.data.iter().map(openrouter_model_to_registry).collect();

    // Merge with existing cached models to preserve metadata (context_window,
    // max_output, etc) that the API may not return, and to keep models that
    // exist in the cache but were not returned by the API.
    let existing = store
        .load_provider_models(&provider_config.id)
        .unwrap_or_default();
    let mut merged: Vec<RegistryModel> = models
        .into_iter()
        .map(|mut m| {
            // Try exact match first, then case-insensitive match.
            if let Some(cached) = existing.get(&m.name) {
                merge_model_metadata(&mut m, cached);
            } else {
                let lower = m.name.to_lowercase();
                if let Some((_, cached)) = existing.iter().find(|(k, _)| k.to_lowercase() == lower)
                {
                    merge_model_metadata(&mut m, cached);
                }
            }
            m
        })
        .collect();

    // Keep models that are in the cache but not returned by the API.
    let api_names: HashSet<String> = merged.iter().map(|m| m.name.to_lowercase()).collect();
    for (name, cached) in &existing {
        if !api_names.contains(&name.to_lowercase()) {
            merged.push(cached.clone());
        }
    }

    // Deduplicate by name (case-insensitive) to avoid UNIQUE constraint
    // failures when the API returns models that differ only in casing.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    merged.retain(|m| seen.insert(m.name.to_lowercase()));

    let model_count = merged.len();

    let registry_provider = RegistryProvider {
        id: provider_config.id.clone(),
        label: provider_config.label.clone(),
        description: provider_config.description.clone(),
        kind: format!("{:?}", provider_config.kind).to_lowercase(),
        api_key_env: provider_config.api_key_env.clone(),
        base_url: provider_config.base_url.clone(),
        tool_calling_mode: provider_config
            .tool_calling_mode
            .map(|m| format!("{:?}", m).to_lowercase()),
        aggregator: true,
        defaults: Default::default(),
        request_options: provider_config.request_options.clone().unwrap_or_default(),
        models: merged,
    };

    store.upsert_provider_with_sha256(&registry_provider, None)?;

    tracing::info!(
        provider = %provider_config.id,
        models = model_count,
        "aggregator model sync completed"
    );

    Ok(model_count)
}

/// Fills in missing metadata from the cached model. The API often returns
/// models without context_window_tokens, max_output_tokens, etc. We preserve
/// the richer metadata from the embedded snapshot or a previous sync.
fn merge_model_metadata(api_model: &mut RegistryModel, cached: &RegistryModel) {
    if api_model.context_window_tokens.is_none() {
        api_model.context_window_tokens = cached.context_window_tokens;
    }
    if api_model.max_output_tokens.is_none() {
        api_model.max_output_tokens = cached.max_output_tokens;
    }
    if api_model.recommended_temperature.is_none() {
        api_model.recommended_temperature = cached.recommended_temperature;
    }
    if api_model.task_size.is_none() {
        api_model.task_size = cached.task_size.clone();
    }
    if api_model.supports_thinking.is_none() {
        api_model.supports_thinking = cached.supports_thinking;
    }
    if api_model.reasoning_levels.is_empty() && !cached.reasoning_levels.is_empty() {
        api_model.reasoning_levels = cached.reasoning_levels.clone();
    }
    if api_model.default_reasoning_effort.is_none() {
        api_model.default_reasoning_effort = cached.default_reasoning_effort.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_free_tag() {
        assert_eq!(
            extract_tags("anthropic/claude-3.5-haiku:free"),
            vec!["free"]
        );
    }

    #[test]
    fn extract_nitro_tag() {
        assert_eq!(extract_tags("openai/gpt-4o:nitro"), vec!["nitro"]);
    }

    #[test]
    fn extract_no_tags() {
        assert!(extract_tags("anthropic/claude-opus-4").is_empty());
    }

    #[test]
    fn strip_free_suffix() {
        assert_eq!(
            strip_tags("anthropic/claude-3.5-haiku:free"),
            "anthropic/claude-3.5-haiku"
        );
    }

    #[test]
    fn strip_no_suffix() {
        assert_eq!(strip_tags("openai/gpt-5.5"), "openai/gpt-5.5");
    }

    #[test]
    fn parse_image_and_file_modalities() {
        let arch = OpenRouterArchitecture {
            input_modalities: vec!["text".into(), "image".into(), "file".into()],
        };
        let attachments = parse_attachments(&arch);
        assert_eq!(attachments.images, Some(true));
        assert_eq!(attachments.documents, Some(true));
        assert_eq!(attachments.audio, None);
        assert_eq!(attachments.video, None);
    }

    #[test]
    fn openrouter_model_gets_free_and_tool_tags() {
        let model = OpenRouterModel {
            id: "tencent/hy3:free".to_string(),
            context_length: Some(262144),
            architecture: None,
            top_provider: None,
            supported_parameters: vec!["tools".into(), "reasoning".into()],
            reasoning: Some(OpenRouterReasoning {
                mandatory: Some(false),
                default_enabled: Some(true),
            }),
            pricing: None,
        };
        let reg = openrouter_model_to_registry(&model);
        assert_eq!(reg.name, "tencent/hy3");
        assert!(reg.capabilities.contains(&"free".to_string()));
        assert!(reg.capabilities.contains(&"tool_calling".to_string()));
        assert_eq!(reg.supports_thinking, Some(true));
        assert_eq!(reg.context_window_tokens, Some(262144));
    }

    #[test]
    fn openrouter_pricing_converts_per_token_to_per_1m() {
        let model = OpenRouterModel {
            id: "openai/gpt-5".to_string(),
            context_length: Some(128000),
            architecture: None,
            top_provider: None,
            supported_parameters: Vec::new(),
            reasoning: None,
            pricing: Some(OpenRouterPricing {
                prompt: Some("0.000002".into()),     // $2 / 1M
                completion: Some("0.000006".into()), // $6 / 1M
            }),
        };
        let reg = openrouter_model_to_registry(&model);
        let pricing = reg.pricing.expect("pricing");
        assert!((pricing.input_per_1m.unwrap() - 2.0).abs() < 1e-9);
        assert!((pricing.output_per_1m.unwrap() - 6.0).abs() < 1e-9);
        assert_eq!(pricing.currency.as_deref(), Some("USD"));
    }
}
