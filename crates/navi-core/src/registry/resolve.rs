//! Resolves `ref`-based model entries against the canonical model catalog.
//!
//! When a provider JSON uses `{ "ref": "gpt-5.4", "pricing": {...} }`, this
//! module merges the canonical model metadata from `models/gpt-5.4.json` with
//! the provider's overrides (pricing, api_name, status, etc.).

use std::collections::HashMap;

use super::types::{CanonicalModel, RegistryModel, RegistryProvider};

/// A loaded catalog of canonical models, keyed by model id.
pub type ModelCatalog = HashMap<String, CanonicalModel>;

/// Resolves all `ref`-based model entries in a provider against the catalog.
///
/// For each model in `provider.models`:
/// - If it has a `model_ref`, look up the canonical model and merge metadata.
/// - If it's a legacy inline model (with `name` set), leave it unchanged.
///
/// Models with unresolved refs are logged as warnings and kept with minimal data.
pub fn resolve_provider_refs(provider: &mut RegistryProvider, catalog: &ModelCatalog) {
    for model in &mut provider.models {
        resolve_model_ref(model, catalog);
    }
}

/// Resolve a single model entry's `ref` against the canonical catalog.
fn resolve_model_ref(model: &mut RegistryModel, catalog: &ModelCatalog) {
    let Some(ref model_ref) = model.model_ref else {
        // Legacy inline model — nothing to resolve.
        return;
    };

    // Look up canonical model by ref id, then try aliases.
    let canonical = catalog.get(model_ref.as_str()).or_else(|| {
        catalog
            .values()
            .find(|c| c.aliases.iter().any(|a| a == model_ref))
    });

    let Some(canonical) = canonical else {
        tracing::warn!(
            model_ref = %model_ref,
            "unresolved model ref — canonical model not found in catalog"
        );
        // Set name from ref so the model is at least usable.
        if model.name.is_empty() {
            model.name = model
                .api_name
                .clone()
                .unwrap_or_else(|| model_ref.clone());
        }
        return;
    };

    // Derive `name` from api_name or ref.
    if model.name.is_empty() {
        model.name = model
            .api_name
            .clone()
            .unwrap_or_else(|| canonical.id.clone());
    }

    // Merge canonical fields into the model, but provider overrides win.
    if model.context_window_tokens.is_none() {
        model.context_window_tokens = canonical.context_window_tokens;
    }
    if model.max_output_tokens.is_none() {
        model.max_output_tokens = canonical.max_output_tokens;
    }
    if model.recommended_temperature.is_none() {
        model.recommended_temperature = canonical.recommended_temperature;
    }
    if model.supports_thinking.is_none() {
        model.supports_thinking = canonical.supports_thinking;
    }
    if model.reasoning_levels.is_empty() {
        model.reasoning_levels.clone_from(&canonical.reasoning_levels);
    }
    if model.default_reasoning_effort.is_none() {
        model
            .default_reasoning_effort
            .clone_from(&canonical.default_reasoning_effort);
    }
    if model.capabilities.is_empty() {
        model.capabilities.clone_from(&canonical.capabilities);
    }

    // Merge attachments: canonical provides defaults, model-level overrides win.
    if model.attachments.is_empty() {
        model.attachments = canonical.attachments.clone();
    } else {
        // Only fill in None fields from canonical.
        if model.attachments.images.is_none() {
            model.attachments.images = canonical.attachments.images;
        }
        if model.attachments.audio.is_none() {
            model.attachments.audio = canonical.attachments.audio;
        }
        if model.attachments.video.is_none() {
            model.attachments.video = canonical.attachments.video;
        }
        if model.attachments.documents.is_none() {
            model.attachments.documents = canonical.attachments.documents;
        }
    }

    // Backfill legacy modality booleans from attachments for compatibility.
    if model.supports_images.is_none() {
        model.supports_images = canonical.attachments.images;
    }
    if model.supports_audio.is_none() {
        model.supports_audio = canonical.attachments.audio;
    }
    if model.supports_video.is_none() {
        model.supports_video = canonical.attachments.video;
    }
    if model.supports_documents.is_none() {
        model.supports_documents = canonical.attachments.documents;
    }
}

/// Builds an alias index from the catalog for quick lookups.
pub fn build_alias_index(catalog: &ModelCatalog) -> HashMap<String, String> {
    let mut index = HashMap::new();
    for (id, model) in catalog {
        for alias in &model.aliases {
            index.insert(alias.clone(), id.clone());
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::types::{CanonicalModel, RegistryAttachments, RegistryModel, RegistryProvider};

    fn catalog_with(id: &str, ctx: u64, images: bool) -> ModelCatalog {
        let mut catalog = ModelCatalog::new();
        catalog.insert(
            id.to_string(),
            CanonicalModel {
                id: id.to_string(),
                vendor: Some("openai".into()),
                family: Some("gpt".into()),
                label: None,
                description: None,
                context_window_tokens: Some(ctx),
                max_output_tokens: Some(32_768),
                recommended_temperature: Some(1.0),
                supports_thinking: Some(true),
                reasoning_levels: vec!["low".into(), "medium".into(), "high".into()],
                default_reasoning_effort: Some("medium".into()),
                attachments: RegistryAttachments {
                    images: Some(images),
                    audio: Some(false),
                    video: Some(false),
                    documents: Some(true),
                },
                capabilities: vec!["vision".into()],
                status: Some("active".into()),
                aliases: vec![format!("{id}-alias")],
            },
        );
        catalog
    }

    #[test]
    fn resolves_ref_and_merges_provider_overrides() {
        let catalog = catalog_with("gpt-5", 400_000, true);
        let mut provider = RegistryProvider {
            id: "openai".into(),
            label: "OpenAI".into(),
            description: String::new(),
            kind: "openai-responses".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            base_url: None,
            extends: None,
            tool_calling_mode: None,
            aggregator: false,
            defaults: Default::default(),
            request_options: Default::default(),
            models: vec![RegistryModel {
                model_ref: Some("gpt-5".into()),
                api_name: Some("gpt-5-2025".into()),
                name: String::new(),
                task_size: None,
                context_window_tokens: Some(123_456), // provider override wins
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                reasoning_levels: Vec::new(),
                default_reasoning_effort: None,
                supports_attachments: None,
                supports_images: None,
                supports_audio: None,
                supports_video: None,
                supports_documents: None,
                attachments: Default::default(),
                capabilities: Vec::new(),
                pricing: None,
            }],
        };

        resolve_provider_refs(&mut provider, &catalog);
        let model = &provider.models[0];
        assert_eq!(model.name, "gpt-5-2025");
        assert_eq!(model.context_window_tokens, Some(123_456));
        assert_eq!(model.max_output_tokens, Some(32_768));
        assert_eq!(model.supports_thinking, Some(true));
        assert_eq!(
            model.reasoning_levels,
            vec!["low".to_string(), "medium".to_string(), "high".to_string()]
        );
        assert_eq!(model.attachments.images, Some(true));
        assert_eq!(model.supports_images, Some(true));
        assert_eq!(model.supports_documents, Some(true));
    }

    #[test]
    fn resolves_alias_ref_when_id_missing() {
        let catalog = catalog_with("gpt-5", 400_000, false);
        let mut provider = RegistryProvider {
            id: "openai".into(),
            label: "OpenAI".into(),
            description: String::new(),
            kind: "openai-responses".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            base_url: None,
            extends: None,
            tool_calling_mode: None,
            aggregator: false,
            defaults: Default::default(),
            request_options: Default::default(),
            models: vec![RegistryModel {
                model_ref: Some("gpt-5-alias".into()),
                api_name: None,
                name: String::new(),
                ..Default::default()
            }],
        };
        resolve_provider_refs(&mut provider, &catalog);
        assert_eq!(provider.models[0].name, "gpt-5");
        assert_eq!(provider.models[0].context_window_tokens, Some(400_000));
        let aliases = build_alias_index(&catalog);
        assert_eq!(aliases.get("gpt-5-alias").map(String::as_str), Some("gpt-5"));
    }

    #[test]
    fn unresolved_ref_falls_back_to_api_name_or_ref() {
        let catalog = ModelCatalog::new();
        let mut provider = RegistryProvider {
            id: "openai".into(),
            label: "OpenAI".into(),
            description: String::new(),
            kind: "openai-responses".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            base_url: None,
            extends: None,
            tool_calling_mode: None,
            aggregator: false,
            defaults: Default::default(),
            request_options: Default::default(),
            models: vec![RegistryModel {
                model_ref: Some("missing-model".into()),
                api_name: Some("api-missing".into()),
                name: String::new(),
                ..Default::default()
            }],
        };
        resolve_provider_refs(&mut provider, &catalog);
        assert_eq!(provider.models[0].name, "api-missing");
    }
}
