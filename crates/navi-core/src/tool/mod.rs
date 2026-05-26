use crate::security::{SecurityDecision, SecurityPolicy};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

mod builtin;
#[cfg(test)]
mod tests;

use builtin::{
    ApplyPatchTool, BashTool, GrepTool, ListFilesTool, ReadFileTool, WriteFileTool,
    truncate_tool_result,
};

#[async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub kind: ToolKind,
    #[serde(default)]
    pub input_schema: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolKind {
    Read,
    Write,
    Command,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub id: String,
    pub tool_name: String,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub invocation_id: String,
    pub ok: bool,
    pub output: Value,
}

pub struct ToolExecutor {
    tools: HashMap<String, Arc<dyn Tool>>,
    validators: HashMap<String, Arc<jsonschema::Validator>>,
    invalid_schemas: HashMap<String, String>,
    policy: SecurityPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallInvalid {
    UnknownTool {
        tool_name: String,
        available_tools: Vec<String>,
    },
    InvalidSchema {
        tool_name: String,
        message: String,
    },
    InvalidArguments {
        tool_name: String,
        problems: Vec<String>,
        example: Value,
    },
}

impl ToolExecutor {
    pub fn new(policy: SecurityPolicy) -> Self {
        let mut executor = Self {
            tools: HashMap::new(),
            validators: HashMap::new(),
            invalid_schemas: HashMap::new(),
            policy,
        };
        executor.register_builtin_tools();
        executor
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    pub fn definition(&self, name: &str) -> Option<ToolDefinition> {
        self.tools.get(name).map(|tool| tool.definition())
    }

    pub fn register_tool(&mut self, tool: Arc<dyn Tool>) -> Option<Arc<dyn Tool>> {
        let definition = tool.definition();
        let name = definition.name.clone();
        match jsonschema::validator_for(&definition.input_schema) {
            Ok(validator) => {
                self.validators.insert(name.clone(), Arc::new(validator));
                self.invalid_schemas.remove(&name);
            }
            Err(err) => {
                self.validators.remove(&name);
                self.invalid_schemas.insert(name.clone(), err.to_string());
                tracing::warn!(tool = %name, error = %err, "tool input schema is invalid");
            }
        }
        self.tools.insert(name, tool)
    }

    pub fn validate_arguments(
        &self,
        invocation: &ToolInvocation,
    ) -> std::result::Result<(), ToolCallInvalid> {
        let Some(_definition) = self.definition(&invocation.tool_name) else {
            return Err(ToolCallInvalid::UnknownTool {
                tool_name: invocation.tool_name.clone(),
                available_tools: self.tool_names(),
            });
        };
        if let Some(error) = self.invalid_schemas.get(&invocation.tool_name) {
            return Err(ToolCallInvalid::InvalidSchema {
                tool_name: invocation.tool_name.clone(),
                message: error.clone(),
            });
        }
        let Some(validator) = self.validators.get(&invocation.tool_name) else {
            return Err(ToolCallInvalid::InvalidSchema {
                tool_name: invocation.tool_name.clone(),
                message: "missing input schema validator".to_string(),
            });
        };
        let errors = validator
            .iter_errors(&invocation.input)
            .take(4)
            .map(|error| {
                let path = error.instance_path().to_string();
                if path.is_empty() {
                    error.to_string()
                } else {
                    format!("{error} at {path}")
                }
            })
            .collect::<Vec<_>>();
        if !errors.is_empty() {
            let example = self
                .definition(&invocation.tool_name)
                .map(|definition| example_from_schema(&definition.input_schema))
                .unwrap_or_else(|| json!({}));
            return Err(ToolCallInvalid::InvalidArguments {
                tool_name: invocation.tool_name.clone(),
                problems: errors,
                example,
            });
        }
        Ok(())
    }

    pub fn tool_names(&self) -> Vec<String> {
        let mut names = self.tools.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    pub fn invalid_tool_result(
        &self,
        invocation: &ToolInvocation,
        invalid: ToolCallInvalid,
    ) -> ToolResult {
        ToolResult {
            invocation_id: invocation.id.clone(),
            ok: false,
            output: tool_call_advice(invalid),
        }
    }

    pub fn validate(&self, invocation: &ToolInvocation) -> SecurityDecision {
        if let Err(err) = self.validate_arguments(invocation) {
            let message = tool_call_advice_message(&err);
            tracing::warn!(tool = %invocation.tool_name, invocation_id = %invocation.id, error = %message, "tool argument validation denied");
            return SecurityDecision::Deny(message);
        }
        let Some(definition) = self.definition(&invocation.tool_name) else {
            tracing::warn!(tool = %invocation.tool_name, "unknown tool validation denied");
            return SecurityDecision::Deny(format!("unknown tool `{}`", invocation.tool_name));
        };
        let decision = self
            .policy
            .validate_tool_invocation(&definition, invocation);
        match &decision {
            SecurityDecision::Allow => {
                tracing::debug!(tool = %invocation.tool_name, invocation_id = %invocation.id, "tool validation allowed");
            }
            SecurityDecision::NeedsApproval(_) => {
                tracing::info!(tool = %invocation.tool_name, invocation_id = %invocation.id, "tool validation requires approval");
            }
            SecurityDecision::Deny(reason) => {
                tracing::warn!(tool = %invocation.tool_name, invocation_id = %invocation.id, reason = %reason, "tool validation denied");
            }
        }
        decision
    }

    pub async fn invoke(&self, invocation: ToolInvocation) -> ToolResult {
        let invocation_id = invocation.id.clone();
        let tool_name = invocation.tool_name.clone();
        let started_at = std::time::Instant::now();
        if let Err(invalid) = self.validate_arguments(&invocation) {
            tracing::warn!(tool = %tool_name, invocation_id = %invocation_id, error = %tool_call_advice_message(&invalid), "tool argument validation denied");
            return self.invalid_tool_result(&invocation, invalid);
        }
        if let SecurityDecision::Deny(reason) = self.validate(&invocation) {
            tracing::warn!(tool = %tool_name, invocation_id = %invocation_id, reason = %reason, "tool invocation blocked");
            return ToolResult {
                invocation_id,
                ok: false,
                output: json!({ "error": reason }),
            };
        }
        let Some(tool) = self.tools.get(&invocation.tool_name).cloned() else {
            tracing::warn!(tool = %tool_name, invocation_id = %invocation_id, "unknown tool invocation");
            return ToolResult {
                invocation_id,
                ok: false,
                output: json!({ "error": format!("unknown tool `{}`", invocation.tool_name) }),
            };
        };

        tracing::info!(tool = %tool_name, invocation_id = %invocation_id, "tool invocation started");
        let result = match tool.invoke(invocation).await {
            Ok(result) => truncate_tool_result(result),
            Err(err) => ToolResult {
                invocation_id,
                ok: false,
                output: json!({ "error": format!("{err:#}") }),
            },
        };
        tracing::info!(
            tool = %tool_name,
            invocation_id = %result.invocation_id,
            ok = result.ok,
            duration_ms = started_at.elapsed().as_millis() as u64,
            "tool invocation finished"
        );
        result
    }

    fn register(&mut self, tool: impl Tool + 'static) {
        self.register_tool(Arc::new(tool));
    }

    fn register_builtin_tools(&mut self) {
        self.register(ReadFileTool);
        self.register(WriteFileTool);
        self.register(ApplyPatchTool);
        self.register(ListFilesTool);
        self.register(GrepTool);
        self.register(BashTool::new());
    }
}

fn tool_call_advice(invalid: ToolCallInvalid) -> Value {
    match invalid {
        ToolCallInvalid::UnknownTool {
            tool_name,
            available_tools,
        } => json!({
            "error_code": "unknown_tool",
            "tool": tool_name,
            "message": "Requested tool is not registered. Use one of the available tool names.",
            "available_tools": available_tools,
        }),
        ToolCallInvalid::InvalidSchema { tool_name, message } => json!({
            "error_code": "invalid_schema",
            "tool": tool_name,
            "message": format!("Tool schema is invalid: {message}"),
        }),
        ToolCallInvalid::InvalidArguments {
            tool_name,
            problems,
            example,
        } => json!({
            "error_code": "invalid_arguments",
            "tool": tool_name,
            "message": "Tool arguments do not match the JSON schema. Fix the arguments and call the tool again.",
            "problems": problems,
            "example": example,
        }),
    }
}

fn tool_call_advice_message(invalid: &ToolCallInvalid) -> String {
    match invalid {
        ToolCallInvalid::UnknownTool { tool_name, .. } => {
            format!("unknown tool `{tool_name}`")
        }
        ToolCallInvalid::InvalidSchema { tool_name, message } => {
            format!("invalid input schema for tool `{tool_name}`: {message}")
        }
        ToolCallInvalid::InvalidArguments {
            tool_name,
            problems,
            ..
        } => {
            format!(
                "invalid arguments for tool `{}`: {}",
                tool_name,
                problems.join("; ")
            )
        }
    }
}

pub fn example_from_schema(schema: &Value) -> Value {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return json!({});
    };
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let mut example = serde_json::Map::new();
    for field in required {
        let value = properties
            .get(field)
            .and_then(|property| property.get("type"))
            .and_then(Value::as_str)
            .map(|kind| match kind {
                "integer" => json!(1),
                "number" => json!(1.0),
                "boolean" => json!(true),
                "array" => json!([]),
                "object" => json!({}),
                _ => json!("example"),
            })
            .unwrap_or_else(|| json!("example"));
        example.insert(field.to_string(), value);
    }
    Value::Object(example)
}
