use anyhow::Result;
use navi_core::{
    CredentialStore, LoadedConfig, ModelProvider, SecurityPolicy, ToolExecutor,
    model_can_run_publicly, resolve_provider_api_key, resolve_provider_config,
};
use navi_plugin_host::load_configured_plugins;
use navi_providers::OpenAiProvider;
use std::path::PathBuf;
use std::sync::Arc;

use crate::types::{NaviMissingCredentialError, NaviRuntimeTooling};

pub(crate) fn build_local_tooling(
    loaded_config: &LoadedConfig,
    project_dir: PathBuf,
) -> Result<NaviRuntimeTooling> {
    let security_policy = SecurityPolicy::new(
        project_dir,
        loaded_config.data_dir.clone(),
        loaded_config.config.security.clone(),
    )?;
    let mut tool_executor = ToolExecutor::new(security_policy.clone());
    let plugin_report = load_configured_plugins(
        &loaded_config.config.plugins,
        &security_policy,
        &mut tool_executor,
    );

    Ok(NaviRuntimeTooling {
        tool_executor: Arc::new(tool_executor),
        warnings: plugin_report.warnings,
        _plugins: plugin_report.loaded_plugins,
    })
}

pub(crate) fn build_model_provider(loaded_config: &LoadedConfig) -> Result<Arc<dyn ModelProvider>> {
    let provider_config =
        resolve_provider_config(&loaded_config.config, &loaded_config.config.model.provider)
            .ok_or_else(|| {
                anyhow::anyhow!("unknown provider {}", loaded_config.config.model.provider)
            })?;
    let credential_store = CredentialStore::new(loaded_config.data_dir.clone());
    let api_key = resolve_provider_api_key(
        &credential_store,
        &provider_config,
        &loaded_config.config.model.provider,
    )
    .or_else(|| {
        (model_can_run_publicly(
            &loaded_config.config.model.provider,
            &loaded_config.config.model.name,
        ) || model_can_run_publicly(&provider_config.id, &loaded_config.config.model.name))
        .then(|| "public".to_string())
    })
    .ok_or_else(|| NaviMissingCredentialError {
        provider_id: provider_config.id.clone(),
        env_var: provider_config.api_key_env.clone(),
        credential_store_path: credential_store.path().to_path_buf(),
    })?;

    Ok(Arc::new(OpenAiProvider::from_provider_config_with_key(
        &provider_config,
        api_key,
    )?))
}

pub(crate) async fn list_models_for_provider(
    provider_config: &navi_core::ProviderConfig,
    api_key: String,
) -> Result<Vec<String>> {
    let provider = model_provider_for_config(provider_config, api_key)?;
    provider.list_models().await
}

pub(crate) fn model_provider_for_config(
    provider_config: &navi_core::ProviderConfig,
    api_key: String,
) -> Result<Arc<dyn ModelProvider>> {
    Ok(Arc::new(OpenAiProvider::from_provider_config_with_key(
        provider_config,
        api_key,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use navi_core::config::ModelConfig;
    use navi_core::{NaviConfig, ProviderConfig, ProviderKind};

    #[test]
    fn build_local_tooling_succeeds_with_default_config() {
        let tempdir = tempfile::tempdir().unwrap();
        let loaded_config = LoadedConfig {
            config: NaviConfig::default(),
            global_config_path: None,
            project_config_path: None,
            data_dir: tempdir.path().to_path_buf(),
        };

        let result = build_local_tooling(&loaded_config, tempdir.path().to_path_buf());
        assert!(
            result.is_ok(),
            "build_local_tooling should succeed with default config"
        );
    }

    #[test]
    fn build_local_tooling_returns_empty_warnings_without_plugins() {
        let tempdir = tempfile::tempdir().unwrap();
        let loaded_config = LoadedConfig {
            config: NaviConfig::default(),
            global_config_path: None,
            project_config_path: None,
            data_dir: tempdir.path().to_path_buf(),
        };

        let tooling = build_local_tooling(&loaded_config, tempdir.path().to_path_buf()).unwrap();

        // Verify the executor can list definitions without panicking.
        let _definitions = tooling.tool_executor.definitions();
        // No plugins configured, so warnings should be empty.
        assert!(
            tooling.warnings.is_empty(),
            "no warnings expected with default config"
        );
    }

    #[test]
    fn build_model_provider_returns_structured_error_for_missing_credentials() {
        let tempdir = tempfile::tempdir().unwrap();
        let loaded_config = LoadedConfig {
            config: NaviConfig {
                model: ModelConfig {
                    provider: "test-provider".to_string(),
                    name: "test-model".to_string(),
                },
                providers: vec![ProviderConfig {
                    id: "test-provider".to_string(),
                    label: "Test".to_string(),
                    kind: ProviderKind::OpenAiResponses,
                    api_key_env: "NAVI_SDK_TOOLING_TEST_MISSING_KEY_12345".to_string(),
                    base_url: Some("https://example.test/v1".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            global_config_path: None,
            project_config_path: None,
            data_dir: tempdir.path().to_path_buf(),
        };

        let error = match build_model_provider(&loaded_config) {
            Ok(_) => panic!("expected missing credential error"),
            Err(e) => e,
        };

        let missing = error
            .downcast_ref::<NaviMissingCredentialError>()
            .expect("error should downcast to NaviMissingCredentialError");
        assert_eq!(missing.provider_id, "test-provider");
        assert_eq!(missing.env_var, "NAVI_SDK_TOOLING_TEST_MISSING_KEY_12345");
        assert_eq!(
            missing.credential_store_path,
            tempdir.path().join("credentials.toml")
        );
    }

    #[test]
    fn build_model_provider_returns_error_for_unknown_provider() {
        let tempdir = tempfile::tempdir().unwrap();
        let loaded_config = LoadedConfig {
            config: NaviConfig {
                model: ModelConfig {
                    provider: "nonexistent-provider".to_string(),
                    name: "some-model".to_string(),
                },
                ..Default::default()
            },
            global_config_path: None,
            project_config_path: None,
            data_dir: tempdir.path().to_path_buf(),
        };

        let error = match build_model_provider(&loaded_config) {
            Ok(_) => panic!("expected error for unknown provider"),
            Err(e) => e,
        };

        assert!(
            error.to_string().contains("unknown provider"),
            "error should mention unknown provider, got: {}",
            error
        );
    }
}
