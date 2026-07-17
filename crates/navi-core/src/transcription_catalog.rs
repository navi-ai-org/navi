//! Catalog of remote transcription / dictation providers.
//!
//! Sources (in order), mirroring the LLM provider catalog:
//! 1. Process-global SQLite registry cache (seeded from embedded + remote sync)
//! 2. Embedded registry snapshot (`registry-snapshot/transcription-providers/`)

use crate::config::providers::registry_store_for_catalog;
use crate::registry::{RegistryTranscriptionProvider, embedded_transcription_providers};

/// Returns all known remote transcription providers.
///
/// Prefers the SQLite cache when the engine has set a registry store; falls
/// back to the binary-embedded snapshot so CLI/offline paths still work.
pub fn transcription_provider_catalog() -> Vec<RegistryTranscriptionProvider> {
    if let Some(store) = registry_store_for_catalog() {
        // Ensure legacy DBs get seeded without waiting for a remote sync.
        let _ = store.seed_transcription_from_embedded_if_empty();
        match store.load_transcription_providers() {
            Ok(providers) if !providers.is_empty() => return providers,
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "failed to load transcription providers from cache"
                );
            }
        }
    }

    embedded_transcription_providers().unwrap_or_else(|err| {
        tracing::warn!(error = %err, "failed to load embedded transcription providers");
        Vec::new()
    })
}

/// Lookup a transcription provider by id (case-sensitive registry id).
pub fn find_transcription_provider(id: &str) -> Option<RegistryTranscriptionProvider> {
    let id = id.trim();
    if id.is_empty() || id.eq_ignore_ascii_case("local") {
        return None;
    }
    transcription_provider_catalog()
        .into_iter()
        .find(|p| p.id == id || p.id.eq_ignore_ascii_case(id))
}

/// Resolve model name: explicit config, else provider default, else first model.
pub fn resolve_transcription_model(
    provider: &RegistryTranscriptionProvider,
    configured_model: &str,
) -> String {
    let configured = configured_model.trim();
    if !configured.is_empty() {
        // Accept exact match or case-insensitive.
        if provider.models.iter().any(|m| m.name == configured) {
            return configured.to_string();
        }
        if let Some(m) = provider
            .models
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(configured))
        {
            return m.name.clone();
        }
        // Allow unknown model names (providers may add models before registry refresh).
        return configured.to_string();
    }
    provider
        .resolved_default_model()
        .unwrap_or("whisper-1")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_contains_openai_groq() {
        let cat = transcription_provider_catalog();
        let ids: Vec<_> = cat.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"openai"));
        assert!(ids.contains(&"groq"));
    }

    #[test]
    fn find_and_default_model() {
        let p = find_transcription_provider("openai").expect("openai");
        assert_eq!(resolve_transcription_model(&p, ""), "whisper-1");
        assert_eq!(
            resolve_transcription_model(&p, "gpt-4o-mini-transcribe"),
            "gpt-4o-mini-transcribe"
        );
        let g = find_transcription_provider("groq").expect("groq");
        assert_eq!(
            resolve_transcription_model(&g, ""),
            "whisper-large-v3-turbo"
        );
    }

    #[test]
    fn local_is_not_a_remote_provider() {
        assert!(find_transcription_provider("local").is_none());
        assert!(find_transcription_provider("").is_none());
    }
}
