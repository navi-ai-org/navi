//! Embedded provider registry snapshot.
//!
//! The registry snapshot is fetched from `navi-ai-org/navi-registry` at build
//! time by `build.rs` and embedded into the binary. This module parses the
//! embedded JSON into [`RegistryProvider`] values and provides a fallback when
//! the SQLite cache and remote fetch are both unavailable.

use anyhow::{Context, Result};

use super::types::{RegistryManifest, RegistryProvider};

include!(concat!(env!("OUT_DIR"), "/embedded_registry/embedded.rs"));

/// Embedded `bases/*.json` pairs used for `extends` resolution.
pub fn embedded_base_files() -> &'static [(&'static str, &'static str)] {
    BASE_FILES
}

/// Returns the embedded manifest, parsed from the snapshot.
pub fn embedded_manifest() -> Result<RegistryManifest> {
    serde_json::from_str(MANIFEST_JSON).context("failed to parse embedded manifest")
}

/// Returns all embedded canonical models, parsed from the snapshot.
///
/// Keyed by the JSON `id`, not by the embedded file label: the registry names
/// files Windows-safely (`:` → `_`, `/` → `__`, see `model_filename_for_id` in
/// the registry's `validate.py`), so `models/gemma3_12b.json` carries
/// `"id": "gemma3:12b"` and provider refs must resolve against the real id.
/// The label is only a fallback for a file without an `id`.
pub fn embedded_model_catalog() -> Result<super::resolve::ModelCatalog> {
    let mut catalog = std::collections::HashMap::new();
    for (label, json) in MODEL_CATALOG_FILES {
        let model: super::types::CanonicalModel = serde_json::from_str(json)
            .with_context(|| format!("failed to parse embedded canonical model '{label}'"))?;
        let id = if model.id.is_empty() {
            label.to_string()
        } else {
            model.id.clone()
        };
        catalog.insert(id, model);
    }
    Ok(catalog)
}

/// Returns all embedded LLM providers, parsed from the snapshot.
pub fn embedded_providers() -> Result<Vec<RegistryProvider>> {
    let catalog = embedded_model_catalog()?;
    let bases = super::extends::base_map_from_embedded(BASE_FILES, PROVIDER_FILES)?;
    let mut providers = Vec::with_capacity(PROVIDER_FILES.len());
    for (id, json) in PROVIDER_FILES {
        let mut provider = super::extends::parse_provider_json(json, &bases)
            .with_context(|| format!("failed to parse embedded provider '{id}'"))?;
        super::resolve::resolve_provider_refs(&mut provider, &catalog);
        providers.push(provider);
    }
    Ok(providers)
}

/// Returns the embedded provider schema JSON, if present.
pub fn embedded_provider_schema() -> Option<&'static str> {
    Some(include_str!(concat!(
        env!("OUT_DIR"),
        "/embedded_registry/schemas/provider.schema.json"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_manifest_parses() {
        let manifest = embedded_manifest().expect("manifest should parse");
        assert!(!manifest.providers.is_empty());
    }

    #[test]
    fn embedded_manifest_version_is_at_least_2() {
        let manifest = embedded_manifest().expect("manifest should parse");
        assert!(
            manifest.version >= 2,
            "manifest version should be >= 2 (bumped when GLM-5.2 was added), got {}",
            manifest.version
        );
    }

    #[test]
    fn embedded_providers_parse_cleanly() {
        let providers = embedded_providers().expect("providers should parse");
        assert!(!providers.is_empty());

        // Every provider must have at least one model.
        for p in &providers {
            assert!(
                !p.models.is_empty(),
                "embedded provider '{}' has no models",
                p.id
            );
        }
    }

    #[test]
    fn embedded_manifest_matches_provider_files() {
        let manifest = embedded_manifest().expect("manifest");
        let providers = embedded_providers().expect("providers");
        assert_eq!(
            manifest.providers.len(),
            providers.len(),
            "manifest provider count != embedded provider file count"
        );
    }

    // ── canonical catalog keys (regression: `:` ids) ──────────────────────

    #[test]
    fn embedded_model_catalog_is_keyed_by_json_id() {
        // The on-disk name is a lossy, Windows-safe encoding of the model id
        // (`:` → `_`, `/` → `__`), so keying the catalog by the file label made
        // ids like `gemma3:12b` unreachable for every provider ref.
        let catalog = embedded_model_catalog().expect("catalog");
        assert!(!catalog.is_empty());
        for (key, model) in &catalog {
            assert_eq!(
                &model.id, key,
                "catalog key must be the canonical model id, not the file label"
            );
        }
        // Ids whose filename differs from the id (validate.py maps `:` → `_`).
        for id in [
            "nemotron-3-ultra-550b-a55b:free",
            "qwen2.5-coder:32b",
            "gemma3:12b",
        ] {
            assert!(
                catalog.contains_key(id),
                "catalog must expose '{id}' (JSON id), got file-labelled keys only"
            );
        }
    }

    #[test]
    fn embedded_provider_refs_all_resolve() {
        // Every `ref` in the snapshot must hit the catalog by id or alias — this
        // is what used to warn `unresolved model ref` at startup for
        // `nemotron-3-ultra-550b-a55b:free` and the Ollama tag ids.
        let catalog = embedded_model_catalog().expect("catalog");
        let providers = embedded_providers().expect("providers");
        let mut unresolved = Vec::new();
        for provider in &providers {
            for model in &provider.models {
                let Some(reference) = model.model_ref.as_deref() else {
                    continue;
                };
                let by_id = catalog.contains_key(reference);
                let by_alias = catalog
                    .values()
                    .any(|c| c.aliases.iter().any(|a| a == reference));
                if !by_id && !by_alias {
                    unresolved.push(format!("{}:{}", provider.id, reference));
                }
            }
        }
        assert!(
            unresolved.is_empty(),
            "unresolved refs in the embedded snapshot: {unresolved:?}"
        );
    }

    #[test]
    fn colon_id_refs_keep_canonical_metadata() {
        let providers = embedded_providers().expect("providers");
        let gitlawb = providers
            .iter()
            .find(|p| p.id == "gitlawb")
            .expect("gitlawb provider");
        let model = gitlawb
            .models
            .iter()
            .find(|m| m.model_ref.as_deref() == Some("nemotron-3-ultra-550b-a55b:free"))
            .expect("free nemotron ref in gitlawb");
        assert!(
            model.max_output_tokens.is_some() || model.attachments.images.is_some(),
            "canonical metadata must be merged into a `:`-id ref (model={model:?})"
        );
    }

    #[test]
    fn embedded_multimodal_models_are_flagged_by_modality() {
        fn provider<'a>(providers: &'a [RegistryProvider], id: &str) -> &'a RegistryProvider {
            providers
                .iter()
                .find(|provider| provider.id == id)
                .unwrap_or_else(|| panic!("missing provider {id}"))
        }

        fn model<'a>(
            provider: &'a RegistryProvider,
            name: &str,
        ) -> &'a super::super::types::RegistryModel {
            provider
                .models
                .iter()
                .find(|model| model.name == name)
                .unwrap_or_else(|| panic!("missing model {}:{}", provider.id, name))
        }

        fn provider_config<'a>(
            providers: &'a [crate::config::types::ProviderConfig],
            id: &str,
        ) -> &'a crate::config::types::ProviderConfig {
            providers
                .iter()
                .find(|provider| provider.id == id)
                .unwrap_or_else(|| panic!("missing provider config {id}"))
        }

        fn config_model<'a>(
            provider: &'a crate::config::types::ProviderConfig,
            name: &str,
        ) -> &'a crate::config::types::ProviderModelConfig {
            provider
                .models
                .iter()
                .find(|model| model.name == name)
                .unwrap_or_else(|| panic!("missing model config {}:{}", provider.id, name))
        }

        let providers = embedded_providers().expect("providers");

        let gemini = provider(&providers, "google-gemini");
        assert_eq!(gemini.defaults.attachments.images, Some(true));
        assert_eq!(gemini.defaults.attachments.audio, Some(true));
        assert_eq!(gemini.defaults.attachments.video, Some(true));
        assert_eq!(gemini.defaults.attachments.documents, Some(true));
        // Canonical refs now materialize attachments onto the model entry.
        assert_eq!(
            model(gemini, "gemini-2.5-flash").attachments.images,
            Some(true)
        );
        assert_eq!(
            model(gemini, "gemini-2.5-flash").attachments.audio,
            Some(true)
        );

        let anthropic = provider(&providers, "anthropic");
        assert_eq!(anthropic.defaults.attachments.images, Some(true));
        assert_eq!(anthropic.defaults.attachments.audio, Some(false));
        assert_eq!(anthropic.defaults.attachments.video, Some(false));
        assert_eq!(anthropic.defaults.attachments.documents, Some(true));

        let openai = provider(&providers, "openai");
        assert_eq!(openai.defaults.attachments.images, Some(true));
        // Canonical attachments override the provider default: the OpenAI
        // provider defaults documents=false, while the GPT-5.x canonical
        // entries flag native document input.
        assert_eq!(model(openai, "gpt-5.4").attachments.documents, Some(true));

        let configs = providers
            .into_iter()
            .map(super::super::store::registry_provider_to_config)
            .collect::<Vec<_>>();

        let gemini_flash = config_model(
            provider_config(&configs, "google-gemini"),
            "gemini-2.5-flash",
        );
        assert_eq!(gemini_flash.supports_images, Some(true));
        assert_eq!(gemini_flash.supports_audio, Some(true));
        assert_eq!(gemini_flash.supports_video, Some(true));
        assert_eq!(gemini_flash.supports_documents, Some(true));

        let claude = config_model(provider_config(&configs, "anthropic"), "claude-sonnet-4-5");
        assert_eq!(claude.supports_images, Some(true));
        assert_eq!(claude.supports_audio, Some(false));
        assert_eq!(claude.supports_video, Some(false));
        assert_eq!(claude.supports_documents, Some(true));

        let gpt_4o = config_model(provider_config(&configs, "openai"), "gpt-4o");
        assert_eq!(gpt_4o.supports_images, Some(true));
        assert_eq!(gpt_4o.supports_documents, Some(false));
        let gpt_5_4 = config_model(provider_config(&configs, "openai"), "gpt-5.4");
        assert_eq!(gpt_5_4.supports_images, Some(true));
        assert_eq!(gpt_5_4.supports_documents, Some(true));
    }
}
