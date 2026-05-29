use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

pub const NAVI_PLUGIN_API_VERSION: u32 = 2;
pub const NAVI_PLUGIN_ENTRYPOINT: &[u8] = b"navi_plugin_entrypoint";

pub type PluginCreate = unsafe fn() -> Box<dyn NaviPlugin>;

pub trait NaviPlugin: Send + Sync {
    fn metadata(&self) -> PluginMetadata;
    fn register(&self, registry: &mut dyn PluginRegistry) -> Result<(), String>;
}

pub trait PluginRegistry {
    fn register_tool(&mut self, tool: Arc<dyn PluginTool>);
    fn register_agent_policy(&mut self, name: &str);
    fn register_tui_component(&mut self, name: &str);
}

/// Self-contained tool trait for plugins. Does not depend on navi-core types.
/// Plugin authors implement this trait; the host adapts it to `navi_core::Tool`.
pub trait PluginTool: Send + Sync {
    fn definition(&self) -> PluginToolDefinition;
    fn invoke(&self, invocation: PluginToolInvocation) -> Result<PluginToolResult, String>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToolDefinition {
    pub name: String,
    pub description: String,
    pub kind: PluginToolKind,
    #[serde(default)]
    pub input_schema: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginToolKind {
    Read,
    Write,
    Command,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginToolInvocation {
    pub id: String,
    pub tool_name: String,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginToolResult {
    pub invocation_id: String,
    pub ok: bool,
    pub output: Value,
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
