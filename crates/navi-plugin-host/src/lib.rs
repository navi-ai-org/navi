use anyhow::{Context, Result, bail};
use libloading::{Library, Symbol};
use navi_core::{PluginConfig, SecurityDecision, SecurityPolicy, Tool, ToolExecutor};
use navi_plugin_api::{
    NAVI_PLUGIN_API_VERSION, NAVI_PLUGIN_ENTRYPOINT, NaviPlugin, PluginCreate, PluginMetadata,
    PluginRegistry,
};
use std::path::Path;
use std::sync::Arc;

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
        self.plugin.register(registry)
    }
}

pub fn load_plugin(path: &Path) -> Result<LoadedPlugin> {
    let library = unsafe { Library::new(path) }
        .with_context(|| format!("failed to load plugin {}", path.display()))?;

    let plugin = {
        let constructor: Symbol<PluginCreate> = unsafe { library.get(NAVI_PLUGIN_ENTRYPOINT) }
            .with_context(|| {
                format!(
                    "plugin {} does not export navi_plugin_entrypoint",
                    path.display()
                )
            })?;
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
        SecurityDecision::Allow | SecurityDecision::NeedsApproval(_) => load_plugin(path),
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
    let mut report = PluginLoadReport::default();

    for plugin_config in plugins.iter().filter(|plugin| plugin.enabled) {
        let path = &plugin_config.path;
        match load_plugin_with_policy(path, policy) {
            Ok(plugin) => {
                let metadata = plugin.metadata().clone();
                let mut registry = DefaultPluginRegistry::default();
                match plugin.register(&mut registry) {
                    Ok(()) => {
                        for tool in registry.tools {
                            let name = tool.definition().name;
                            executor.register_tool(tool);
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

    report
}

#[derive(Default)]
pub struct DefaultPluginRegistry {
    pub tools: Vec<Arc<dyn Tool>>,
    pub agent_policies: Vec<String>,
    pub tui_components: Vec<String>,
}

impl PluginRegistry for DefaultPluginRegistry {
    fn register_tool(&mut self, tool: Arc<dyn Tool>) {
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
    use navi_core::{SecurityConfig, ToolDefinition, ToolInvocation, ToolKind, ToolResult};
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

        fn register(&self, registry: &mut dyn PluginRegistry) -> Result<()> {
            registry.register_tool(Arc::new(TestTool));
            registry.register_agent_policy("test-policy");
            registry.register_tui_component("test-component");
            Ok(())
        }
    }

    struct TestTool;

    #[async_trait::async_trait]
    impl Tool for TestTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "test_echo".to_string(),
                description: "Echo test plugin input.".to_string(),
                kind: ToolKind::Custom,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "Text to echo." }
                    },
                    "required": ["text"]
                }),
            }
        }

        async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
            Ok(ToolResult {
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
        for tool in registry.tools {
            executor.register_tool(tool);
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
}
