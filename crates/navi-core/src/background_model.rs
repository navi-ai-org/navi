//! Background model resolver — selects cheap models for background tasks
//! based on capability profiles from the SQLite registry.

use crate::credentials::{CredentialStore, resolve_provider_api_key};
use crate::registry::RegistryStore;
use crate::{ProviderConfig, resolve_provider_config};
use std::sync::{Arc, RwLock};

/// A resolved background model ready for use.
#[derive(Debug, Clone)]
pub struct ResolvedBackgroundModel {
    /// Provider identifier (e.g. "openai").
    pub provider_id: String,
    /// Model name (e.g. "gpt-4.1-nano").
    pub model_name: String,
    /// Resolved provider configuration.
    pub provider_config: ProviderConfig,
}

/// Resolves background models by querying the SQLite registry for models
/// matching a requested profile, then checking credential availability.
pub struct BackgroundModelResolver {
    registry: Option<Arc<RegistryStore>>,
    config: Arc<RwLock<crate::config::NaviConfig>>,
    credential_store: CredentialStore,
}

impl BackgroundModelResolver {
    /// Creates a new resolver.
    pub fn new(
        registry: Option<Arc<RegistryStore>>,
        config: Arc<RwLock<crate::config::NaviConfig>>,
        credential_store: CredentialStore,
    ) -> Self {
        Self {
            registry,
            config,
            credential_store,
        }
    }

    /// Resolves a model for the given task type (e.g. "naming", "compaction").
    ///
    /// Resolution order:
    /// 1. Check user config `background_models.<task>` for explicit override or profile
    /// 2. Query SQLite registry for models matching the profile
    /// 3. Check credential availability for each candidate
    /// 4. Return first match, or fallback to main model
    pub fn resolve(&self, task: &str) -> ResolvedBackgroundModel {
        let config = self.config.read().unwrap_or_else(|e| e.into_inner());
        let bg_config = &config.background_models;

        // 1. Check for explicit provider+model override in config.
        if let Some(entry) = bg_config.resolve(task) {
            if let (Some(provider), Some(model)) = (&entry.provider, &entry.model)
                && let Some(resolved) = self.try_explicit(provider, model, &config)
            {
                return resolved;
            }
            // 2. If profile specified, query registry.
            if let Some(profile) = &entry.profile
                && let Some(resolved) = self.resolve_from_profile(profile)
            {
                return resolved;
            }
        }

        // 3. Map task to default profile and query registry.
        let default_profile = match task {
            "naming" => "naming",
            "repo_search" => "repo_search",
            "compaction" => "long_context_cheap",
            "subagent_research" => "research_synthesis",
            "simple_code_edit" => "cheap_code",
            _ => "cheap_general",
        };
        if let Some(resolved) = self.resolve_from_profile(default_profile) {
            return resolved;
        }

        // 4. Fallback to main model.
        self.main_model_fallback(&config)
    }

    /// Resolves a model from a profile name by querying the registry.
    fn resolve_from_profile(&self, profile_id: &str) -> Option<ResolvedBackgroundModel> {
        let registry = self.registry.as_ref()?;
        let ranked = registry.query_models_by_profile(profile_id).ok()?;

        for candidate in &ranked {
            if self.has_credential(&candidate.provider_id) {
                let config = self.config.read().unwrap_or_else(|e| e.into_inner());
                if let Some(resolved) =
                    self.try_explicit(&candidate.provider_id, &candidate.model_name, &config)
                {
                    return Some(resolved);
                }
            }
        }

        None
    }

    /// Checks if a credential is available for the given provider.
    fn has_credential(&self, provider_id: &str) -> bool {
        let config = self.config.read().unwrap_or_else(|e| e.into_inner());
        let Some(provider_config) = resolve_provider_config(&config, provider_id) else {
            return false;
        };
        resolve_provider_api_key(&self.credential_store, &provider_config, provider_id).is_some()
    }

    /// Tries to build a ResolvedBackgroundModel from explicit provider+model.
    fn try_explicit(
        &self,
        provider_id: &str,
        model_name: &str,
        config: &crate::config::NaviConfig,
    ) -> Option<ResolvedBackgroundModel> {
        let provider_config = resolve_provider_config(config, provider_id)?;
        Some(ResolvedBackgroundModel {
            provider_id: provider_id.to_string(),
            model_name: model_name.to_string(),
            provider_config,
        })
    }

    /// Returns a fallback resolved model using the main configured model.
    fn main_model_fallback(&self, config: &crate::config::NaviConfig) -> ResolvedBackgroundModel {
        let provider_id = config.model.provider.clone();
        let model_name = config.model.name.clone();
        let provider_config =
            resolve_provider_config(config, &provider_id).unwrap_or_else(|| ProviderConfig {
                id: provider_id.clone(),
                ..ProviderConfig::default()
            });
        ResolvedBackgroundModel {
            provider_id,
            model_name,
            provider_config,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NaviConfig;
    use crate::config::types::{BackgroundModelEntry, ModelConfig};

    fn test_resolver(config: NaviConfig) -> BackgroundModelResolver {
        let tempdir = tempfile::tempdir().unwrap();
        let registry = RegistryStore::open_memory().ok().map(Arc::new);
        let config = Arc::new(RwLock::new(config));
        let cred_store = CredentialStore::new(tempdir.path().to_path_buf());
        BackgroundModelResolver::new(registry, config, cred_store)
    }

    #[test]
    fn resolve_falls_back_to_main_model() {
        let config = NaviConfig {
            model: ModelConfig {
                provider: "openai".to_string(),
                name: "gpt-5.5".to_string(),
            },
            ..Default::default()
        };
        let resolver = test_resolver(config);
        let resolved = resolver.resolve("naming");
        assert_eq!(resolved.provider_id, "openai");
        assert_eq!(resolved.model_name, "gpt-5.5");
    }

    #[test]
    fn resolve_explicit_override() {
        let mut config = NaviConfig {
            model: ModelConfig {
                provider: "openai".to_string(),
                name: "gpt-5.5".to_string(),
            },
            ..Default::default()
        };
        config.background_models.naming = Some(BackgroundModelEntry {
            profile: None,
            provider: Some("anthropic".to_string()),
            model: Some("claude-haiku".to_string()),
            fallback: None,
        });
        // Need anthropic provider in config for resolve_provider_config to work.
        config.providers.push(crate::ProviderConfig {
            id: "anthropic".to_string(),
            label: "Anthropic".to_string(),
            kind: crate::ProviderKind::AnthropicMessages,
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
            ..Default::default()
        });

        let resolver = test_resolver(config);
        let resolved = resolver.resolve("naming");
        // Since no API key is set, explicit override won't resolve via credential check.
        // It falls through to the main model fallback.
        // The try_explicit doesn't check credentials, it just checks provider exists.
        assert_eq!(resolved.provider_id, "anthropic");
        assert_eq!(resolved.model_name, "claude-haiku");
    }

    #[test]
    fn task_to_default_profile_mapping() {
        let config = NaviConfig::default();
        let resolver = test_resolver(config);
        // All tasks should fall back to main model since registry is empty.
        for task in &["naming", "repo_search", "compaction", "subagent_research"] {
            let resolved = resolver.resolve(task);
            // Falls back to default config model.
            assert_eq!(resolved.provider_id, "openai");
        }
    }
}
