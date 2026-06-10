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
    ApplyPatchTool, BashTool, BuildRunnerTool, FsBrowserTool, GitOpsTool, GrepTool,
    PackageManagerTool, PlanTool, QuestionTool, ReadFileTool, RuntimeInfoTool, TestRunnerTool,
    ToolWorkflowTool, TopFilesTool, WriteFileTool, truncate_tool_result,
};

/// Trait for executable tools that can be invoked by the agent.
///
/// Built-in tools (read_file, write_file, apply_patch, fs_browser, grep, bash,
/// test_runner, build_runner, git_ops, package_manager) implement this trait. Host applications can also
/// register custom tools via the SDK's host tool interface.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Returns the tool's definition (name, description, kind, input schema).
    fn definition(&self) -> ToolDefinition;

    /// Executes the tool with the given invocation and returns the result.
    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult>;
}

/// Describes a tool's name, purpose, security kind, and input schema.
///
/// This is sent to the model as part of the request so it knows which tools
/// are available and how to call them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique tool name used in invocations (e.g. `"read_file"`).
    pub name: String,
    /// Human-readable description of what the tool does.
    pub description: String,
    /// Security classification that determines approval behavior.
    pub kind: ToolKind,
    /// JSON Schema describing the tool's input parameters.
    #[serde(default)]
    pub input_schema: Value,
}

/// Security classification for a tool, used by [`SecurityPolicy`] to determine
/// whether invocation requires approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolKind {
    /// Read-only operations (default: allowed without approval).
    Read,
    /// File write operations (default: requires approval).
    Write,
    /// Shell command execution (default: requires approval).
    Command,
    /// Custom/plugin tools (default: requires approval).
    Custom,
}

/// A specific tool invocation requested by the model, with a unique id,
/// tool name, and JSON input arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolInvocation {
    /// Unique identifier for this invocation (matches tool call ids in messages).
    pub id: String,
    /// Name of the tool to invoke.
    pub tool_name: String,
    /// JSON input arguments for the tool.
    pub input: Value,
}

/// The result of a tool invocation, containing success/failure status and
/// JSON output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /// Identifier of the invocation this result responds to.
    pub invocation_id: String,
    /// Whether the tool executed successfully.
    pub ok: bool,
    /// JSON output from the tool (result data or error details).
    pub output: Value,
}

/// Registry and executor for tools, validating invocations against security
/// policy and JSON Schema before execution.
///
/// The executor holds a set of registered [`Tool`] implementations, their
/// compiled JSON Schema validators, and a [`SecurityPolicy`] that governs
/// whether invocations are allowed, need approval, or are denied.
pub struct ToolExecutor {
    tools: HashMap<String, Arc<dyn Tool>>,
    validators: HashMap<String, Arc<jsonschema::Validator>>,
    invalid_schemas: HashMap<String, String>,
    policy: SecurityPolicy,
    harness_profile: String,
}

/// Reasons a tool call can be rejected before execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallInvalid {
    /// The requested tool name is not registered.
    UnknownTool {
        /// The tool name that was requested.
        tool_name: String,
        /// List of tool names that are available.
        available_tools: Vec<String>,
    },
    /// The tool's input schema is malformed or could not be compiled.
    InvalidSchema {
        /// The tool name with the bad schema.
        tool_name: String,
        /// Description of the schema error.
        message: String,
    },
    /// The input arguments fail schema validation.
    InvalidArguments {
        /// The tool name with invalid arguments.
        tool_name: String,
        /// Descriptions of each validation problem.
        problems: Vec<String>,
        /// A valid example input for reference.
        example: Value,
    },
}

impl ToolExecutor {
    /// Creates a new executor with the given security policy and registers the
    /// built-in tools (read_file, write_file, apply_patch, fs_browser, grep, bash,
    /// test_runner, build_runner).
    pub fn new(policy: SecurityPolicy) -> Self {
        let mut executor = Self {
            tools: HashMap::new(),
            validators: HashMap::new(),
            invalid_schemas: HashMap::new(),
            policy,
            harness_profile: "medium".to_string(),
        };
        executor.register_builtin_tools();
        executor
    }

    /// Sets the harness profile label reported by the `runtime_info` tool.
    pub fn set_harness_profile(&mut self, profile: String) {
        self.harness_profile = profile;
        // Re-register runtime_info with the updated profile.
        self.register(RuntimeInfoTool::new(
            self.policy.clone(),
            self.harness_profile.clone(),
        ));
    }

    pub(crate) fn new_workflow_host(policy: SecurityPolicy) -> Self {
        let project_root = policy.project_root().to_path_buf();
        let mut executor = Self {
            tools: HashMap::new(),
            validators: HashMap::new(),
            invalid_schemas: HashMap::new(),
            policy,
            harness_profile: "medium".to_string(),
        };
        executor.register(ReadFileTool::new());
        executor.register(FsBrowserTool);
        executor.register(GrepTool);
        executor.register(GitOpsTool::new(project_root));
        executor
    }

    /// Returns definitions for all registered tools.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    /// Returns the definition for a specific tool by name, if registered.
    pub fn definition(&self, name: &str) -> Option<ToolDefinition> {
        self.tools.get(name).map(|tool| tool.definition())
    }

    /// Registers a tool, compiling its input schema for validation.
    ///
    /// Returns the previous tool with the same name, if any.
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

    /// Validates a tool invocation's arguments against the tool's JSON Schema.
    ///
    /// Returns `Ok(())` if valid, or a [`ToolCallInvalid`] error describing the problem.
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

    /// Returns sorted list of all registered tool names.
    pub fn tool_names(&self) -> Vec<String> {
        let mut names = self.tools.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    /// Removes WASM plugin tools (`plugin__*` namespaced IDs) so the host can reload plugins.
    pub fn unregister_plugin_tools(&mut self) {
        self.tools.retain(|name, _| !name.starts_with("plugin__"));
        self.validators
            .retain(|name, _| !name.starts_with("plugin__"));
        self.invalid_schemas
            .retain(|name, _| !name.starts_with("plugin__"));
    }

    /// Creates a failed [`ToolResult`] from an invalid tool call, including
    /// corrective advice for the model.
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
        let invocation = self.policy.normalize_invocation_paths(&invocation);
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
        let project_root = self.policy.project_root().to_path_buf();
        self.register(ReadFileTool::new());
        self.register(TopFilesTool::new(self.policy.clone()));
        self.register(WriteFileTool::new());
        self.register(ApplyPatchTool::new(project_root.clone()));
        self.register(FsBrowserTool);
        self.register(GrepTool);
        self.register(BashTool::new(project_root.clone()));
        self.register(TestRunnerTool::new(project_root.clone()));
        self.register(BuildRunnerTool::new(project_root.clone()));
        self.register(GitOpsTool::new(project_root.clone()));
        self.register(QuestionTool);
        self.register(PlanTool::new(self.policy.clone()));
        self.register(PackageManagerTool::new(project_root));
        self.register(ToolWorkflowTool::new(self.policy.clone()));
        self.register(RuntimeInfoTool::new(
            self.policy.clone(),
            self.harness_profile.clone(),
        ));
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

/// Generates a minimal example JSON value from a JSON Schema by extracting
/// required properties and filling in default/example values.
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
