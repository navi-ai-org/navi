use anyhow::Result;
use navi_core::ToolDefinition;

pub const NAVI_PLUGIN_API_VERSION: u32 = 1;
pub const NAVI_PLUGIN_ENTRYPOINT: &[u8] = b"navi_plugin_entrypoint";

pub type PluginCreate = unsafe fn() -> Box<dyn NaviPlugin>;

pub trait NaviPlugin: Send + Sync {
    fn metadata(&self) -> PluginMetadata;
    fn register(&self, registry: &mut dyn PluginRegistry) -> Result<()>;
}

pub trait PluginRegistry {
    fn register_tool(&mut self, definition: ToolDefinition);
    fn register_agent_policy(&mut self, name: &str);
    fn register_tui_component(&mut self, name: &str);
}

#[derive(Debug, Clone)]
pub struct PluginMetadata {
    pub name: String,
    pub version: String,
    pub api_version: u32,
    pub capabilities: Vec<PluginCapability>,
}

#[derive(Debug, Clone)]
pub enum PluginCapability {
    FileSystem,
    Shell,
    Network,
    Tui,
    Model,
    Session,
}
