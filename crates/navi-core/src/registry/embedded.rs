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

        let providers = embedded_providers().expect("providers");

        let gemini = model(provider(&providers, "google-gemini"), "gemini-2.5-flash");
        assert_eq!(gemini.supports_images, Some(true));
        assert_eq!(gemini.supports_audio, Some(true));
        assert_eq!(gemini.supports_video, Some(true));
        assert_eq!(gemini.supports_documents, Some(true));

        let claude = model(provider(&providers, "anthropic"), "claude-sonnet-4");
        assert_eq!(claude.supports_images, Some(true));
        assert_eq!(claude.supports_documents, Some(true));
        assert_eq!(claude.supports_audio, None);
        assert_eq!(claude.supports_video, None);

        let gpt_4o = model(provider(&providers, "openai"), "gpt-4o");
        assert_eq!(gpt_4o.supports_images, Some(true));
        assert_eq!(gpt_4o.supports_audio, None);
        assert_eq!(gpt_4o.supports_video, None);

        let grok_vision = model(provider(&providers, "xai"), "grok-2-vision-1212");
        assert_eq!(grok_vision.supports_images, Some(true));
    }
}
