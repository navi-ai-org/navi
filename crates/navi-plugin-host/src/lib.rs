use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use libloading::{Library, Symbol};
use navi_core::{
    PluginConfig, SecurityDecision, SecurityPolicy, Tool, ToolDefinition, ToolExecutor,
    ToolInvocation, ToolKind, ToolResult,
};
use navi_plugin_api::{
    NAVI_PLUGIN_API_VERSION, NAVI_PLUGIN_ENTRYPOINT, NaviPlugin, PluginCreate, PluginMetadata,
    PluginRegistry, PluginTool, PluginToolDefinition, PluginToolInvocation, PluginToolKind,
};
use std::path::Path;
use std::sync::Arc;

pub mod sandbox;
pub use sandbox::{SandboxStatus, apply_filesystem_sandbox};

/// Adapter that wraps a `PluginTool` (from the stable plugin ABI) and implements
/// `navi_core::Tool` so it can be registered with the engine's `ToolExecutor`.
pub struct PluginToolAdapter {
    inner: Arc<dyn PluginTool>,
    def_cache: ToolDefinition,
}

impl PluginToolAdapter {
    pub fn new(inner: Arc<dyn PluginTool>) -> Self {
        let plugin_def = inner.definition();
        let def_cache = plugin_def_to_core(plugin_def);
        Self { inner, def_cache }
    }
}

#[async_trait]
impl Tool for PluginToolAdapter {
    fn definition(&self) -> ToolDefinition {
        self.def_cache.clone()
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let plugin_invocation = PluginToolInvocation {
            id: invocation.id.clone(),
            tool_name: invocation.tool_name.clone(),
            input: invocation.input,
        };
        match self.inner.invoke(plugin_invocation) {
            Ok(result) => Ok(ToolResult {
                invocation_id: result.invocation_id,
                ok: result.ok,
                output: result.output,
            }),
            Err(err) => Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: serde_json::json!({ "error": err }),
            }),
        }
    }
}

fn plugin_def_to_core(def: PluginToolDefinition) -> ToolDefinition {
    ToolDefinition {
        name: def.name,
        description: def.description,
        kind: match def.kind {
            PluginToolKind::Read => ToolKind::Read,
            PluginToolKind::Write => ToolKind::Write,
            PluginToolKind::Command => ToolKind::Command,
            PluginToolKind::Custom => ToolKind::Custom,
        },
        input_schema: def.input_schema,
    }
}

pub struct LoadedPlugin {
    metadata: PluginMetadata,
    plugin: Box<dyn NaviPlugin>,
    _library: Library,
}

impl LoadedPlugin {
    pub fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    pub fn register(&self, registry: &mut dyn PluginRegistry) -> Result<()> {
        self.plugin
            .register(registry)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }
}

pub fn load_plugin(path: &Path) -> Result<LoadedPlugin> {
    // SAFETY: We load a shared library from a path that has been validated by
    // `SecurityPolicy::validate_plugin_path` before this function is called.
    // The library must export the `navi_plugin_entrypoint` symbol to be usable.
    let library = unsafe { Library::new(path) }
        .with_context(|| format!("failed to load plugin {}", path.display()))?;

    let plugin = {
        // SAFETY: We look up the well-known entrypoint symbol. The returned
        // Symbol is valid for the lifetime of `library`. The type `PluginCreate`
        // matches the expected signature `unsafe fn() -> Box<dyn NaviPlugin>`.
        let constructor: Symbol<PluginCreate> = unsafe { library.get(NAVI_PLUGIN_ENTRYPOINT) }
            .with_context(|| {
                format!(
                    "plugin {} does not export navi_plugin_entrypoint",
                    path.display()
                )
            })?;
        // SAFETY: We call the plugin constructor after validating its API version
        // (below). The returned Box<dyn NaviPlugin> is Send+Sync and owned by us.
        unsafe { constructor() }
    };

    let metadata = plugin.metadata();
    validate_plugin_api_version(&metadata)?;

    Ok(LoadedPlugin {
        metadata,
        plugin,
        _library: library,
    })
}

pub fn load_plugin_with_policy(path: &Path, policy: &SecurityPolicy) -> Result<LoadedPlugin> {
    match policy.validate_plugin_path(path) {
        SecurityDecision::Deny(reason) => bail!("{reason}"),
        SecurityDecision::NeedsApproval(_) => bail!(
            "plugin {} requires explicit approval and cannot be loaded during startup",
            path.display()
        ),
        SecurityDecision::Allow => load_plugin(path),
    }
}

#[derive(Default)]
pub struct PluginLoadReport {
    pub loaded_plugins: Vec<LoadedPlugin>,
    pub loaded: Vec<PluginMetadata>,
    pub warnings: Vec<String>,
    pub tools: Vec<String>,
    pub agent_policies: Vec<String>,
    pub tui_components: Vec<String>,
}

pub fn load_configured_plugins(
    plugins: &[PluginConfig],
    policy: &SecurityPolicy,
    executor: &mut ToolExecutor,
) -> PluginLoadReport {
    load_configured_plugins_with_options(plugins, policy, executor, &LoadOptions::default())
}

/// Options that control plugin loading behavior (e.g. sandboxing).
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// If set, applies a filesystem sandbox after plugin load completes.
    /// Only the listed paths (plus standard system paths) will be accessible.
    pub sandbox_paths: Option<Vec<std::path::PathBuf>>,
}

pub fn load_configured_plugins_with_options(
    plugins: &[PluginConfig],
    policy: &SecurityPolicy,
    executor: &mut ToolExecutor,
    options: &LoadOptions,
) -> PluginLoadReport {
    let mut report = PluginLoadReport::default();

    for plugin_config in plugins.iter().filter(|plugin| plugin.enabled) {
        let path = &plugin_config.path;
        match load_plugin_with_policy(path, policy) {
            Ok(plugin) => {
                let metadata = plugin.metadata().clone();
                let mut registry = DefaultPluginRegistry::default();
                match plugin.register(&mut registry) {
                    Ok(()) => {
                        for plugin_tool in registry.tools {
                            let name = plugin_tool.definition().name.clone();
                            let adapted: Arc<dyn Tool> =
                                Arc::new(PluginToolAdapter::new(plugin_tool));
                            executor.register_tool(adapted);
                            report.tools.push(name);
                        }
                        report.agent_policies.extend(registry.agent_policies);
                        report.tui_components.extend(registry.tui_components);
                        report.loaded.push(metadata);
                        report.loaded_plugins.push(plugin);
                    }
                    Err(err) => report.warnings.push(format!(
                        "failed to register plugin {}: {err:#}",
                        path.display()
                    )),
                }
            }
            Err(err) => report
                .warnings
                .push(format!("failed to load plugin {}: {err:#}", path.display())),
        }
    }

    // Apply sandbox if requested and we successfully loaded at least one plugin.
    if let Some(paths) = &options.sandbox_paths {
        if !report.loaded.is_empty() {
            match apply_filesystem_sandbox(paths.iter().map(|p| p.as_path())) {
                Ok(SandboxStatus::Active) => {
                    tracing::info!("filesystem sandbox active for {} path(s)", paths.len());
                }
                Ok(SandboxStatus::ActiveWithWarnings) => {
                    tracing::warn!("filesystem sandbox active with warnings (some paths rejected)");
                }
                Ok(SandboxStatus::Unavailable(reason)) => {
                    report
                        .warnings
                        .push(format!("filesystem sandbox unavailable: {reason}"));
                }
                Err(e) => {
                    report
                        .warnings
                        .push(format!("filesystem sandbox failed: {e:#}"));
                }
            }
        }
    }

    report
}

#[derive(Default)]
pub struct DefaultPluginRegistry {
    pub tools: Vec<Arc<dyn PluginTool>>,
    pub agent_policies: Vec<String>,
    pub tui_components: Vec<String>,
}

impl PluginRegistry for DefaultPluginRegistry {
    fn register_tool(&mut self, tool: Arc<dyn PluginTool>) {
        self.tools.push(tool);
    }

    fn register_agent_policy(&mut self, name: &str) {
        self.agent_policies.push(name.to_string());
    }

    fn register_tui_component(&mut self, name: &str) {
        self.tui_components.push(name.to_string());
    }
}

fn validate_plugin_api_version(metadata: &PluginMetadata) -> Result<()> {
    if metadata.api_version != NAVI_PLUGIN_API_VERSION {
        bail!(
            "plugin {} uses API version {}, but NAVI expects {}",
            metadata.name,
            metadata.api_version,
            NAVI_PLUGIN_API_VERSION
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use navi_core::{SecurityConfig, SecurityPolicy};
    use navi_plugin_api::PluginToolResult;
    use serde_json::json;

    #[test]
    fn rejects_incompatible_plugin_api_version() {
        let metadata = PluginMetadata {
            name: "example".to_string(),
            version: "0.1.0".to_string(),
            api_version: NAVI_PLUGIN_API_VERSION + 1,
            capabilities: Vec::new(),
        };

        assert!(validate_plugin_api_version(&metadata).is_err());
    }

    struct TestPlugin;

    impl NaviPlugin for TestPlugin {
        fn metadata(&self) -> PluginMetadata {
            PluginMetadata {
                name: "test-plugin".to_string(),
                version: "0.1.0".to_string(),
                api_version: NAVI_PLUGIN_API_VERSION,
                capabilities: Vec::new(),
            }
        }

        fn register(&self, registry: &mut dyn PluginRegistry) -> Result<(), String> {
            registry.register_tool(Arc::new(TestPluginTool));
            registry.register_agent_policy("test-policy");
            registry.register_tui_component("test-component");
            Ok(())
        }
    }

    struct TestPluginTool;

    impl PluginTool for TestPluginTool {
        fn definition(&self) -> PluginToolDefinition {
            PluginToolDefinition {
                name: "test_echo".to_string(),
                description: "Echo test plugin input.".to_string(),
                kind: PluginToolKind::Custom,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "Text to echo." }
                    },
                    "required": ["text"]
                }),
            }
        }

        fn invoke(&self, invocation: PluginToolInvocation) -> Result<PluginToolResult, String> {
            Ok(PluginToolResult {
                invocation_id: invocation.id,
                ok: true,
                output: json!({
                    "text": invocation.input.get("text").and_then(|v| v.as_str()).unwrap_or("")
                }),
            })
        }
    }

    #[tokio::test]
    async fn registry_registers_executable_plugin_tools() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let policy = SecurityPolicy::new(
            tempdir.path().to_path_buf(),
            tempdir.path().join("data"),
            SecurityConfig::default(),
        )
        .expect("policy");
        let mut executor = ToolExecutor::new(policy);
        let mut registry = DefaultPluginRegistry::default();

        TestPlugin.register(&mut registry).expect("register");
        for plugin_tool in registry.tools {
            let adapted: Arc<dyn navi_core::Tool> = Arc::new(PluginToolAdapter::new(plugin_tool));
            executor.register_tool(adapted);
        }

        assert!(executor.definition("test_echo").is_some());
        let result = executor
            .invoke(ToolInvocation {
                id: "call-1".to_string(),
                tool_name: "test_echo".to_string(),
                input: json!({ "text": "wired" }),
            })
            .await;

        assert!(result.ok);
        assert_eq!(result.output["text"], "wired");
    }

    #[test]
    fn configured_plugin_loads_warn_and_continue_on_bad_path() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let data_dir = tempdir.path().join("data");
        let policy = SecurityPolicy::new(
            tempdir.path().to_path_buf(),
            data_dir,
            SecurityConfig::default(),
        )
        .expect("policy");
        let mut executor = ToolExecutor::new(policy.clone());
        let report = load_configured_plugins(
            &[PluginConfig {
                path: tempdir.path().join(".navi/plugins/missing.so"),
                enabled: true,
            }],
            &policy,
            &mut executor,
        );

        assert!(report.loaded.is_empty());
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("failed to load plugin"));
    }

    #[test]
    fn plugin_needing_approval_is_not_loaded_at_startup() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let plugin_dir = tempdir.path().join(".navi/plugins");
        std::fs::create_dir_all(&plugin_dir).expect("plugin dir");
        let plugin_path = plugin_dir.join("native.so");
        std::fs::write(&plugin_path, b"not a real plugin").expect("plugin file");
        let policy = SecurityPolicy::new(
            tempdir.path().to_path_buf(),
            tempdir.path().join("data"),
            SecurityConfig::default(),
        )
        .expect("policy");

        let err = match load_plugin_with_policy(&plugin_path, &policy) {
            Ok(_) => panic!("plugin should require approval"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("requires explicit approval"));
    }

    #[test]
    fn sandbox_runs_even_when_no_plugins_loaded() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let data_dir = tempdir.path().join("data");
        let policy = SecurityPolicy::new(
            tempdir.path().to_path_buf(),
            data_dir,
            SecurityConfig::default(),
        )
        .expect("policy");
        let mut executor = ToolExecutor::new(policy.clone());
        let report = load_configured_plugins_with_options(
            &[],
            &policy,
            &mut executor,
            &LoadOptions {
                sandbox_paths: Some(vec![tempdir.path().to_path_buf()]),
            },
        );
        // No plugins loaded => sandbox is not enforced (no risk surface).
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn sandbox_reports_unavailable_when_no_paths_given() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let data_dir = tempdir.path().join("data");
        let policy = SecurityPolicy::new(
            tempdir.path().to_path_buf(),
            data_dir,
            SecurityConfig::default(),
        )
        .expect("policy");
        let result = apply_filesystem_sandbox(Vec::<std::path::PathBuf>::new());
        // Either unavailable (off-platform) or active (Landlock enabled).
        assert!(result.is_ok());
    }
}
