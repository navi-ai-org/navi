use crate::capability::{
    CapabilityDecision, CapabilityLedgerEntry, CapabilityScope, capabilities_from_tool_metadata,
};
use crate::effect::PostDecision;
use crate::event::AgentEvent;
use crate::runtime_components::{DefaultToolSecurityPolicy, ToolSecurityPolicy};
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
pub mod metadata;
pub mod registry;
#[cfg(test)]
mod tests;

use builtin::BranchRaceTool;
use builtin::{
    AppendNoteTool, BashTool, CodeExecTool, ContextRemainingTool, CurrentTimeTool, HistoryOpsTool,
    InitSessionTool, MarkFeatureDoneTool, MemoryTool, NewContextWindowTool, PackageManagerTool,
    PlanTool, ProcessTool, QuestionTool, ReadTool, RepoIntelligenceAction, RepoIntelligenceTool,
    RequestUserInputTool, RuntimeInfoTool, SandboxTool, SearchTool, SetGoalTool, SleepTool,
    ToolSearchTool, VerifierTool, ViewImageTool, WriteTool, builtin_metadata, truncate_tool_result,
};
#[cfg(feature = "browser")]
use builtin::BrowserTool;
#[cfg(feature = "code-vfs")]
use builtin::{CodeEditTool, CodeReadTool};

pub use builtin::{AgentProfile, ApprovalMode, ProviderBuilderFn, RepoExploreTool, SubagentTool};
pub use metadata::{ToolExposure, ToolMetadata, ToolRisk, capabilities};
pub use registry::{ToolRegistry, ToolSet, phases};

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
    /// When set, bash can request a sudo password without exposing it to the model.
    pub sudo_password_resolver: Option<crate::runtime::SudoPasswordResolver>,
    pub cancel_token: Option<crate::cancel::CancelToken>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub kind: ToolKind,
    #[serde(default)]
    pub input_schema: Value,
    /// Rich metadata for routing, policy, UI, traces, concurrency, and verifiers.
    /// Backward-compatible: defaults to empty/unspecified when not present.
    #[serde(default)]
    pub metadata: ToolMetadata,
}

impl Default for ToolDefinition {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            kind: ToolKind::Custom,
            input_schema: Value::Object(Default::default()),
            metadata: ToolMetadata::default(),
        }
    }
}

impl ToolDefinition {
    /// Creates a new tool definition with the given fields and default metadata.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        kind: ToolKind,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            kind,
            input_schema,
            metadata: ToolMetadata::default(),
        }
    }

    /// Creates a new tool definition with rich metadata.
    pub fn with_metadata(
        name: impl Into<String>,
        description: impl Into<String>,
        kind: ToolKind,
        input_schema: Value,
        metadata: ToolMetadata,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            kind,
            input_schema,
            metadata,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolKind {
    Read,
    Write,
    Command,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolParallelism {
    /// The tool can run concurrently with other shared tool calls.
    Shared,
    /// The tool needs exclusive execution within a model-emitted tool batch.
    Exclusive,
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
    security: Arc<dyn ToolSecurityPolicy>,
    harness_profile: String,
    registry: ToolRegistry,
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

const EXCLUSIVE_BATCH_TOOL_NAMES: &[&str] = &[
    "branch_race_start",
    "plan",
    "question",
    // Nested model turns: serialize so parallel spawn storms cannot hang the
    // parent batch or thrash provider quotas.
    "subagent",
    "repo_explore",
];

impl ToolExecutor {
    pub fn new(policy: SecurityPolicy) -> Self {
        Self::with_security_policy(policy, Arc::new(DefaultToolSecurityPolicy))
    }

    pub fn empty(policy: SecurityPolicy) -> Self {
        Self::empty_with_security_policy(policy, Arc::new(DefaultToolSecurityPolicy))
    }

    pub fn empty_with_security_policy(
        policy: SecurityPolicy,
        security: Arc<dyn ToolSecurityPolicy>,
    ) -> Self {
        Self {
            tools: HashMap::new(),
            validators: HashMap::new(),
            invalid_schemas: HashMap::new(),
            policy,
            security,
            harness_profile: "medium".to_string(),
            registry: ToolRegistry::new(),
        }
    }

    pub fn with_security_policy(
        policy: SecurityPolicy,
        security: Arc<dyn ToolSecurityPolicy>,
    ) -> Self {
        let mut executor = Self::empty_with_security_policy(policy, security);
        executor.register_builtin_tools();
        executor
    }

    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    pub fn registry_mut(&mut self) -> &mut ToolRegistry {
        &mut self.registry
    }

    /// Searches tools by keyword across name, description, tags, and capabilities.
    pub fn search_tools(&self, query: &str, max_results: usize) -> Vec<ToolDefinition> {
        self.registry.search(query, max_results)
    }

    pub fn set_harness_profile(&mut self, profile: String) {
        self.harness_profile = profile;
        self.register(RuntimeInfoTool::new(
            self.policy.clone(),
            self.harness_profile.clone(),
        ));
    }

    /// Replaces the security policy used for subsequent tool validations.
    pub fn set_security_policy(&mut self, policy: SecurityPolicy) {
        self.policy = policy;
        self.register(RuntimeInfoTool::new(
            self.policy.clone(),
            self.harness_profile.clone(),
        ));
    }

    /// Returns a reference to the active security policy.
    pub fn security_policy(&self) -> &SecurityPolicy {
        &self.policy
    }

    pub fn register_skill_loader(
        &mut self,
        project_dir: std::path::PathBuf,
        data_dir: std::path::PathBuf,
        config: std::sync::Arc<std::sync::RwLock<crate::config::NaviConfig>>,
    ) {
        // load_skill + skill store management tools (create-skill workflow).
        let loader = crate::tool::builtin::SkillTool::new(
            project_dir.clone(),
            data_dir.clone(),
            config.clone(),
        );
        self.register_tool(std::sync::Arc::new(loader));
        self.register_tool(std::sync::Arc::new(crate::tool::builtin::SkillListTool::new(
            project_dir.clone(),
            data_dir.clone(),
            config.clone(),
        )));
        self.register_tool(std::sync::Arc::new(crate::tool::builtin::SkillGetTool::new(
            project_dir.clone(),
            data_dir.clone(),
            config,
        )));
        self.register_tool(std::sync::Arc::new(
            crate::tool::builtin::SkillSaveTool::new(project_dir.clone(), data_dir.clone()),
        ));
        self.register_tool(std::sync::Arc::new(
            crate::tool::builtin::SkillDeleteTool::new(project_dir, data_dir),
        ));
    }

    pub(crate) fn new_code_exec_host(policy: SecurityPolicy) -> Self {
        let pr = policy.project_root().to_path_buf();
        let mut executor = Self {
            tools: HashMap::new(),
            validators: HashMap::new(),
            invalid_schemas: HashMap::new(),
            policy: policy.clone(),
            security: Arc::new(DefaultToolSecurityPolicy),
            harness_profile: "medium".to_string(),
            registry: ToolRegistry::new(),
        };
        executor.register(ReadTool::new(pr.clone()));
        executor.register(ReadTool::alias(pr.clone(), "read_file"));
        executor.register(SearchTool::new(pr.clone()));
        executor.register(SearchTool::grep(pr.clone()));
        executor.register(SearchTool::fs_browser(pr.clone()));
        executor.register(WriteTool::apply_patch(pr.clone()));
        executor.register(VerifierTool::new(pr));
        executor.register(RepoIntelligenceTool::new(
            policy.clone(),
            RepoIntelligenceAction::AstSearch,
        ));
        executor.register(RepoIntelligenceTool::new(
            policy,
            RepoIntelligenceAction::TestDiscovery,
        ));
        executor
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        // Use registry exposure info to filter, but get definitions from live tools.
        // Merge in the enriched metadata from the registry so that schema
        // simplification is applied and MCP/plugin tools remain current while
        // respecting exposure levels.
        let visible_names: std::collections::HashSet<String> =
            self.registry.visible_tool_names().into_iter().collect();

        let mut result: Vec<ToolDefinition> = self
            .tools
            .values()
            .filter(|tool| {
                let def = tool.definition();
                visible_names.contains(&def.name)
            })
            .map(|tool| {
                let mut def = model_friendly_definition(tool.definition());
                // Merge enriched metadata from the registry
                if let Some(registered) = self.registry.get(&def.name) {
                    def.metadata = registered.definition.metadata.clone();
                }
                def
            })
            .collect();
        result.sort_by(|a, b| a.name.cmp(&b.name));
        result
    }

    pub fn all_definitions(&self) -> Vec<ToolDefinition> {
        let mut result = self
            .tools
            .values()
            .map(|tool| model_friendly_definition(self.enriched_definition(tool.as_ref())))
            .collect::<Vec<_>>();
        result.sort_by(|a, b| a.name.cmp(&b.name));
        result
    }

    pub fn definition(&self, name: &str) -> Option<ToolDefinition> {
        self.tools
            .get(name)
            .map(|tool| self.enriched_definition(tool.as_ref()))
    }

    pub fn parallelism_for(&self, tool_name: &str) -> ToolParallelism {
        if EXCLUSIVE_BATCH_TOOL_NAMES.contains(&tool_name) {
            return ToolParallelism::Exclusive;
        }

        let Some(definition) = self.definition(tool_name) else {
            return ToolParallelism::Shared;
        };

        if definition.metadata.is_read_only && definition.metadata.is_concurrency_safe {
            ToolParallelism::Shared
        } else {
            ToolParallelism::Exclusive
        }
    }

    pub fn register_tool(&mut self, tool: Arc<dyn Tool>) -> Option<Arc<dyn Tool>> {
        let mut def = tool.definition();
        let name = def.name.clone();

        // Inject builtin metadata (enriches tool definitions without changing each tool struct)
        if def.metadata.is_default() {
            let builtin = builtin_metadata(&name, def.kind);
            def.metadata = builtin;
        }

        // Register in the tool registry for search/discovery
        self.registry.register(def.clone());

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
        self.registry.unregister_prefix("plugin__");
    }

    pub fn invalid_tool_result(&self, inv: &ToolInvocation, err: ToolCallInvalid) -> ToolResult {
        ToolResult {
            invocation_id: inv.id.clone(),
            ok: false,
            output: tool_call_advice(err),
        }
    }

    pub fn validate(&self, inv: &ToolInvocation) -> SecurityDecision {
        if let Err(e) = self.validate_arguments(inv) {
            return SecurityDecision::Deny(tool_call_advice_message(&e));
        }
        let Some(def) = self.definition(&inv.tool_name) else {
            return SecurityDecision::Deny(format!("unknown `{}`", inv.tool_name));
        };
        self.security.validate_tool(&self.policy, &def, inv)
    }

    pub async fn invoke(&self, invocation: ToolInvocation) -> ToolResult {
        self.invoke_with_event_tx(invocation, None).await
    }

    pub async fn invoke_with_event_tx(
        &self,
        invocation: ToolInvocation,
        event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> ToolResult {
        self.invoke_with_context_inner(
            invocation,
            ToolInvocationContext {
                event_tx,
                ..Default::default()
            },
            false,
        )
        .await
    }

    pub async fn invoke_approved_with_event_tx(
        &self,
        invocation: ToolInvocation,
        event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> ToolResult {
        self.invoke_with_context_inner(
            invocation,
            ToolInvocationContext {
                event_tx,
                ..Default::default()
            },
            true,
        )
        .await
    }

    pub async fn invoke_with_full_context(
        &self,
        invocation: ToolInvocation,
        context: ToolInvocationContext,
        approval_granted: bool,
    ) -> ToolResult {
        self.invoke_with_context_inner(invocation, context, approval_granted)
            .await
    }

    async fn invoke_with_context_inner(
        &self,
        invocation: ToolInvocation,
        context: ToolInvocationContext,
        approval_granted: bool,
    ) -> ToolResult {
        let event_tx = context.event_tx.clone();
        let inv_id = invocation.id.clone();
        let started = std::time::Instant::now();
        let invocation = self.policy.normalize_invocation_paths(&invocation);
        // Weak models sometimes emit a path or bare action as the tool name.
        // Recover high-confidence mistakes before validation so analysis turns
        // don't die on three consecutive unknown tools.
        let invocation =
            recover_misnamed_tool_invocation(&invocation, |name| self.definition(name).is_some())
                .unwrap_or(invocation);
        let tool_name = invocation.tool_name.clone();

        // Determine tool kind and metadata before consuming the invocation.
        let tool_def = self.definition(&invocation.tool_name);
        let tool_kind = tool_def.as_ref().map(|d| d.kind);
        let tool_verifier_hint = tool_def
            .as_ref()
            .and_then(|d| d.metadata.verifier.as_deref())
            .map(|v| v.to_string());
        let capability_event_tx = event_tx.clone();
        emit_capability_events(
            capability_event_tx.as_ref(),
            &invocation,
            tool_def.as_ref(),
            CapabilityDecision::Requested,
            "tool invocation requested",
        );

        if let Err(e) = self.validate_arguments(&invocation) {
            return self.invalid_tool_result(&invocation, e);
        }
        match self.validate(&invocation) {
            SecurityDecision::Allow => {}
            SecurityDecision::NeedsApproval(risk) if approval_granted => {
                tracing::debug!(tool = %invocation.tool_name, ?risk, "tool approval already granted");
            }
            SecurityDecision::NeedsApproval(risk) => {
                let message = format!(
                    "approval required for tool `{}`: {:?}",
                    invocation.tool_name, risk
                );
                emit_capability_events(
                    capability_event_tx.as_ref(),
                    &invocation,
                    tool_def.as_ref(),
                    CapabilityDecision::Denied,
                    &message,
                );
                return ToolResult {
                    invocation_id: inv_id,
                    ok: false,
                    output: json!({"error": message}),
                };
            }
            SecurityDecision::Deny(r) => {
                emit_capability_events(
                    capability_event_tx.as_ref(),
                    &invocation,
                    tool_def.as_ref(),
                    CapabilityDecision::Denied,
                    &r,
                );
                return ToolResult {
                    invocation_id: inv_id,
                    ok: false,
                    output: json!({"error": r}),
                };
            }
        }
        let Some(tool) = self.tools.get(&invocation.tool_name).cloned() else {
            return ToolResult {
                invocation_id: inv_id,
                ok: false,
                output: json!({"error": format!("unknown `{}`", invocation.tool_name)}),
            };
        };
        if invocation.tool_name == "tool_search" {
            return self.invoke_tool_search(invocation);
        }

        let pre_execution_snapshot = if tool_kind == Some(crate::tool::ToolKind::Write) {
            let paths = self.snapshot_paths_for_invocation(&invocation);
            if paths.is_empty() {
                None
            } else {
                Some(crate::sandbox::SandboxManager::create_snapshot(&paths))
            }
        } else {
            None
        };

        // Snapshot the invocation input for post-execution effect analysis
        // before it is consumed by invoke_with_context.
        let inv_input = invocation.input.clone();

        let mut result = match tool.invoke_with_context(invocation, context).await {
            Ok(r) => truncate_tool_result(r),
            Err(e) => ToolResult {
                invocation_id: inv_id.clone(),
                ok: false,
                output: json!({"error": format!("{e:#}")}),
            },
        };

        // Post-execution effect check for successful write/command tools.
        if result.ok {
            let should_check = match tool_kind {
                Some(crate::tool::ToolKind::Write) => true,
                Some(crate::tool::ToolKind::Command) => true,
                _ => false,
            };

            if should_check {
                let paths = crate::effect::extract_paths(
                    &result,
                    &ToolInvocation {
                        id: inv_id.clone(),
                        tool_name: tool_name.clone(),
                        input: inv_input,
                    },
                );

                if !paths.is_empty() {
                    let command = None;
                    let decision = self
                        .policy
                        .post_execution_effect_check(&tool_name, &paths, command);

                    match decision {
                        PostDecision::Allow => {
                            // No action needed; result stands.
                        }
                        PostDecision::Ask(reason) => {
                            tracing::warn!(
                                tool = %tool_name,
                                reason = %reason,
                                "post-execution effect check: ask user"
                            );
                            if let Value::Object(ref mut map) = result.output {
                                map.insert(
                                    "effect_warning".to_string(),
                                    json!({
                                        "decision": "ask",
                                        "message": reason,
                                    }),
                                );
                            }
                        }
                        PostDecision::Deny(reason) => {
                            tracing::warn!(
                                tool = %tool_name,
                                reason = %reason,
                                "post-execution effect check: denied"
                            );
                            let rollback = pre_execution_snapshot
                                .as_ref()
                                .map(crate::sandbox::SandboxManager::rollback);
                            let (rolled_back, rollback_error) = rollback_outcome(rollback);
                            emit_capability_events(
                                capability_event_tx.as_ref(),
                                &ToolInvocation {
                                    id: inv_id.clone(),
                                    tool_name: tool_name.clone(),
                                    input: json!({}),
                                },
                                tool_def.as_ref(),
                                CapabilityDecision::Violated,
                                &reason,
                            );
                            return ToolResult {
                                invocation_id: inv_id,
                                ok: false,
                                output: json!({
                                    "error": reason,
                                    "error_code": "effect_denied",
                                    "rolled_back": rolled_back,
                                    "rollback_error": rollback_error,
                                }),
                            };
                        }
                        PostDecision::Rollback(reason) => {
                            tracing::warn!(
                                tool = %tool_name,
                                reason = %reason,
                                "post-execution effect check: rollback recommended"
                            );
                            let rollback = pre_execution_snapshot
                                .as_ref()
                                .map(crate::sandbox::SandboxManager::rollback);
                            let (rolled_back, rollback_error) = rollback_outcome(rollback);
                            emit_capability_events(
                                capability_event_tx.as_ref(),
                                &ToolInvocation {
                                    id: inv_id.clone(),
                                    tool_name: tool_name.clone(),
                                    input: json!({}),
                                },
                                tool_def.as_ref(),
                                CapabilityDecision::Violated,
                                &reason,
                            );
                            return ToolResult {
                                invocation_id: inv_id,
                                ok: false,
                                output: json!({
                                    "error": reason,
                                    "error_code": "effect_rollback",
                                    "rolled_back": rolled_back,
                                    "rollback_error": rollback_error,
                                }),
                            };
                        }
                    }
                }
            }
        }

        emit_capability_events(
            capability_event_tx.as_ref(),
            &ToolInvocation {
                id: inv_id.clone(),
                tool_name: tool_name.clone(),
                input: json!({}),
            },
            tool_def.as_ref(),
            if result.ok {
                CapabilityDecision::Consumed
            } else {
                CapabilityDecision::Violated
            },
            if result.ok {
                "tool invocation completed"
            } else {
                "tool invocation failed"
            },
        );

        // Post-execution verifier hint injection: if the tool metadata advertises
        // a verifier hint, surface it in the result so the harness (and model)
        // can suggest or run verification after mutation tools.
        if result.ok && tool_kind == Some(crate::tool::ToolKind::Write) {
            if let Some(verifier_cmd) = tool_verifier_hint {
                if let Value::Object(ref mut map) = result.output {
                    map.insert(
                        "verifier_hint".to_string(),
                        json!({
                            "suggested": true,
                            "command": verifier_cmd,
                            "message": format!(
                                "After writing, verify with: verifier(action='run', verifier='command', command='{}')",
                                verifier_cmd
                            ),
                        }),
                    );
                }
            }
        }

        tracing::info!(tool = %tool_name, ok = result.ok, dur_ms = started.elapsed().as_millis() as u64, "invoke finished");
        result
    }

    fn invoke_tool_search(&self, invocation: ToolInvocation) -> ToolResult {
        let query = invocation
            .input
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let max_results = invocation
            .input
            .get("max_results")
            .and_then(Value::as_u64)
            .unwrap_or(10)
            .min(50) as usize;
        let results = self.registry.search(&query, max_results);
        let results = results
            .into_iter()
            .map(model_friendly_definition)
            .map(|def| {
                json!({
                    "name": def.name,
                    "description": def.description,
                    "kind": def.kind,
                    "metadata": def.metadata,
                    "input_schema": def.input_schema,
                })
            })
            .collect::<Vec<_>>();

        ToolResult {
            invocation_id: invocation.id,
            ok: true,
            output: json!({
                "query": query,
                "results": results,
                "total": results.len(),
                "hint": if results.is_empty() {
                    "No tools found. Try a different query."
                } else {
                    "Call a returned tool by its `name` with arguments matching `input_schema`."
                },
            }),
        }
    }

    fn snapshot_paths_for_invocation(
        &self,
        invocation: &ToolInvocation,
    ) -> Vec<std::path::PathBuf> {
        let mut paths = Vec::new();
        for key in ["path", "file"] {
            if let Some(path) = invocation.input.get(key).and_then(Value::as_str) {
                push_unique_snapshot_path(
                    &mut paths,
                    self.policy.resolve_project_path(Path::new(path)),
                );
            }
        }

        if let Some(patch) = invocation.input.get("patch").and_then(Value::as_str) {
            for path in crate::security::extract_apply_patch_paths(patch) {
                push_unique_snapshot_path(
                    &mut paths,
                    self.policy.resolve_project_path(Path::new(&path)),
                );
            }
        }
        if let Some(patches) = invocation.input.get("patches").and_then(Value::as_array) {
            for patch in patches.iter().filter_map(Value::as_str) {
                for path in crate::security::extract_apply_patch_paths(patch) {
                    push_unique_snapshot_path(
                        &mut paths,
                        self.policy.resolve_project_path(Path::new(&path)),
                    );
                }
            }
        }

        paths
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

    fn enriched_definition(&self, tool: &dyn Tool) -> ToolDefinition {
        let mut def = tool.definition();
        if let Some(registered) = self.registry.get(&def.name) {
            def.metadata = registered.definition.metadata.clone();
        }
        def
    }

    fn register_builtin_tools(&mut self) {
        let pr = self.policy.project_root().to_path_buf();
        self.register(ReadTool::new(pr.clone()));
        self.register(ReadTool::alias(pr.clone(), "read_file"));
        self.register(SearchTool::new(pr.clone()));
        self.register(SearchTool::grep(pr.clone()));
        self.register(SearchTool::fs_browser(pr.clone()));
        self.register(SearchTool::list_dir(pr.clone()));
        self.register(SearchTool::glob(pr.clone()));
        self.register(WriteTool::new(pr.clone()));
        self.register(WriteTool::write_file(pr.clone()));
        self.register(WriteTool::apply_patch(pr.clone()));
        self.register(BashTool::new(pr.clone()));
        self.register(ProcessTool::new(pr.clone()));
        self.register(QuestionTool);
        self.register(PlanTool::new(self.policy.clone()));
        self.register(PackageManagerTool::new(pr.clone()));
        self.register(RuntimeInfoTool::new(
            self.policy.clone(),
            self.harness_profile.clone(),
        ));
        #[cfg(feature = "code-vfs")]
        {
            self.register(CodeReadTool::new(self.policy.clone()));
            self.register(CodeEditTool::new(self.policy.clone()));
        }
        self.register(CodeExecTool::new(self.policy.clone()));
        self.register(RepoIntelligenceTool::new(
            self.policy.clone(),
            RepoIntelligenceAction::AstSearch,
        ));
        self.register(RepoIntelligenceTool::new(
            self.policy.clone(),
            RepoIntelligenceAction::SymbolGoto,
        ));
        self.register(RepoIntelligenceTool::new(
            self.policy.clone(),
            RepoIntelligenceAction::SymbolReferences,
        ));
        self.register(RepoIntelligenceTool::new(
            self.policy.clone(),
            RepoIntelligenceAction::DependencyGraph,
        ));
        self.register(RepoIntelligenceTool::new(
            self.policy.clone(),
            RepoIntelligenceAction::TestDiscovery,
        ));
        self.register(RepoIntelligenceTool::new(
            self.policy.clone(),
            RepoIntelligenceAction::OwnershipChurn,
        ));
        self.register(BranchRaceTool);
        self.register(InitSessionTool::new(self.policy.clone()));
        self.register(MarkFeatureDoneTool::new(self.policy.clone()));
        self.register(AppendNoteTool::new(pr.clone()));
        self.register(MemoryTool::new(pr.clone()));
        self.register(HistoryOpsTool::new(pr.clone()));
        self.register(CurrentTimeTool::new());
        self.register(SleepTool::new());
        self.register(SetGoalTool);
        self.register(ContextRemainingTool::new(pr.clone()));
        self.register(RequestUserInputTool::new());
        self.register(SandboxTool::new(pr.clone()));
        self.register(ViewImageTool::new(pr.clone()));
        self.register(ViewImageTool::inspect_image(pr.clone()));
        #[cfg(feature = "browser")]
        self.register(BrowserTool::new(pr.clone()));
        self.register(NewContextWindowTool::new());
        self.register(ToolSearchTool::new(Arc::new(self.registry.clone())));
        self.register(VerifierTool::new(pr));
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
    let lower = tool_name.trim().to_ascii_lowercase();
    let candidates: &[&str] = match lower.as_str() {
        "list_files" | "ls" | "listdir" | "dir" | "list" | "list_dir" => {
            &["list_dir", "fs_browser", "search", "glob"]
        }
        "find" | "find_files" => &["glob", "fs_browser", "search"],
        "cat" | "type" | "open" | "view_file" | "read" | "view" => &["read_file", "read"],
        "edit" | "patch" | "str_replace" | "search_replace" => {
            &["write", "apply_patch", "write_file"]
        }
        "shell" | "run" | "terminal" | "exec" | "sh" | "cmd" => &["bash", "process"],
        "rg" | "ripgrep" | "search_code" => &["grep", "search", "ast_search"],
        "symbols" | "symbol" | "symbols_overview" | "find_symbol" | "find_references"
        | "code_diagnostics" => &["code", "ast_search", "symbol_goto"],
        "replace_symbol_body"
        | "insert_before_symbol"
        | "insert_after_symbol"
        | "rename_symbol" => &["code_edit"],
        _ if looks_like_filesystem_path(tool_name) => &["read_file", "fs_browser", "grep", "bash"],
        _ => &[],
    };
    candidates
        .iter()
        .filter(|candidate| available_tools.iter().any(|tool| tool == **candidate))
        .map(|candidate| (*candidate).to_string())
        .collect()
}

/// Heuristic: weak models often put a path (or `.`) in the tool *name* field
/// instead of using `read_file` / `fs_browser`. Rewrite only high-confidence cases.
fn recover_misnamed_tool_invocation(
    inv: &ToolInvocation,
    has_tool: impl Fn(&str) -> bool,
) -> Option<ToolInvocation> {
    if has_tool(&inv.tool_name) {
        return None;
    }

    let name = inv.tool_name.trim();
    if name.is_empty() {
        return None;
    }

    let input = inv.input.as_object();
    let action = input
        .and_then(|obj| obj.get("action"))
        .and_then(Value::as_str)
        .map(|s| s.to_ascii_lowercase());
    let has_path_field = input
        .and_then(|obj| obj.get("path"))
        .and_then(Value::as_str)
        .is_some_and(|p| !p.is_empty());
    let path_from_input = input
        .and_then(|obj| obj.get("path"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let path_like_name = looks_like_filesystem_path(name);

    // `{ "name": ".", "arguments": { "action": "list", "path": "." } }` → fs_browser
    // also covers list/tree/find/stat with a path-like tool name.
    if matches!(
        action.as_deref(),
        Some("list" | "tree" | "find" | "stat" | "read" | "search")
    ) && has_tool("fs_browser")
    {
        let mut recovered = inv.clone();
        recovered.tool_name = "fs_browser".to_string();
        if !has_path_field && path_like_name {
            if let Some(obj) = recovered.input.as_object_mut() {
                obj.insert("path".to_string(), Value::String(name.to_string()));
            }
        }
        tracing::info!(
            from = %inv.tool_name,
            to = "fs_browser",
            "recovered misnamed tool invocation"
        );
        return Some(recovered);
    }

    // `{ "name": "IDEA.md", "arguments": { "path": "IDEA.md" } }` → read_file
    // `{ "name": "src/main.rs", "arguments": {} }` → read_file
    if path_like_name && has_tool("read_file") {
        let keys: Vec<&str> = input
            .map(|obj| obj.keys().map(String::as_str).collect())
            .unwrap_or_default();
        let readish = keys.is_empty()
            || keys.iter().all(|k| {
                matches!(
                    *k,
                    "path"
                        | "offset"
                        | "limit"
                        | "start_line"
                        | "end_line"
                        | "max_bytes"
                        | "encoding"
                )
            });
        // Don't rewrite obvious write/patch payloads.
        let writeish = keys
            .iter()
            .any(|k| matches!(*k, "content" | "patch" | "patches" | "edits" | "new_string"));
        if readish && !writeish {
            let mut recovered = inv.clone();
            recovered.tool_name = "read_file".to_string();
            if !has_path_field {
                let mut obj = serde_json::Map::new();
                obj.insert("path".to_string(), Value::String(name.to_string()));
                if let Some(input_obj) = input {
                    for (k, v) in input_obj {
                        if k != "path" {
                            obj.insert(k.clone(), v.clone());
                        }
                    }
                }
                recovered.input = Value::Object(obj);
            } else if path_from_input.as_deref() != Some(name)
                && path_from_input
                    .as_deref()
                    .is_some_and(|p| p == "." || p.is_empty())
            {
                // Prefer the path-like tool name when the path field is a placeholder.
                if let Some(obj) = recovered.input.as_object_mut() {
                    obj.insert("path".to_string(), Value::String(name.to_string()));
                }
            }
            tracing::info!(
                from = %inv.tool_name,
                to = "read_file",
                "recovered misnamed tool invocation"
            );
            return Some(recovered);
        }
    }

    None
}

fn looks_like_filesystem_path(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return false;
    }
    if name == "." || name == ".." {
        return true;
    }
    if name.contains('/') || name.contains('\\') {
        return true;
    }
    // Bare filenames with extensions: IDEA.md, main.rs, package.json
    if let Some((_, ext)) = name.rsplit_once('.') {
        let ext = ext.trim();
        if !ext.is_empty()
            && ext.len() <= 12
            && ext
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-')
            && !name.starts_with('.')
            // Avoid treating dotted tool ids like `chrome.devtools` without path separators
            // only when they look like real extensions (mostly lowercase 1–8 chars).
            && (ext.len() <= 8)
        {
            return true;
        }
    }
    false
}

fn push_unique_snapshot_path(paths: &mut Vec<std::path::PathBuf>, path: std::path::PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

fn rollback_outcome(rollback: Option<std::result::Result<(), String>>) -> (bool, Option<String>) {
    match rollback {
        Some(Ok(())) => (true, None),
        Some(Err(error)) => (false, Some(error)),
        None => (false, None),
    }
}

fn emit_capability_events(
    event_tx: Option<&mpsc::UnboundedSender<AgentEvent>>,
    invocation: &ToolInvocation,
    definition: Option<&ToolDefinition>,
    decision: CapabilityDecision,
    justification: &str,
) {
    let Some(event_tx) = event_tx else {
        return;
    };
    let Some(definition) = definition else {
        return;
    };
    for capability in capabilities_from_tool_metadata(&definition.metadata.capabilities) {
        let _ = event_tx.send(AgentEvent::CapabilityRecorded(CapabilityLedgerEntry {
            capability,
            scope: CapabilityScope::SingleCall(invocation.id.clone()),
            decision: decision.clone(),
            at_ms: tool_unix_millis(),
            justification: format!("{}: {justification}", invocation.tool_name),
        }));
    }
}

fn tool_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
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
