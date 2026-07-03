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
}
