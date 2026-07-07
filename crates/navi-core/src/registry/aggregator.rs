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
use crate::registry::types::{RegistryAttachments, RegistryModel, RegistryProvider};

/// OpenRouter `/models` response item.
#[derive(Debug, serde::Deserialize)]
struct OpenRouterModel {
    id: String,
    #[serde(default)]
    context_length: Option<u64>,
    #[serde(default)]
    architecture: Option<OpenRouterArchitecture>,
    #[serde(default)]
    pricing: Option<OpenRouterPricing>,
    #[serde(default)]
    top_provider: Option<OpenRouterTopProvider>,
    #[serde(default)]
    supported_parameters: Vec<String>,
    #[serde(default)]
    reasoning: Option<OpenRouterReasoning>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct OpenRouterArchitecture {
    #[serde(default)]
    modality: Option<String>,
    #[serde(default)]
    input_modalities: Vec<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct OpenRouterPricing {
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    completion: Option<String>,
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
        supports_attachments: None,
        supports_images: attachments.images,
        supports_audio: attachments.audio,
        supports_video: attachments.video,
        supports_documents: attachments.documents,
        attachments,
        capabilities,
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

    let model_count = models.len();

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
        models,
    };

    store.upsert_provider_with_sha256(&registry_provider, None)?;

    tracing::info!(
        provider = %provider_config.id,
        models = model_count,
        "aggregator model sync completed"
    );

    Ok(model_count)
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
            modality: Some("text+image+file->text".to_string()),
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
            pricing: None,
            top_provider: None,
            supported_parameters: vec!["tools".into(), "reasoning".into()],
            reasoning: Some(OpenRouterReasoning {
                mandatory: Some(false),
                default_enabled: Some(true),
            }),
        };
        let reg = openrouter_model_to_registry(&model);
        assert_eq!(reg.name, "tencent/hy3");
        assert!(reg.capabilities.contains(&"free".to_string()));
        assert!(reg.capabilities.contains(&"tool_calling".to_string()));
        assert_eq!(reg.supports_thinking, Some(true));
        assert_eq!(reg.context_window_tokens, Some(262144));
    }
}
