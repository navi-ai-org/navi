use anyhow::Result;
use navi_core::{
    CredentialStore, LoadedConfig, ModelProvider, ProviderConfig, ProviderKind, RuntimeComponents,
    SecurityPolicy, ToolExecutor, model_can_run_publicly, resolve_provider_api_key,
    resolve_provider_api_key_for_project, resolve_provider_config,
};
#[cfg(feature = "wasm-plugins")]
use navi_plugin_manifest::{SecurityDefaults, aggregate_lockfile_path, installed_plugins_dir};
#[cfg(feature = "wasm-plugins")]
use navi_plugin_orchestrator::PluginOrchestrator;
use navi_providers::OpenAiProvider;
#[cfg(feature = "wasm-plugins")]
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::types::{NaviMissingCredentialError, NaviRuntimeTooling};

pub(crate) fn build_local_tooling(
    loaded_config: &LoadedConfig,
    project_dir: PathBuf,
    runtime_components: &RuntimeComponents,
) -> Result<NaviRuntimeTooling> {
    let security_policy = SecurityPolicy::new(
        project_dir.clone(),
        loaded_config.data_dir.clone(),
        loaded_config.config.effective_security_config(),
    )?;
    #[allow(unused_mut)] // mut when feature `wasm-plugins` loads into the executor
    let mut tool_executor =
        ToolExecutor::with_security_policy(security_policy, runtime_components.security.clone());

    let mut warnings = Vec::new();

    // Native .so/.dylib plugins (libloading) are retired. WASM is the only runtime.
    if let Some(warning) = native_plugins_deprecated_warning(&loaded_config.config.plugins) {
        tracing::warn!(warning = %warning, "native plugins ignored");
        warnings.push(warning);
    }

    // Load WASM plugins from the data-dir store and any configured scan roots.
    #[cfg(feature = "wasm-plugins")]
    {
        let security_defaults = SecurityDefaults::default();
        for plugin_dir in
            wasm_plugin_scan_roots(&loaded_config.data_dir, &loaded_config.config.wasm_plugins)
        {
            load_wasm_plugins_from_root(
                &plugin_dir,
                &project_dir,
                &security_defaults,
                &mut tool_executor,
                &mut warnings,
            );
        }
    }

    #[cfg(not(feature = "wasm-plugins"))]
    if loaded_config
        .config
        .wasm_plugins
        .iter()
        .any(|plugin| plugin.enabled)
    {
        warnings.push(wasm_plugins_disabled_warning());
    }

    Ok(NaviRuntimeTooling {
        tool_executor: Arc::new(tool_executor),
        warnings,
        agent_policies: Vec::new(),
        tui_components: Vec::new(),
        tui_panels: Vec::new(),
    })
}

/// Warn when legacy `[[plugins]]` native library paths are present in config.
fn native_plugins_deprecated_warning(plugins: &[navi_core::PluginConfig]) -> Option<String> {
    let enabled: Vec<String> = plugins
        .iter()
        .filter(|p| p.enabled)
        .map(|p| p.path.display().to_string())
        .collect();
    if enabled.is_empty() {
        return None;
    }
    Some(format!(
        "native plugins are no longer loaded (WASM-only); ignoring [[plugins]] paths: {}",
        enabled.join(", ")
    ))
}

/// Directories to scan for installed WASM plugin subfolders.

#[cfg(feature = "wasm-plugins")]
fn wasm_plugin_scan_roots(
    data_dir: &Path,
    configured: &[navi_core::WasmPluginConfig],
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();

    let installed = installed_plugins_dir(data_dir);
    if seen.insert(installed.clone()) {
        roots.push(installed);
    }

    for wasm_config in configured {
        if !wasm_config.enabled {
            continue;
        }
        if seen.insert(wasm_config.path.clone()) {
            roots.push(wasm_config.path.clone());
        }
    }

    roots
}

/// Reload WASM plugins into an existing executor (unregisters prior `plugin__*` tools first).
#[cfg(feature = "wasm-plugins")]
pub fn reload_wasm_plugins_on_executor(
    executor: &mut ToolExecutor,
    data_dir: &Path,
    project_dir: &Path,
    wasm_plugins: &[navi_core::WasmPluginConfig],
) -> Vec<String> {
    executor.unregister_plugin_tools();
    let mut warnings = Vec::new();
    let security_defaults = SecurityDefaults::default();
    for plugin_dir in wasm_plugin_scan_roots(data_dir, wasm_plugins) {
        load_wasm_plugins_from_root(
            &plugin_dir,
            project_dir,
            &security_defaults,
            executor,
            &mut warnings,
        );
    }
    warnings
}

/// Reload WASM plugins into an existing executor.
#[cfg(not(feature = "wasm-plugins"))]
pub fn reload_wasm_plugins_on_executor(
    executor: &mut ToolExecutor,
    _data_dir: &Path,
    _project_dir: &Path,
    _wasm_plugins: &[navi_core::WasmPluginConfig],
) -> Vec<String> {
    executor.unregister_plugin_tools();
    vec![wasm_plugins_disabled_warning()]
}

#[cfg(feature = "wasm-plugins")]
fn load_wasm_plugins_from_root(
    plugin_dir: &Path,
    project_dir: &Path,
    security_defaults: &SecurityDefaults,
    tool_executor: &mut ToolExecutor,
    warnings: &mut Vec<String>,
) {
    let lockfile_path = aggregate_lockfile_path(plugin_dir);

    let mut orchestrator = PluginOrchestrator::new(
        project_dir.to_path_buf(),
        plugin_dir.to_path_buf(),
        lockfile_path,
        security_defaults.clone(),
    );

    match orchestrator.load_plugins(tool_executor) {
        Ok(report) => {
            for warning in &report.warnings {
                tracing::warn!(
                    path = %plugin_dir.display(),
                    warning = %warning,
                    "WASM plugin warning"
                );
            }
            for loaded in &report.loaded {
                tracing::info!(
                    plugin = %loaded.plugin_id,
                    tools = loaded.tool_count,
                    risk = %loaded.risk_level,
                    "loaded WASM plugin"
                );
            }
            warnings.extend(report.warnings);
        }
        Err(err) => {
            warnings.push(format!(
                "failed to load WASM plugins from {}: {:#}",
                plugin_dir.display(),
                err
            ));
        }
    }
}

#[cfg(not(feature = "wasm-plugins"))]
fn wasm_plugins_disabled_warning() -> String {
    "WASM plugin runtime is disabled in this build; rebuild `navi-sdk` with feature `wasm-plugins`"
        .to_string()
}

/// Builds a `ModelProvider` for the given loaded configuration.
///
/// This is the standard way to construct a provider from config. It resolves
/// the provider config, checks credentials, and returns a boxed provider.
pub fn build_provider_for_config(loaded_config: &LoadedConfig) -> Result<Arc<dyn ModelProvider>> {
    build_provider_for_config_inner(loaded_config, None)
}

pub fn build_provider_for_project_config(
    loaded_config: &LoadedConfig,
    project_dir: &Path,
) -> Result<Arc<dyn ModelProvider>> {
    build_provider_for_config_inner(loaded_config, Some(project_dir))
}

fn build_provider_for_config_inner(
    loaded_config: &LoadedConfig,
    project_dir: Option<&Path>,
) -> Result<Arc<dyn ModelProvider>> {
    let provider_config =
        resolve_provider_config(&loaded_config.config, &loaded_config.config.model.provider)
            .ok_or_else(|| {
                anyhow::anyhow!("unknown provider {}", loaded_config.config.model.provider)
            })?;
    let credential_store = CredentialStore::new(loaded_config.data_dir.clone());
    let api_key = project_dir
        .and_then(|project_dir| {
            resolve_provider_api_key_for_project(
                &credential_store,
                &provider_config,
                &loaded_config.config.model.provider,
                project_dir,
            )
        })
        .or_else(|| {
            resolve_provider_api_key(
                &credential_store,
                &provider_config,
                &loaded_config.config.model.provider,
            )
        })
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

    let provider_config = provider_config_for_api_key(
        &provider_config,
        &credential_store,
        &loaded_config.config.model.provider,
        &api_key,
    );

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
    let provider_config = inferred_provider_config_for_api_key(provider_config, &api_key);
    Ok(Arc::new(OpenAiProvider::from_provider_config_with_key(
        &provider_config,
        api_key,
    )?))
}

const CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

fn provider_config_for_api_key(
    provider_config: &ProviderConfig,
    credential_store: &CredentialStore,
    requested_provider_id: &str,
    api_key: &str,
) -> ProviderConfig {
    if uses_stored_openai_oauth(credential_store, &provider_config.id, api_key)
        || (requested_provider_id != provider_config.id
            && uses_stored_openai_oauth(credential_store, requested_provider_id, api_key))
    {
        return openai_chatgpt_codex_config(provider_config);
    }

    // xAI OAuth JWTs → Grok Build (cli-chat-proxy). OpenAiProvider also rewrites
    // base_url for OAuth JWTs; this keeps list/config paths consistent.
    inferred_provider_config_for_api_key(provider_config, api_key)
}

fn inferred_provider_config_for_api_key(
    provider_config: &ProviderConfig,
    api_key: &str,
) -> ProviderConfig {
    if provider_config.id == "openai" && is_probably_chatgpt_oauth_token(api_key) {
        return openai_chatgpt_codex_config(provider_config);
    }
    if provider_config.id == "xai" && navi_providers::is_xai_oauth_access_token(api_key) {
        return xai_grok_build_config(provider_config);
    }

    provider_config.clone()
}

/// Grok Build / official CLI chat proxy (subscription quota, not Platform API).
fn xai_grok_build_config(provider_config: &ProviderConfig) -> ProviderConfig {
    let mut config = provider_config.clone();
    config.base_url = Some(navi_providers::XAI_GROK_CLI_BASE_URL.to_string());
    config
}

fn uses_stored_openai_oauth(
    credential_store: &CredentialStore,
    provider_id: &str,
    api_key: &str,
) -> bool {
    provider_id == "openai"
        && credential_store.get_oauth_api_kind(provider_id).is_some()
        && credential_store
            .get_api_key(provider_id)
            .is_some_and(|stored| stored == api_key)
}

fn is_probably_chatgpt_oauth_token(api_key: &str) -> bool {
    !api_key.is_empty() && !api_key.starts_with("sk-") && api_key != "public"
}

fn openai_chatgpt_codex_config(provider_config: &ProviderConfig) -> ProviderConfig {
    let mut config = provider_config.clone();
    config.kind = ProviderKind::OpenAiResponses;
    config.base_url = Some(CHATGPT_CODEX_BASE_URL.to_string());
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use navi_core::config::ModelConfig;
    use navi_core::{NaviConfig, ProviderConfig, ProviderKind};

    #[test]
    fn xai_oauth_jwt_config_routes_to_grok_build_proxy() {
        let provider = ProviderConfig {
            id: "xai".into(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: "XAI_API_KEY".into(),
            base_url: Some("https://api.x.ai/v1".into()),
            ..ProviderConfig::default()
        };
        let rewritten = inferred_provider_config_for_api_key(
            &provider,
            "eyJhbGciOiJFUzI1NiIsInR5cCI6ImF0K2p3dCJ9.payload.sig",
        );
        assert_eq!(
            rewritten.base_url.as_deref(),
            Some(navi_providers::XAI_GROK_CLI_BASE_URL)
        );

        let platform = inferred_provider_config_for_api_key(&provider, "xai-platform-key");
        assert_eq!(platform.base_url.as_deref(), Some("https://api.x.ai/v1"));
    }

    #[test]
    fn build_local_tooling_succeeds_with_default_config() {
        let tempdir = tempfile::tempdir().unwrap();
        let loaded_config = LoadedConfig {
            config: NaviConfig::default(),
            global_config_path: None,
            project_config_path: None,
            data_dir: tempdir.path().to_path_buf(),
        };

        let result = build_local_tooling(
            &loaded_config,
            tempdir.path().to_path_buf(),
            &RuntimeComponents::default(),
        );
        assert!(
            result.is_ok(),
            "build_local_tooling should succeed with default config"
        );
    }

    // Needs a loadable wasm binary; placeholder bytes are rejected by the runtime.
    #[test]
    #[ignore = "needs real wasm fixture"]
    fn build_local_tooling_loads_installed_wasm_plugin_store() {
        use navi_plugin_manifest::{
            PluginManifest, PluginMeta, RuntimeKind, ToolDef, ToolRisk, installed_plugins_dir,
            sign_plugin_manifest_for_tests,
        };

        let tempdir = tempfile::tempdir().unwrap();
        let wasm = b"minimal-wasm-bytes";
        let plugins_root = installed_plugins_dir(tempdir.path());
        let plugin_dir = plugins_root.join("echo");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        let mut manifest = PluginManifest {
            plugin: PluginMeta {
                id: "echo".into(),
                name: "echo".into(),
                version: "1.0.0".into(),
                publisher: "gh:test".into(),
                runtime: RuntimeKind::WasmComponent,
                entry: "plugin.wasm".into(),
                wasm_hash: String::new(),
                signature: String::new(),
                public_key: None,
                minimum_navi: "0.1.0".into(),
            },
            capabilities: vec![],
            tools: vec![ToolDef {
                id: "echo".into(),
                summary: "Echo input".into(),
                risk: ToolRisk::ReadOnly,
                input_schema: None,
                capabilities: vec![],
            }],
        };
        sign_plugin_manifest_for_tests(&mut manifest, wasm);
        std::fs::write(
            plugin_dir.join("plugin.toml"),
            toml::to_string(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(plugin_dir.join("plugin.wasm"), wasm).unwrap();

        let entry = navi_plugin_manifest::lock_entry_from_manifest(&manifest, vec![]);
        navi_plugin_manifest::upsert_aggregate_lock_entry(&plugins_root, entry).unwrap();

        let loaded_config = LoadedConfig {
            config: NaviConfig::default(),
            global_config_path: None,
            project_config_path: None,
            data_dir: tempdir.path().to_path_buf(),
        };

        let tooling = build_local_tooling(
            &loaded_config,
            tempdir.path().to_path_buf(),
            &RuntimeComponents::default(),
        )
        .unwrap_or_else(|e| {
            panic!("build_local_tooling failed: {e:#}");
        });

        let names: Vec<String> = tooling
            .tool_executor
            .definitions()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert!(
            names.iter().any(|n| n.contains("echo")),
            "expected namespaced echo tool, got: {names:?}"
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

        let tooling = build_local_tooling(
            &loaded_config,
            tempdir.path().to_path_buf(),
            &RuntimeComponents::default(),
        )
        .unwrap();

        // Verify the executor can list definitions without panicking.
        let _definitions = tooling.tool_executor.definitions();
        // No plugins configured, so warnings should be empty.
        assert!(
            tooling.warnings.is_empty(),
            "no warnings expected with default config"
        );
    }

    #[test]
    fn build_local_tooling_warns_and_ignores_native_plugin_config() {
        use navi_core::PluginConfig;
        use std::path::PathBuf;

        let tempdir = tempfile::tempdir().unwrap();
        let loaded_config = LoadedConfig {
            config: NaviConfig {
                plugins: vec![PluginConfig {
                    path: PathBuf::from("/tmp/fake-native-plugin.so"),
                    enabled: true,
                }],
                ..Default::default()
            },
            global_config_path: None,
            project_config_path: None,
            data_dir: tempdir.path().to_path_buf(),
        };

        let tooling = build_local_tooling(
            &loaded_config,
            tempdir.path().to_path_buf(),
            &RuntimeComponents::default(),
        )
        .unwrap();

        assert!(
            tooling
                .warnings
                .iter()
                .any(|w| w.contains("native plugins are no longer loaded")),
            "expected deprecation warning, got: {:?}",
            tooling.warnings
        );
        assert!(tooling.tui_panels.is_empty());
        assert!(tooling.agent_policies.is_empty());
    }

    #[test]
    fn build_provider_for_config_returns_structured_error_for_missing_credentials() {
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

        let error = match build_provider_for_config(&loaded_config) {
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
    fn build_provider_for_config_returns_error_for_unknown_provider() {
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

        let error = match build_provider_for_config(&loaded_config) {
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
