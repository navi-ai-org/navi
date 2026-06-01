use anyhow::Result;
use async_trait::async_trait;
use navi_core::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};
use serde::{Deserialize, Serialize};
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

/// A host-tool invocation delivered by the engine.
///
/// Carries the invocation id as well as the model-produced JSON arguments so
/// host apps can correlate tool calls with their own logs or UI state.
#[derive(Debug, Clone)]
pub struct HostToolInvocation {
    pub invocation_id: String,
    pub input: Value,
}

/// Structured result returned by a host tool.
///
/// `ok` reports whether the tool completed successfully from the host app's
/// perspective. `output` is the JSON payload sent back to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SdkHostToolResult {
    pub ok: bool,
    pub output: Value,
}

impl SdkHostToolResult {
    /// Creates a successful host-tool result.
    pub fn success(output: Value) -> Self {
        Self { ok: true, output }
    }

    /// Creates a failed host-tool result with a structured JSON payload.
    pub fn failure(output: Value) -> Self {
        Self { ok: false, output }
    }
}

/// Trait for host applications to implement tool execution logic.
///
/// Implement this trait and wrap it in an `Arc` to register a custom tool
/// with `SdkHostTool`. The handler receives the model's JSON arguments and
/// returns a JSON result that will be sent back to the model.
#[async_trait]
pub trait HostToolHandler: Send + Sync {
    async fn invoke(&self, invocation: HostToolInvocation) -> Result<SdkHostToolResult>;
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
        let invocation_id = invocation.id;
        let result = self
            .handler
            .invoke(HostToolInvocation {
                invocation_id: invocation_id.clone(),
                input: invocation.input,
            })
            .await?;
        Ok(ToolResult {
            invocation_id,
            ok: result.ok,
            output: result.output,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct EchoHandler;

    #[async_trait]
    impl HostToolHandler for EchoHandler {
        async fn invoke(&self, invocation: HostToolInvocation) -> Result<SdkHostToolResult> {
            Ok(SdkHostToolResult::success(json!({
                "id": invocation.invocation_id,
                "input": invocation.input,
            })))
        }
    }

    struct FailureHandler;

    #[async_trait]
    impl HostToolHandler for FailureHandler {
        async fn invoke(&self, _invocation: HostToolInvocation) -> Result<SdkHostToolResult> {
            Ok(SdkHostToolResult::failure(json!({
                "error_code": "host_tool_failed",
                "message": "host tool failed semantically",
            })))
        }
    }

    fn definition() -> HostToolDefinition {
        HostToolDefinition {
            name: "test_host_tool".to_string(),
            description: "test".to_string(),
            kind: ToolKind::Read,
            input_schema: json!({ "type": "object" }),
        }
    }

    #[tokio::test]
    async fn host_tool_passes_invocation_metadata_to_handler() {
        let tool = SdkHostTool::new(definition(), Arc::new(EchoHandler));
        let result = tool
            .invoke(ToolInvocation {
                id: "call-1".to_string(),
                tool_name: "test_host_tool".to_string(),
                input: json!({ "value": 42 }),
            })
            .await
            .expect("invoke");

        assert!(result.ok);
        assert_eq!(result.invocation_id, "call-1");
        assert_eq!(result.output["id"], "call-1");
        assert_eq!(result.output["input"]["value"], 42);
    }

    #[tokio::test]
    async fn host_tool_can_return_structured_failure() {
        let tool = SdkHostTool::new(definition(), Arc::new(FailureHandler));
        let result = tool
            .invoke(ToolInvocation {
                id: "call-2".to_string(),
                tool_name: "test_host_tool".to_string(),
                input: json!({}),
            })
            .await
            .expect("invoke");

        assert!(!result.ok);
        assert_eq!(result.output["error_code"], "host_tool_failed");
    }
}
