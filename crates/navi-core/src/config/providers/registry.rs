use crate::config::types::{ModelTaskSize, ProviderRequestOptions};

/// Returns the built-in default [`ProviderRequestOptions`] for a canonical
/// provider id, or `None` when the provider has no known defaults.
///
/// This is the single source of truth for the "out of the box" prompt
/// caching settings. The catalog layer merges these defaults into the resolved
/// [`ProviderConfig`] whenever the user has not explicitly configured the
/// options, so prompt caching stays enabled even when the local registry
/// cache is stale or when a user override replaces the provider wholesale.
pub fn default_request_options_for(provider_id: &str) -> Option<ProviderRequestOptions> {
    match provider_id {
        "openai" | "openai-responses" => Some(ProviderRequestOptions {
            prompt_cache_key: Some("openai".to_string()),
            prompt_cache_retention: Some("24h".to_string()),
            ..Default::default()
        }),
        "anthropic" => Some(ProviderRequestOptions {
            anthropic_cache_control: Some(serde_json::json!({ "type": "ephemeral" })),
            ..Default::default()
        }),
        _ => None,
    }
}

/// Deprecated: task size is no longer inferred from model names. Returns `None`.
/// Kept for backward compatibility with config file overrides.
pub(super) fn determine_task_size(_name: &str) -> Option<ModelTaskSize> {
    None
}
