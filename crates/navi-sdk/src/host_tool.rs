use anyhow::Result;
use async_trait::async_trait;
use navi_core::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};
use serde_json::Value;
use std::sync::Arc;

/// Metadata for a host-registered tool visible to the model.
///
/// The `name` and `description` appear in the model's tool list. `kind`
/// classifies the tool as read, write, or command for the approval policy.
/// `input_schema` is the JSON Schema the model uses to produce arguments.
#[derive(Clone)]
pub struct HostToolDefinition {
    pub name: String,
    pub description: String,
    pub kind: ToolKind,
    pub input_schema: Value,
}

/// Trait for host applications to implement tool execution logic.
///
/// Implement this trait and wrap it in an `Arc` to register a custom tool
/// with `SdkHostTool`. The handler receives the model's JSON arguments and
/// returns a JSON result that will be sent back to the model.
#[async_trait]
pub trait HostToolHandler: Send + Sync {
    async fn invoke(&self, input: Value) -> Result<Value>;
}

/// Adapter that bridges a [`HostToolDefinition`] and [`HostToolHandler`] into
/// the engine's `Tool` trait.
///
/// Create one with `SdkHostTool::new(definition, handler)` and pass it to
/// `NaviEngineBuilder::host_tool()` to register it with the engine.
pub struct SdkHostTool {
    definition: HostToolDefinition,
    handler: Arc<dyn HostToolHandler>,
}

impl SdkHostTool {
    /// Creates a new SDK host tool from a definition and an async handler.
    pub fn new(definition: HostToolDefinition, handler: Arc<dyn HostToolHandler>) -> Self {
        Self {
            definition,
            handler,
        }
    }
}

#[async_trait]
impl Tool for SdkHostTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.definition.name.clone(),
            description: self.definition.description.clone(),
            kind: self.definition.kind,
            input_schema: self.definition.input_schema.clone(),
        }
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let output = self.handler.invoke(invocation.input).await?;
        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: true,
            output,
        })
    }
}
