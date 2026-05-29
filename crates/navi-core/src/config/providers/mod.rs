mod opencode;
mod registry;

use crate::config::types::{
    ModelOption, NaviConfig, ProviderConfig, ProviderKind, ProviderModelConfig,
};

pub use opencode::{
    is_free_model_name, model_can_run_publicly, opencode_zen_model_id, provider_request_model_name,
};

// Re-export for config.rs tests that use these helpers
pub use registry::{model, model_ctx};

/// Returns the full provider catalog: built-in providers merged with any
/// user-configured overrides.
pub fn provider_catalog(config: &NaviConfig) -> Vec<ProviderConfig> {
    let mut providers = registry::built_in_providers();
    merge_provider_configs(&mut providers, config.providers.clone());
    providers
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
pub fn effective_tool_prompt_manifest(config: &NaviConfig) -> bool {
    use crate::config::types::ToolPromptManifest;

    match config.harness.tool_prompt_manifest {
        ToolPromptManifest::Always => return true,
        ToolPromptManifest::Never => return false,
        ToolPromptManifest::Auto => {}
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

impl NaviConfig {
    /// Updates the model list for a provider, merging with existing model metadata
    /// from the built-in catalog.
    pub fn update_provider_models(&mut self, provider_id: &str, model_names: &[String]) {
        let mut existing_models = std::collections::HashMap::new();

        let provider_id = canonical_provider_id(provider_id).to_string();

        if let Some(built_in) = registry::built_in_providers()
            .into_iter()
            .find(|p| canonical_provider_id(&p.id) == provider_id)
        {
            for m in built_in.models {
                existing_models.insert(
                    m.name.clone(),
                    (m.task_size, m.context_window_tokens, m.tool_prompt_manifest),
                );
            }
        }

        if let Some(existing_override) = self
            .providers
            .iter()
            .find(|p| canonical_provider_id(&p.id) == provider_id)
        {
            for m in &existing_override.models {
                existing_models.insert(
                    m.name.clone(),
                    (m.task_size, m.context_window_tokens, m.tool_prompt_manifest),
                );
            }
        }

        let mut new_models = Vec::new();
        for name in model_names {
            if let Some(&(size, ctx, tool_prompt_manifest)) = existing_models.get(name) {
                new_models.push(ProviderModelConfig {
                    name: name.clone(),
                    task_size: size,
                    context_window_tokens: ctx,
                    tool_prompt_manifest,
                });
            } else {
                new_models.push(ProviderModelConfig {
                    name: name.clone(),
                    task_size: registry::determine_task_size(name),
                    context_window_tokens: None,
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
            *existing = override_config;
            existing.id = canonical_provider_id(&existing.id).to_string();
        } else {
            providers.push(override_config);
        }
    }
}
