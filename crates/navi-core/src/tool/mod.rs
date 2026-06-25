use crate::event::AgentEvent;
use crate::file_lock::FileLockManager;
use crate::security::{SecurityDecision, SecurityPolicy};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;

pub mod background;
pub(crate) mod builtin;
#[cfg(test)]
mod tests;

use builtin::{
    AppendNoteTool, ApplyPatchTool, BashTool, CodeDiagnosticsTool, FindReferencesTool,
    FindSymbolTool, FsBrowserTool, GitOpsTool, GrepTool, HistoryOpsTool, InitSessionTool,
    InsertAfterSymbolTool, InsertBeforeSymbolTool, MarkFeatureDoneTool, PackageManagerTool,
    PlanTool, QuestionTool, ReadFileTool, RenameSymbolTool, ReplaceSymbolBodyTool, RuntimeInfoTool,
    SymbolsOverviewTool, ToolWorkflowTool, TopFilesTool, WaitTool, WriteFileTool,
    truncate_tool_result,
};

pub use builtin::{ProviderBuilderFn, RepoExploreTool, SubagentTool};

#[async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult>;
    async fn invoke_with_context(
        &self,
        invocation: ToolInvocation,
        context: ToolInvocationContext,
    ) -> Result<ToolResult> {
        let _ = context;
        self.invoke(invocation).await
    }
}

#[derive(Clone, Default)]
pub struct ToolInvocationContext {
    pub event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
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
    harness_profile: String,
    lock_manager: Option<Arc<FileLockManager>>,
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
    MalformedArguments {
        tool_name: String,
        raw_arguments_preview: String,
        example: Value,
    },
    InvalidArguments {
        tool_name: String,
        problems: Vec<String>,
        example: Value,
    },
}

const WRITE_TOOL_NAMES: &[&str] = &["write_file", "apply_patch"];

impl ToolExecutor {
    pub fn new(policy: SecurityPolicy) -> Self {
        let mut executor = Self {
            tools: HashMap::new(),
            validators: HashMap::new(),
            invalid_schemas: HashMap::new(),
            policy,
            harness_profile: "medium".to_string(),
            lock_manager: None,
        };
        executor.register_builtin_tools();
        executor
    }

    /// Sets the file lock manager for cross-instance file locking.
    /// Re-registers write tools with lock awareness and registers the WaitTool.
    pub fn set_lock_manager(&mut self, lock_manager: Arc<FileLockManager>) {
        self.lock_manager = Some(lock_manager.clone());
        self.register(WriteFileTool::with_lock_manager(lock_manager.clone()));
        let pr = self.policy.project_root().to_path_buf();
        self.register(ApplyPatchTool::with_lock_manager(pr, lock_manager.clone()));
        self.register(WaitTool::new(lock_manager));
    }

    /// Sets the session ID on the lock manager (for lock metadata).
    pub fn set_session_id_on_locks(&self, session_id: &str) {
        if let Some(ref lm) = self.lock_manager {
            lm.set_session_id(session_id);
        }
    }

    pub fn lock_manager(&self) -> Option<&Arc<FileLockManager>> {
        self.lock_manager.as_ref()
    }

    pub fn set_harness_profile(&mut self, profile: String) {
        self.harness_profile = profile;
        self.register(RuntimeInfoTool::new(
            self.policy.clone(),
            self.harness_profile.clone(),
        ));
    }

    pub(crate) fn new_workflow_host(policy: SecurityPolicy) -> Self {
        let pr = policy.project_root().to_path_buf();
        let mut executor = Self {
            tools: HashMap::new(),
            validators: HashMap::new(),
            invalid_schemas: HashMap::new(),
            policy,
            harness_profile: "medium".to_string(),
            lock_manager: None,
        };
        executor.register(ReadFileTool::new());
        executor.register(FsBrowserTool);
        executor.register(GrepTool::new(pr.clone()));
        executor.register(GitOpsTool::new(pr));
        executor
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| model_friendly_definition(tool.definition()))
            .collect()
    }

    pub fn definition(&self, name: &str) -> Option<ToolDefinition> {
        self.tools.get(name).map(|t| t.definition())
    }

    pub fn register_tool(&mut self, tool: Arc<dyn Tool>) -> Option<Arc<dyn Tool>> {
        let def = tool.definition();
        let name = def.name.clone();
        match jsonschema::validator_for(&def.input_schema) {
            Ok(v) => {
                self.validators.insert(name.clone(), Arc::new(v));
                self.invalid_schemas.remove(&name);
            }
            Err(e) => {
                self.validators.remove(&name);
                self.invalid_schemas.insert(name.clone(), e.to_string());
                tracing::warn!(tool = %name, error = %e, "invalid schema");
            }
        }
        self.tools.insert(name, tool)
    }

    pub fn validate_arguments(
        &self,
        inv: &ToolInvocation,
    ) -> std::result::Result<(), ToolCallInvalid> {
        let Some(_) = self.definition(&inv.tool_name) else {
            return Err(ToolCallInvalid::UnknownTool {
                tool_name: inv.tool_name.clone(),
                available_tools: self.tool_names(),
            });
        };
        if let Some(e) = self.invalid_schemas.get(&inv.tool_name) {
            return Err(ToolCallInvalid::InvalidSchema {
                tool_name: inv.tool_name.clone(),
                message: e.clone(),
            });
        }
        if let Some(raw) = inv.input.get("raw_arguments").and_then(Value::as_str) {
            return Err(ToolCallInvalid::MalformedArguments {
                tool_name: inv.tool_name.clone(),
                raw_arguments_preview: raw.chars().take(200).collect(),
                example: self
                    .definition(&inv.tool_name)
                    .map(|d| example_from_schema(&d.input_schema))
                    .unwrap_or(json!({})),
            });
        }
        let Some(v) = self.validators.get(&inv.tool_name) else {
            return Err(ToolCallInvalid::InvalidSchema {
                tool_name: inv.tool_name.clone(),
                message: "missing validator".into(),
            });
        };
        let errors: Vec<String> = v
            .iter_errors(&inv.input)
            .take(4)
            .map(|e| {
                let p = e.instance_path().to_string();
                if p.is_empty() {
                    e.to_string()
                } else {
                    format!("{e} at {p}")
                }
            })
            .collect();
        if !errors.is_empty() {
            return Err(ToolCallInvalid::InvalidArguments {
                tool_name: inv.tool_name.clone(),
                problems: errors,
                example: self
                    .definition(&inv.tool_name)
                    .map(|d| example_from_schema(&d.input_schema))
                    .unwrap_or(json!({})),
            });
        }
        Ok(())
    }

    pub fn tool_names(&self) -> Vec<String> {
        let mut n: Vec<String> = self.tools.keys().cloned().collect();
        n.sort();
        n
    }

    pub fn unregister_plugin_tools(&mut self) {
        self.tools.retain(|n, _| !n.starts_with("plugin__"));
        self.validators.retain(|n, _| !n.starts_with("plugin__"));
        self.invalid_schemas
            .retain(|n, _| !n.starts_with("plugin__"));
    }

    pub fn invalid_tool_result(&self, inv: &ToolInvocation, err: ToolCallInvalid) -> ToolResult {
        ToolResult {
            invocation_id: inv.id.clone(),
            ok: false,
            output: tool_call_advice(err),
        }
    }

    fn check_file_lock(&self, inv: &ToolInvocation) -> Option<ToolResult> {
        let Some(ref lm) = self.lock_manager else {
            return None;
        };
        if !WRITE_TOOL_NAMES.contains(&inv.tool_name.as_str()) {
            return None;
        }
        let path_str = inv.input.get("path").and_then(Value::as_str)?;
        let path = Path::new(path_str);
        let info = match lm.is_locked(path) {
            Ok(Some(i)) => i,
            Ok(None) => return None,
            Err(e) => {
                return Some(ToolResult {
                    invocation_id: inv.id.clone(),
                    ok: false,
                    output: json!({"error": format!("lock check failed: {e}"), "error_code": "lock_check_failed"}),
                });
            }
        };
        Some(ToolResult {
            invocation_id: inv.id.clone(),
            ok: false,
            output: json!({
                "error": format!("O arquivo `{}` está bloqueado por outra instância ({}). Use `wait` com `file_path=\"{}\"`.", info.path.display(), info.instance_id, info.path.display()),
                "error_code": "file_locked", "file_path": info.path.to_string_lossy(),
                "locked_by_instance": info.instance_id, "locked_by_session": info.session_id,
                "hint": "Use `wait` tool with `file_path` to await unlock.",
            }),
        })
    }

    pub fn validate(&self, inv: &ToolInvocation) -> SecurityDecision {
        if let Err(e) = self.validate_arguments(inv) {
            return SecurityDecision::Deny(tool_call_advice_message(&e));
        }
        let Some(def) = self.definition(&inv.tool_name) else {
            return SecurityDecision::Deny(format!("unknown `{}`", inv.tool_name));
        };
        self.policy.validate_tool_invocation(&def, inv)
    }

    pub async fn invoke(&self, invocation: ToolInvocation) -> ToolResult {
        self.invoke_with_event_tx(invocation, None).await
    }

    pub async fn invoke_with_event_tx(
        &self,
        invocation: ToolInvocation,
        event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> ToolResult {
        let inv_id = invocation.id.clone();
        let tool_name = invocation.tool_name.clone();
        let started = std::time::Instant::now();
        let invocation = self.policy.normalize_invocation_paths(&invocation);
        if let Err(e) = self.validate_arguments(&invocation) {
            return self.invalid_tool_result(&invocation, e);
        }
        if let SecurityDecision::Deny(r) = self.validate(&invocation) {
            return ToolResult {
                invocation_id: inv_id,
                ok: false,
                output: json!({"error": r}),
            };
        }
        if let Some(r) = self.check_file_lock(&invocation) {
            return r;
        }
        let Some(tool) = self.tools.get(&invocation.tool_name).cloned() else {
            return ToolResult {
                invocation_id: inv_id,
                ok: false,
                output: json!({"error": format!("unknown `{}`", invocation.tool_name)}),
            };
        };
        let result = match tool
            .invoke_with_context(invocation, ToolInvocationContext { event_tx })
            .await
        {
            Ok(r) => truncate_tool_result(r),
            Err(e) => ToolResult {
                invocation_id: inv_id,
                ok: false,
                output: json!({"error": format!("{e:#}")}),
            },
        };
        tracing::info!(tool = %tool_name, ok = result.ok, dur_ms = started.elapsed().as_millis() as u64, "invoke finished");
        result
    }

    /// Lists all active background bash commands.
    pub async fn list_background_commands(&self) -> Vec<background::BackgroundCommandSnapshot> {
        let r = self
            .invoke(ToolInvocation {
                id: "bg-list".into(),
                tool_name: "bash".into(),
                input: json!({"action": "list"}),
            })
            .await;
        r.output
            .get("tasks")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(background::BackgroundCommandSnapshot::from_json)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Polls a specific background bash command.
    pub async fn poll_background_command(
        &self,
        task_id: &str,
    ) -> Option<background::BackgroundCommandSnapshot> {
        let r = self
            .invoke(ToolInvocation {
                id: "bg-poll".into(),
                tool_name: "bash".into(),
                input: json!({"task_id": task_id}),
            })
            .await;
        if r.ok {
            background::BackgroundCommandSnapshot::from_json(&r.output)
        } else {
            None
        }
    }

    /// Cancels a specific background bash command.
    pub async fn cancel_background_command(
        &self,
        task_id: &str,
    ) -> Option<background::BackgroundCommandSnapshot> {
        let r = self
            .invoke(ToolInvocation {
                id: "bg-cancel".into(),
                tool_name: "bash".into(),
                input: json!({"task_id": task_id, "action": "cancel"}),
            })
            .await;
        if r.ok {
            background::BackgroundCommandSnapshot::from_json(&r.output)
        } else {
            None
        }
    }

    fn register(&mut self, tool: impl Tool + 'static) {
        self.register_tool(Arc::new(tool));
    }

    fn register_builtin_tools(&mut self) {
        let pr = self.policy.project_root().to_path_buf();
        self.register(ReadFileTool::new());
        self.register(TopFilesTool::new(self.policy.clone()));
        self.register(WriteFileTool::new());
        self.register(ApplyPatchTool::new(pr.clone()));
        self.register(FsBrowserTool);
        self.register(GrepTool::new(pr.clone()));
        self.register(GrepTool::search_alias(pr.clone()));
        self.register(BashTool::new(pr.clone()));
        self.register(GitOpsTool::new(pr.clone()));
        self.register(QuestionTool);
        self.register(PlanTool::new(self.policy.clone()));
        self.register(PackageManagerTool::new(pr.clone()));
        self.register(ToolWorkflowTool::new(self.policy.clone()));
        self.register(RuntimeInfoTool::new(
            self.policy.clone(),
            self.harness_profile.clone(),
        ));
        self.register(SymbolsOverviewTool::new(self.policy.clone()));
        self.register(FindSymbolTool::new(self.policy.clone()));
        self.register(FindReferencesTool::new(self.policy.clone()));
        self.register(CodeDiagnosticsTool::new(self.policy.clone()));
        self.register(ReplaceSymbolBodyTool::new(self.policy.clone()));
        self.register(InsertBeforeSymbolTool::new(self.policy.clone()));
        self.register(InsertAfterSymbolTool::new(self.policy.clone()));
        self.register(RenameSymbolTool::new(self.policy.clone()));
        self.register(InitSessionTool::new(self.policy.clone()));
        self.register(MarkFeatureDoneTool::new(self.policy.clone()));
        self.register(AppendNoteTool::new(pr.clone()));
        self.register(HistoryOpsTool::new(pr));
    }
}

fn tool_call_advice(err: ToolCallInvalid) -> Value {
    match err {
        ToolCallInvalid::UnknownTool {
            tool_name,
            available_tools,
        } => json!({
            "error_code": "unknown_tool",
            "error_kind": "unknown_tool",
            "tool": tool_name,
            "message": "Requested tool is not registered. Use one of the available tool names.",
            "suggestions": suggest_tool_replacements(&tool_name, &available_tools),
            "available_tools": available_tools.into_iter().take(20).collect::<Vec<_>>(),
        }),
        ToolCallInvalid::InvalidSchema { tool_name, message } => {
            json!({
                "error_code": "invalid_schema",
                "error_kind": "invalid_schema",
                "tool": tool_name,
                "message": format!("Tool schema is invalid: {message}"),
            })
        }
        ToolCallInvalid::MalformedArguments {
            tool_name,
            raw_arguments_preview,
            example,
        } => json!({
            "error_code": "invalid_arguments",
            "error_kind": "malformed_arguments",
            "tool": tool_name,
            "message": "Tool arguments were not valid JSON. Emit one complete JSON object matching the schema before calling the tool again.",
            "raw_arguments_preview": raw_arguments_preview,
            "example": example,
        }),
        ToolCallInvalid::InvalidArguments {
            tool_name,
            problems,
            example,
        } => json!({
            "error_code": "invalid_arguments",
            "error_kind": "invalid_arguments",
            "tool": tool_name,
            "message": "Tool arguments do not match the JSON schema. Fix the arguments and call the tool again.",
            "problems": problems,
            "example": example,
        }),
    }
}

fn tool_call_advice_message(err: &ToolCallInvalid) -> String {
    match err {
        ToolCallInvalid::UnknownTool { tool_name, .. } => format!("unknown tool `{tool_name}`"),
        ToolCallInvalid::InvalidSchema { tool_name, message } => {
            format!("invalid schema for `{tool_name}`: {message}")
        }
        ToolCallInvalid::MalformedArguments { tool_name, .. } => {
            format!("malformed args for `{tool_name}`")
        }
        ToolCallInvalid::InvalidArguments {
            tool_name,
            problems,
            ..
        } => format!("invalid args for `{tool_name}`: {}", problems.join("; ")),
    }
}

fn model_friendly_definition(mut definition: ToolDefinition) -> ToolDefinition {
    definition.input_schema = simplify_schema_for_model(&definition.input_schema);
    definition
}

fn simplify_schema_for_model(schema: &Value) -> Value {
    match schema {
        Value::Object(object) => simplify_schema_object_for_model(object),
        Value::Array(values) => {
            Value::Array(values.iter().map(simplify_schema_for_model).collect())
        }
        value => value.clone(),
    }
}

fn simplify_schema_object_for_model(object: &Map<String, Value>) -> Value {
    let mut simplified = Map::new();

    for keyword in ["oneOf", "anyOf", "allOf"] {
        let Some(branches) = object.get(keyword).and_then(Value::as_array) else {
            continue;
        };
        if let Some(Value::Object(branch)) = branches.first().map(simplify_schema_for_model) {
            simplified.extend(branch);
        }
        break;
    }

    for (key, value) in object {
        if matches!(key.as_str(), "oneOf" | "anyOf" | "allOf" | "const") {
            continue;
        }
        simplified.insert(key.clone(), simplify_schema_for_model(value));
    }

    Value::Object(simplified)
}

fn suggest_tool_replacements(tool_name: &str, available_tools: &[String]) -> Vec<String> {
    let candidates: &[&str] = match tool_name {
        "glob" | "list_files" | "ls" | "find" => &["fs_browser", "grep"],
        "read" | "cat" => &["read_file"],
        "edit" | "patch" => &["apply_patch"],
        "write" => &["write_file"],
        "shell" | "run" | "terminal" => &["bash"],
        "search" | "rg" => &["grep"],
        _ => &[],
    };
    candidates
        .iter()
        .filter(|candidate| available_tools.iter().any(|tool| tool == **candidate))
        .map(|candidate| (*candidate).to_string())
        .collect()
}

pub fn example_from_schema(schema: &Value) -> Value {
    if let Some(ex) = schema
        .get("examples")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        return ex.clone();
    }
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return json!({});
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut ex = serde_json::Map::new();
    for field in required {
        let v = properties
            .get(field)
            .and_then(|p| p.get("type"))
            .and_then(Value::as_str)
            .map(|k| match k {
                "integer" => json!(1),
                "number" => json!(1.0),
                "boolean" => json!(true),
                "array" => json!([]),
                "object" => json!({}),
                _ => json!("example"),
            })
            .unwrap_or(json!("example"));
        ex.insert(field.to_string(), v);
    }
    Value::Object(ex)
}
