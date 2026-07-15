//! Inherit model metadata when Ctrl+R / `sync models` discovers new SKUs.
//!
//! Providers like xAI only return model *ids* from `/models`. Without inheritance
//! the sync path used to write bare rows (`supports_images = NULL`, no context
//! window) and config overrides lost vision defaults forever.

use std::collections::HashMap;

use super::types::{RegistryAttachments, RegistryModel, RegistryProviderDefaults};
use crate::config::providers::{
    canonical_provider_id, model_attachment_family_candidates, model_attachment_name_candidates,
};

/// Provider-level `defaults.attachments` from the registry snapshot.
///
/// Kept in sync with `registry-snapshot/providers/*/defaults.attachments`.
pub fn provider_registry_attachment_defaults(provider_id: &str) -> RegistryAttachments {
    match canonical_provider_id(provider_id) {
        "xai" => RegistryAttachments {
            images: Some(true),
            audio: Some(false),
            video: Some(false),
            documents: Some(false),
        },
        "google-gemini" | "gemini" => RegistryAttachments {
            images: Some(true),
            audio: Some(true),
            video: Some(true),
            documents: Some(true),
        },
        "anthropic" => RegistryAttachments {
            images: Some(true),
            audio: Some(false),
            video: Some(false),
            documents: Some(true),
        },
        "openai" => RegistryAttachments {
            images: Some(true),
            audio: Some(false),
            video: Some(false),
            documents: Some(false),
        },
        _ => RegistryAttachments::default(),
    }
}

pub fn provider_registry_defaults(provider_id: &str) -> RegistryProviderDefaults {
    RegistryProviderDefaults {
        attachments: provider_registry_attachment_defaults(provider_id),
    }
}

/// Build a [`RegistryModel`] for a name returned by `/models`, inheriting from:
/// 1. exact (or case-insensitive) cache hit
/// 2. family siblings already in the cache (`grok-4.5` ← `grok-4` / `grok-4.3`)
/// 3. provider attachment defaults (xAI images=true, …)
pub fn enrich_synced_registry_model(
    name: &str,
    existing: &HashMap<String, RegistryModel>,
    provider_id: &str,
) -> RegistryModel {
    let defaults = provider_registry_attachment_defaults(provider_id);
    let mut model = lookup_existing(name, existing).unwrap_or_else(|| bare_model(name));
    model.name = name.to_string();

    if let Some(donor) = family_donor(name, existing) {
        inherit_missing_fields(&mut model, donor);
    }

    apply_attachment_defaults(&mut model, &defaults);
    model
}

/// Fill `None` modality flags on a config-layer model from provider defaults.
pub fn apply_provider_attachment_defaults_to_config_model(
    model: &mut crate::config::types::ProviderModelConfig,
    provider_id: &str,
) {
    let defaults = provider_registry_attachment_defaults(provider_id);
    if model.supports_images.is_none() {
        model.supports_images = defaults.images;
    }
    if model.supports_audio.is_none() {
        model.supports_audio = defaults.audio;
    }
    if model.supports_video.is_none() {
        model.supports_video = defaults.video;
    }
    if model.supports_documents.is_none() {
        model.supports_documents = defaults.documents;
    }
}

fn bare_model(name: &str) -> RegistryModel {
    RegistryModel {
        model_ref: None,
        api_name: None,
        name: name.to_string(),
        task_size: None,
        context_window_tokens: None,
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
        attachments: RegistryAttachments::default(),
        capabilities: Vec::new(),
        pricing: None,
    }
}

fn lookup_existing<'a>(
    name: &str,
    existing: &'a HashMap<String, RegistryModel>,
) -> Option<RegistryModel> {
    if let Some(m) = existing.get(name) {
        return Some(m.clone());
    }
    let lower = name.to_ascii_lowercase();
    existing
        .iter()
        .find(|(k, _)| k.to_ascii_lowercase() == lower)
        .map(|(_, m)| m.clone())
}

fn family_donor<'a>(
    name: &str,
    existing: &'a HashMap<String, RegistryModel>,
) -> Option<&'a RegistryModel> {
    let family = model_attachment_family_candidates(name);
    // Prefer longer stems; among matches prefer richer metadata (context set).
    let mut best: Option<&RegistryModel> = None;
    let mut best_score = -1_i32;

    for (existing_name, model) in existing {
        if names_match_family(existing_name, name, &family) {
            let score = donor_richness(model);
            if score > best_score {
                best_score = score;
                best = Some(model);
            }
        }
    }
    best
}

fn names_match_family(existing_name: &str, target_name: &str, family_stems: &[String]) -> bool {
    let existing_leaf = leaf_name(existing_name);
    let target_leaf = leaf_name(target_name);
    if existing_leaf.eq_ignore_ascii_case(&target_leaf) {
        return false; // same model, not a donor
    }
    for stem in family_stems {
        let stem_l = stem.to_ascii_lowercase();
        let existing_l = existing_leaf.to_ascii_lowercase();
        if existing_l == stem_l
            || existing_l.starts_with(&format!("{stem_l}-"))
            || existing_l.starts_with(&format!("{stem_l}."))
        {
            return true;
        }
        // Also accept when stem matches any attachment name candidate of existing.
        for cand in model_attachment_name_candidates(existing_name) {
            if cand.eq_ignore_ascii_case(stem) {
                return true;
            }
        }
    }
    false
}

fn leaf_name(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).replace('_', "-")
}

fn donor_richness(model: &RegistryModel) -> i32 {
    let mut score = 0;
    if model.context_window_tokens.is_some() {
        score += 4;
    }
    if model.supports_images == Some(true) || model.attachments.images == Some(true) {
        score += 3;
    }
    if model.supports_thinking.is_some() {
        score += 2;
    }
    if model.max_output_tokens.is_some() {
        score += 1;
    }
    if !model.reasoning_levels.is_empty() {
        score += 1;
    }
    if model.pricing.is_some() {
        score += 1;
    }
    score
}

fn inherit_missing_fields(target: &mut RegistryModel, donor: &RegistryModel) {
    if target.task_size.is_none() {
        target.task_size = donor.task_size.clone();
    }
    if target.context_window_tokens.is_none() {
        target.context_window_tokens = donor.context_window_tokens;
    }
    if target.max_output_tokens.is_none() {
        target.max_output_tokens = donor.max_output_tokens;
    }
    if target.recommended_temperature.is_none() {
        target.recommended_temperature = donor.recommended_temperature;
    }
    if target.supports_thinking.is_none() {
        target.supports_thinking = donor.supports_thinking;
    }
    if target.reasoning_levels.is_empty() {
        target.reasoning_levels = donor.reasoning_levels.clone();
    }
    if target.default_reasoning_effort.is_none() {
        target.default_reasoning_effort = donor.default_reasoning_effort.clone();
    }
    if target.supports_images.is_none() && target.attachments.images.is_none() {
        target.supports_images = donor.supports_images.or(donor.attachments.images);
        target.attachments.images = target.supports_images;
    }
    if target.supports_audio.is_none() && target.attachments.audio.is_none() {
        target.supports_audio = donor.supports_audio.or(donor.attachments.audio);
        target.attachments.audio = target.supports_audio;
    }
    if target.supports_video.is_none() && target.attachments.video.is_none() {
        target.supports_video = donor.supports_video.or(donor.attachments.video);
        target.attachments.video = target.supports_video;
    }
    if target.supports_documents.is_none() && target.attachments.documents.is_none() {
        target.supports_documents = donor.supports_documents.or(donor.attachments.documents);
        target.attachments.documents = target.supports_documents;
    }
    if target.pricing.is_none() {
        target.pricing = donor.pricing.clone();
    }
    if target.capabilities.is_empty() {
        target.capabilities = donor.capabilities.clone();
    }
}

fn apply_attachment_defaults(model: &mut RegistryModel, defaults: &RegistryAttachments) {
    if model.supports_images.is_none() && model.attachments.images.is_none() {
        model.supports_images = defaults.images;
        model.attachments.images = defaults.images;
    }
    if model.supports_audio.is_none() && model.attachments.audio.is_none() {
        model.supports_audio = defaults.audio;
        model.attachments.audio = defaults.audio;
    }
    if model.supports_video.is_none() && model.attachments.video.is_none() {
        model.supports_video = defaults.video;
        model.attachments.video = defaults.video;
    }
    if model.supports_documents.is_none() && model.attachments.documents.is_none() {
        model.supports_documents = defaults.documents;
        model.attachments.documents = defaults.documents;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(name: &str, images: Option<bool>, ctx: Option<u64>) -> RegistryModel {
        let mut m = bare_model(name);
        m.supports_images = images;
        m.attachments.images = images;
        m.context_window_tokens = ctx;
        m.supports_thinking = Some(true);
        m
    }

    #[test]
    fn new_grok_sku_inherits_vision_and_context_from_sibling() {
        let mut existing = HashMap::new();
        existing.insert(
            "grok-4".into(),
            model("grok-4", Some(true), Some(1_000_000)),
        );

        let enriched = enrich_synced_registry_model("grok-4.5", &existing, "xai");
        assert_eq!(enriched.supports_images, Some(true));
        assert_eq!(enriched.context_window_tokens, Some(1_000_000));
        assert_eq!(enriched.supports_thinking, Some(true));
    }

    #[test]
    fn new_grok_sku_gets_xai_defaults_without_siblings() {
        let existing = HashMap::new();
        let enriched = enrich_synced_registry_model("grok-4.5", &existing, "xai");
        assert_eq!(
            enriched.supports_images,
            Some(true),
            "xAI defaults.attachments.images must apply on bare sync"
        );
        assert_eq!(enriched.supports_audio, Some(false));
    }

    #[test]
    fn explicit_false_is_not_overwritten_by_defaults() {
        let mut existing = HashMap::new();
        existing.insert("text-only".into(), model("text-only", Some(false), None));

        let enriched = enrich_synced_registry_model("text-only", &existing, "xai");
        assert_eq!(enriched.supports_images, Some(false));
    }

    #[test]
    fn openrouter_has_no_family_wide_image_default() {
        let existing = HashMap::new();
        let enriched = enrich_synced_registry_model("some/random-model", &existing, "openrouter");
        assert_eq!(enriched.supports_images, None);
    }
}
