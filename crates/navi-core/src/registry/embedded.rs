//! Embedded provider registry snapshot.
//!
//! The registry snapshot is vendored at `registry-snapshot/` and embedded into
//! the binary at build time by `build.rs`. This module parses the embedded JSON
//! into [`RegistryProvider`] values and provides a fallback when the SQLite
//! cache and remote fetch are both unavailable.

use anyhow::{Context, Result};

use super::types::{RegistryManifest, RegistryProvider};

include!(concat!(env!("OUT_DIR"), "/embedded_registry/embedded.rs"));

/// Returns the embedded manifest, parsed from the snapshot.
pub fn embedded_manifest() -> Result<RegistryManifest> {
    serde_json::from_str(MANIFEST_JSON).context("failed to parse embedded manifest")
}

/// Returns all embedded providers, parsed from the snapshot.
pub fn embedded_providers() -> Result<Vec<RegistryProvider>> {
    let mut providers = Vec::with_capacity(PROVIDER_FILES.len());
    for (id, json) in PROVIDER_FILES {
        let provider: RegistryProvider = serde_json::from_str(json)
            .with_context(|| format!("failed to parse embedded provider '{id}'"))?;
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
        assert!(model(gemini, "gemini-2.5-flash").attachments.is_empty());

        let anthropic = provider(&providers, "anthropic");
        assert_eq!(anthropic.defaults.attachments.images, Some(true));
        assert_eq!(anthropic.defaults.attachments.audio, Some(false));
        assert_eq!(anthropic.defaults.attachments.video, Some(false));
        assert_eq!(anthropic.defaults.attachments.documents, Some(true));

        let openai = provider(&providers, "openai");
        assert_eq!(openai.defaults.attachments.images, Some(true));
        assert_eq!(model(openai, "o3-mini").attachments.images, Some(false));

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

        let claude = config_model(provider_config(&configs, "anthropic"), "claude-sonnet-4");
        assert_eq!(claude.supports_images, Some(true));
        assert_eq!(claude.supports_audio, Some(false));
        assert_eq!(claude.supports_video, Some(false));
        assert_eq!(claude.supports_documents, Some(true));

        let gpt_4o = config_model(provider_config(&configs, "openai"), "gpt-4o");
        assert_eq!(gpt_4o.supports_images, Some(true));
        let o3_mini = config_model(provider_config(&configs, "openai"), "o3-mini");
        assert_eq!(o3_mini.supports_images, Some(false));
    }
}
