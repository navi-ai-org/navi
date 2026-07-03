mod opencode;
mod registry;

use std::cell::RefCell;
use std::sync::Arc;

use crate::config::types::{
    ModelOption, ModelTaskSize, NaviConfig, ProviderConfig, ProviderKind, ProviderModelConfig,
    ToolCallingMode,
};
use crate::registry::RegistryStore;

pub use opencode::{is_free_model_name, model_can_run_publicly, provider_request_model_name};
pub use registry::default_request_options_for;

// ── Thread-local registry store for zero-API-change catalog integration ──

thread_local! {
    static REGISTRY_STORE: RefCell<Option<Arc<RegistryStore>>> = const { RefCell::new(None) };
}

/// Sets the thread-local registry store used by [`provider_catalog`].
/// Typically called once during engine initialization.
pub fn set_registry_store(store: Arc<RegistryStore>) {
    REGISTRY_STORE.with(|cell| {
        *cell.borrow_mut() = Some(store);
    });
}

/// Returns the full provider catalog: SQLite registry cache merged with any
/// user-configured overrides. Falls back to built-in providers if the
/// registry cache is empty or unavailable.
pub fn provider_catalog(config: &NaviConfig) -> Vec<ProviderConfig> {
    let mut providers = base_provider_catalog();
    merge_provider_configs(&mut providers, config.providers.clone());
    apply_default_request_options(&mut providers);
    providers
}

fn base_provider_catalog() -> Vec<ProviderConfig> {
    REGISTRY_STORE.with(|cell| {
        cell.borrow().as_ref().map_or_else(
            || {
                tracing::debug!("registry store not set, falling back to embedded snapshot");
                load_embedded_or_minimal_fallback()
            },
            |store| {
                match crate::registry::load_registry(store) {
                    loaded if !loaded.providers.is_empty() => loaded.providers,
                    _ => {
                        tracing::debug!("loaded registry is empty, falling back to embedded snapshot");
                        load_embedded_or_minimal_fallback()
                    }
                }
            },
        )
    })
}

fn load_embedded_or_minimal_fallback() -> Vec<ProviderConfig> {
    match crate::registry::load_embedded_registry() {
        Some(loaded) if !loaded.providers.is_empty() => loaded.providers,
        Some(_) => minimal_fallback_providers(),
        None => {
            tracing::error!("failed to parse embedded registry snapshot, using minimal fallback");
            minimal_fallback_providers()
        }
    }
}

/// Minimal hardcoded fallback used only if the embedded snapshot itself fails
/// to parse (should never happen in practice).
fn minimal_fallback_providers() -> Vec<ProviderConfig> {
    vec![ProviderConfig {
        id: "openai".to_string(),
        label: "OpenAI".to_string(),
        description: "OpenAI API key required".to_string(),
        kind: ProviderKind::OpenAiResponses,
        api_key_env: "OPENAI_API_KEY".to_string(),
        base_url: Some("https://api.openai.com/v1".to_string()),
        models: vec![ProviderModelConfig {
            name: "gpt-5.1".to_string(),
            task_size: ModelTaskSize::Large,
            context_window_tokens: Some(1_000_000),
            max_output_tokens: None,
            recommended_temperature: None,
            supports_thinking: None,
            tool_prompt_manifest: None,
        }],
        request_options: default_request_options_for("openai"),
        ..Default::default()
    }]
}

/// Maps provider aliases to their canonical form (e.g. `"opencode-zen"` to `"opencode"`).
pub fn canonical_provider_id(id: &str) -> &str {
    match id {
        "opencode-zen" => "opencode",
        other => other,
    }
}

/// Resolves a provider config by id from the merged catalog, following aliases.
pub fn resolve_provider_config(config: &NaviConfig, id: &str) -> Option<ProviderConfig> {
    let canonical_id = canonical_provider_id(id);
    provider_catalog(config)
        .into_iter()
        .find(|provider| canonical_provider_id(&provider.id) == canonical_id)
}

/// Returns all available model options across all providers in the catalog.
pub fn available_model_options(config: &NaviConfig) -> Vec<ModelOption> {
    provider_catalog(config)
        .into_iter()
        .flat_map(|provider| {
            let desc = provider.description.clone();
            provider
                .models
                .clone()
                .into_iter()
                .map(move |model| ModelOption {
                    name: model.name,
                    provider_id: provider.id.clone(),
                    provider_label: provider.label.clone(),
                    provider_description: desc.clone(),
                    task_size: model.task_size,
                    context_window_tokens: model.context_window_tokens,
                })
        })
        .collect()
}

/// Returns the context window size for the selected model, or a default if unknown.
pub fn effective_context_window(config: &NaviConfig) -> u64 {
    let selected_provider = &config.model.provider;
    let selected_model = &config.model.name;
    available_model_options(config)
        .into_iter()
        .find(|m| m.provider_id == *selected_provider && m.name == *selected_model)
        .and_then(|m| m.context_window_tokens)
        .unwrap_or(crate::config::defaults::DEFAULT_CONTEXT_WINDOW)
}

/// Whether the tool prompt manifest should be included for the selected model,
/// based on harness config and provider/model settings.
pub(crate) fn effective_tool_prompt_manifest(config: &NaviConfig) -> bool {
    use crate::config::types::ToolPromptManifest;

    match config.harness.tool_prompt_manifest {
        ToolPromptManifest::Always => return true,
        ToolPromptManifest::Never => return false,
        ToolPromptManifest::Auto => {}
    }

    match effective_tool_calling_mode(config) {
        ToolCallingMode::TextExtracted | ToolCallingMode::ManifestOnly => return true,
        ToolCallingMode::Disabled => return false,
        ToolCallingMode::Native => {}
    }

    let selected_provider = &config.model.provider;
    let selected_model = &config.model.name;
    provider_catalog(config)
        .into_iter()
        .find(|provider| {
            canonical_provider_id(&provider.id) == canonical_provider_id(selected_provider)
        })
        .and_then(|provider| {
            provider
                .models
                .iter()
                .find(|model| model.name == *selected_model)
                .and_then(|model| model.tool_prompt_manifest)
                .or(provider.tool_prompt_manifest)
        })
        .unwrap_or(false)
}

/// Returns the selected provider's resolved tool calling compatibility mode.
pub fn effective_tool_calling_mode(config: &NaviConfig) -> ToolCallingMode {
    let selected_provider = &config.model.provider;
    provider_catalog(config)
        .into_iter()
        .find(|provider| {
            canonical_provider_id(&provider.id) == canonical_provider_id(selected_provider)
        })
        .and_then(|provider| provider.tool_calling_mode)
        .unwrap_or(ToolCallingMode::Native)
}

impl NaviConfig {
    /// Updates the model list for a provider, merging with existing model metadata
    /// from the registry or built-in catalog.
    pub fn update_provider_models(&mut self, provider_id: &str, model_names: &[String]) {
        let mut existing_models = std::collections::HashMap::new();

        let provider_id = canonical_provider_id(provider_id).to_string();

        // Start with user overrides as a fallback for custom models.
        if let Some(existing_override) = self
            .providers
            .iter()
            .find(|p| canonical_provider_id(&p.id) == provider_id)
        {
            for m in &existing_override.models {
                existing_models.insert(m.name.clone(), m.clone());
            }
        }

        // Registry metadata is authoritative for models it knows about. This
        // lets `sync models` refresh stale context windows saved in config.
        if let Some(registry_provider) = base_provider_catalog()
            .into_iter()
            .find(|p| canonical_provider_id(&p.id) == provider_id)
        {
            for m in registry_provider.models {
                existing_models.insert(m.name.clone(), m);
            }
        }

        let mut new_models = Vec::new();
        for name in model_names {
            if let Some(model) = existing_models.get(name) {
                let mut model = model.clone();
                model.name = name.clone();
                new_models.push(model);
            } else {
                new_models.push(ProviderModelConfig {
                    name: name.clone(),
                    task_size: registry::determine_task_size(name),
                    context_window_tokens: None,
                    max_output_tokens: None,
                    recommended_temperature: None,
                    supports_thinking: None,
                    tool_prompt_manifest: None,
                });
            }
        }

        if let Some(p) = self
            .providers
            .iter_mut()
            .find(|p| canonical_provider_id(&p.id) == provider_id)
        {
            p.id = provider_id.clone();
            p.models = new_models;
        } else {
            if let Some(mut resolved) = resolve_provider_config(self, &provider_id) {
                resolved.models = new_models;
                self.providers.push(resolved);
            } else {
                self.providers.push(ProviderConfig {
                    id: provider_id.to_string(),
                    label: provider_id.to_string(),
                    description: "Synced dynamically".to_string(),
                    kind: ProviderKind::OpenAiChatCompletions,
                    api_key_env: format!(
                        "{}_API_KEY",
                        provider_id.to_uppercase().replace('-', "_")
                    ),
                    base_url: None,
                    models: new_models,
                    ..Default::default()
                });
            }
        }
    }
}

pub(crate) fn merge_provider_configs(
    providers: &mut Vec<ProviderConfig>,
    overrides: Vec<ProviderConfig>,
) {
    for override_config in overrides {
        if let Some(existing) = providers.iter_mut().find(|provider| {
            canonical_provider_id(&provider.id) == canonical_provider_id(&override_config.id)
        }) {
            // Merge models by name: preserve registry metadata (context_window_tokens,
            // max_output_tokens, recommended_temperature, supports_thinking,
            // tool_prompt_manifest) when the user override doesn't specify them.
            let existing_models: std::collections::HashMap<String, ProviderModelConfig> = existing
                .models
                .drain(..)
                .map(|m| (m.name.clone(), m))
                .collect();

            let mut merged_models = Vec::new();
            for override_model in override_config.models {
                if let Some(registry_model) = existing_models.get(&override_model.name) {
                    merged_models.push(ProviderModelConfig {
                        name: override_model.name,
                        task_size: override_model.task_size,
                        context_window_tokens: override_model
                            .context_window_tokens
                            .or(registry_model.context_window_tokens),
                        max_output_tokens: override_model
                            .max_output_tokens
                            .or(registry_model.max_output_tokens),
                        recommended_temperature: override_model
                            .recommended_temperature
                            .or(registry_model.recommended_temperature),
                        supports_thinking: override_model
                            .supports_thinking
                            .or(registry_model.supports_thinking),
                        tool_prompt_manifest: override_model
                            .tool_prompt_manifest
                            .or(registry_model.tool_prompt_manifest),
                    });
                } else {
                    merged_models.push(override_model);
                }
            }

            // Override provider-level fields, keep merged models.
            existing.id = canonical_provider_id(&existing.id).to_string();
            existing.label = override_config.label;
            existing.description = override_config.description;
            existing.kind = override_config.kind;
            existing.api_key_env = override_config.api_key_env;
            if override_config.base_url.is_some() {
                existing.base_url = override_config.base_url;
            }
            existing.models = merged_models;
            if override_config.request_options.is_some() {
                existing.request_options = override_config.request_options;
            }
            if override_config.request_timeout_ms.is_some() {
                existing.request_timeout_ms = override_config.request_timeout_ms;
            }
            if override_config.request_max_retries.is_some() {
                existing.request_max_retries = override_config.request_max_retries;
            }
            if override_config.stream_idle_timeout_ms.is_some() {
                existing.stream_idle_timeout_ms = override_config.stream_idle_timeout_ms;
            }
            if override_config.stream_max_retries.is_some() {
                existing.stream_max_retries = override_config.stream_max_retries;
            }
            if override_config.websocket_connect_timeout_ms.is_some() {
                existing.websocket_connect_timeout_ms =
                    override_config.websocket_connect_timeout_ms;
            }
            if override_config.retry_429.is_some() {
                existing.retry_429 = override_config.retry_429;
            }
            if override_config.tool_prompt_manifest.is_some() {
                existing.tool_prompt_manifest = override_config.tool_prompt_manifest;
            }
            if override_config.tool_calling_mode.is_some() {
                existing.tool_calling_mode = override_config.tool_calling_mode;
            }
        } else {
            providers.push(override_config);
        }
    }
}

/// Fills in the canonical default [`ProviderRequestOptions`] for any provider
/// whose `request_options` field is `None`. This guarantees that prompt
/// caching stays enabled for known providers (OpenAI, Anthropic) even when:
///   * the local registry cache is stale and ships no `request_options`
///   * a user override in `config.toml` replaces the provider wholesale
///     without setting `request_options`
///
/// Providers that explicitly carry `Some(opts)` keep the user's configuration
/// verbatim — including the empty `ProviderRequestOptions` value that opts
/// out of prompt caching.
fn apply_default_request_options(providers: &mut [ProviderConfig]) {
    for provider in providers {
        let id = canonical_provider_id(&provider.id);
        if provider.request_options.is_none()
            && let Some(defaults) = default_request_options_for(id)
        {
            provider.request_options = Some(defaults);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{effective_tool_calling_mode, effective_tool_prompt_manifest};
    use crate::config::types::{
        ModelTaskSize, NaviConfig, ProviderConfig, ProviderModelConfig, ToolCallingMode,
    };

    #[test]
    fn update_provider_models_prefers_registry_metadata_over_stale_override() {
        let mut config = NaviConfig::default();
        config.providers.push(ProviderConfig {
            id: "commandcode".to_string(),
            models: vec![ProviderModelConfig {
                name: "claude-sonnet-4-6".to_string(),
                task_size: ModelTaskSize::Large,
                context_window_tokens: Some(128_000),
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                tool_prompt_manifest: None,
            }],
            ..Default::default()
        });

        config.update_provider_models("commandcode", &["claude-sonnet-4-6".to_string()]);

        let provider = config
            .providers
            .iter()
            .find(|provider| provider.id == "commandcode")
            .expect("commandcode override");
        assert_eq!(provider.models[0].context_window_tokens, Some(1_000_000));
    }

    #[test]
    fn commandcode_uses_native_tool_mode() {
        let mut config = NaviConfig::default();
        config.model.provider = "commandcode".to_string();

        assert_eq!(
            effective_tool_calling_mode(&config),
            ToolCallingMode::Native
        );
        assert!(!effective_tool_prompt_manifest(&config));
    }
}
