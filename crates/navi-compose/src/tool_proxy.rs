//! Plugin tools: a third-party component's tool, exposed to the agent.
//!
//! The plugin runs out of process (`matrix-host` spawns it); the agent sees a
//! normal tool whose invocation is admitted by the kernel and delivered over
//! `matrix.component` to the child. Because the *dispatch* goes through the
//! kernel by capability name, reloading the plugin swaps the implementation
//! while the tool name/schema stay stable for the session.

use std::sync::Arc;

use matrix_core::Kernel;
use navi_core::tool::{ToolDefinition, ToolInvocation, ToolKind, ToolResult};
use serde_json::Value;

/// Tool definition fetched from an active plugin (`navi.tool.definition@1`).
#[derive(Debug, Clone, PartialEq)]
pub struct PluginToolSpec {
    /// Component that provides the tool.
    pub component: String,
    /// Tool name shown to the model.
    pub name: String,
    /// Tool description shown to the model.
    pub description: String,
    /// Versioned capability invoked through the kernel.
    pub capability: String,
    /// JSON schema for the tool input.
    pub input_schema: Value,
}

impl PluginToolSpec {
    /// Parses the plugin's `navi.tool.definition@1` response.
    pub fn from_response(component: &str, value: &Value) -> Option<Self> {
        Some(Self {
            component: component.to_string(),
            name: value.get("name")?.as_str()?.to_string(),
            description: value
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("Third-party plugin tool")
                .to_string(),
            capability: value.get("capability")?.as_str()?.to_string(),
            input_schema: value
                .get("input_schema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"type": "object"})),
        })
    }

    /// The tool definition NAVI registers for the session.
    pub fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            self.name.clone(),
            format!("{} (third-party plugin `{}`)", self.description, self.component),
            ToolKind::Custom,
            self.input_schema.clone(),
        )
    }
}

/// A `navi_core::Tool` that forwards invocations to a plugin process.
pub struct PluginToolProxy {
    kernel: Arc<Kernel>,
    spec: PluginToolSpec,
}

impl PluginToolProxy {
    /// Builds a proxy for an active plugin tool.
    pub fn new(kernel: Arc<Kernel>, spec: PluginToolSpec) -> Self {
        Self { kernel, spec }
    }

    /// The spec this proxy serves.
    pub fn spec(&self) -> &PluginToolSpec {
        &self.spec
    }
}

#[async_trait::async_trait]
impl navi_core::tool::Tool for PluginToolProxy {
    fn definition(&self) -> ToolDefinition {
        self.spec.definition()
    }

    async fn invoke(&self, invocation: ToolInvocation) -> anyhow::Result<ToolResult> {
        let kernel = self.kernel.clone();
        let capability = self.spec.capability.clone();
        let input = invocation.input.clone();
        let invocation_id = invocation.id.clone();
        // The kernel is synchronous (and the plugin call blocks on the socket
        // round trip), so it runs on a blocking thread: the agent loop keeps
        // its async executor free.
        let (output, ok) = tokio::task::spawn_blocking(move || kernel.invoke(&capability, &input))
            .await
            .map_err(|err| anyhow::anyhow!("plugin call panicked: {err}"))?;
        Ok(ToolResult {
            invocation_id,
            ok,
            output,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spec_parses_a_plugin_definition() {
        let response = json!({
            "name": "roll_dice",
            "description": "Roll dice",
            "capability": "navi.tool.roll_dice@1",
            "input_schema": {"type": "object", "properties": {"sides": {"type": "integer"}}}
        });
        let spec = PluginToolSpec::from_response("navi.plugin.roll_dice", &response).expect("spec");
        assert_eq!(spec.name, "roll_dice");
        assert_eq!(spec.capability, "navi.tool.roll_dice@1");
        let definition = spec.definition();
        assert_eq!(definition.name, "roll_dice");
        assert_eq!(definition.kind, ToolKind::Custom);
        assert!(
            definition.description.contains("navi.plugin.roll_dice"),
            "the definition must name the providing component: {}",
            definition.description
        );
    }

    #[test]
    fn malformed_definition_is_rejected() {
        assert!(PluginToolSpec::from_response("x", &json!({"description": "no name"})).is_none());
        assert!(
            PluginToolSpec::from_response("x", &json!({"name": "n", "capability": 3})).is_none()
        );
    }
}
