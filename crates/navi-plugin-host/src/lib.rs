use anyhow::{Context, Result, bail};
use libloading::{Library, Symbol};
use navi_core::ToolDefinition;
use navi_plugin_api::{
    NAVI_PLUGIN_API_VERSION, NAVI_PLUGIN_ENTRYPOINT, NaviPlugin, PluginCreate, PluginMetadata,
    PluginRegistry,
};
use std::path::Path;

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

#[derive(Default)]
pub struct DefaultPluginRegistry {
    pub tools: Vec<ToolDefinition>,
    pub agent_policies: Vec<String>,
    pub tui_components: Vec<String>,
}

impl PluginRegistry for DefaultPluginRegistry {
    fn register_tool(&mut self, definition: ToolDefinition) {
        self.tools.push(definition);
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
}
