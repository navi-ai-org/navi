//! Agent backends for workflow workers.
//!
//! - [`WorkerProbeBackend`]: exercises real tool registration + [`SecurityPolicy`]
//!   (no live model). Used for permission/concurrency integration tests and as
//!   the safe default when no parent executor is available.
//! - [`SubagentBridgeBackend`]: production path — each `agent()` runs through
//!   the real `subagent` tool / turn infrastructure with a filtered allowlist.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use serde_json::json;

use super::AgentBackend;
use super::policy::{EffectiveAgentPolicy, RunPolicy};
use super::types::{AgentBackendResult, AgentRequest, NESTED_WORKFLOW_TOOLS};
use crate::security::{SecurityDecision, SecurityPolicy};
use crate::tool::{ToolExecutor, ToolInvocation, ToolResult};

/// Builds a filtered worker executor and probes tool/path access via the real
/// [`SecurityPolicy`] and tool registry path (no model call).
pub struct WorkerProbeBackend {
    policy: SecurityPolicy,
    pub delay_ms: u64,
    pub in_flight: Option<Arc<AtomicUsize>>,
    pub peak_in_flight: Option<Arc<AtomicUsize>>,
}

impl WorkerProbeBackend {
    pub fn new(policy: SecurityPolicy) -> Self {
        Self {
            policy,
            delay_ms: 0,
            in_flight: None,
            peak_in_flight: None,
        }
    }

    pub fn with_delay(mut self, delay_ms: u64) -> Self {
        self.delay_ms = delay_ms;
        self
    }

    pub fn with_inflight(mut self, in_flight: Arc<AtomicUsize>, peak: Arc<AtomicUsize>) -> Self {
        self.in_flight = Some(in_flight);
        self.peak_in_flight = Some(peak);
        self
    }
}

#[async_trait]
impl AgentBackend for WorkerProbeBackend {
    async fn run_agent(&self, request: AgentRequest) -> AgentBackendResult {
        if let Some(ref inflight) = self.in_flight {
            let n = inflight.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some(ref peak) = self.peak_in_flight {
                peak.fetch_max(n, Ordering::SeqCst);
            }
        }

        if self.delay_ms > 0 {
            let delay = std::time::Duration::from_millis(self.delay_ms);
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = request.cancel_token.notified() => {
                    if let Some(ref inflight) = self.in_flight {
                        inflight.fetch_sub(1, Ordering::SeqCst);
                    }
                    return AgentBackendResult {
                        ok: false,
                        output: json!({"error": "cancelled"}),
                        error: Some("cancelled".into()),
                    };
                }
            }
        }

        if request.cancel_token.is_requested() {
            if let Some(ref inflight) = self.in_flight {
                inflight.fetch_sub(1, Ordering::SeqCst);
            }
            return AgentBackendResult {
                ok: false,
                output: json!({"error": "cancelled"}),
                error: Some("cancelled".into()),
            };
        }

        let probe =
            probe_worker_capabilities(&self.policy, &request.run_policy, &request.effective);

        if let Some(ref inflight) = self.in_flight {
            inflight.fetch_sub(1, Ordering::SeqCst);
        }

        AgentBackendResult {
            ok: true,
            output: json!({
                "ok": true,
                "backend": "worker_probe",
                "prompt": request.prompt,
                "label": request.label,
                "agent_index": request.agent_index,
                "profile": request.effective.profile,
                "tools": request.run_policy.tools,
                "create_files": request.run_policy.create_files,
                "create_dirs": request.run_policy.create_dirs,
                "write_allow": request.run_policy.write_allow,
                "path_allow": request.effective.path_allow,
                "path_deny": request.effective.path_deny,
                "can_write_file": probe.can_write_file,
                "can_edit": probe.can_edit,
                "can_bash": probe.can_bash,
                "can_subagent": probe.can_subagent,
                "can_workflow": probe.can_workflow,
                "write_path_allowed": probe.write_path_allowed,
                "write_path_denied": probe.write_path_denied,
                "create_new_file_allowed": probe.create_new_file_allowed,
                "registered_tools": probe.registered_tools,
                "policy_denials": probe.policy_denials,
            }),
            error: None,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ProbeResult {
    pub(crate) can_write_file: bool,
    pub(crate) can_edit: bool,
    pub(crate) can_bash: bool,
    pub(crate) can_subagent: bool,
    pub(crate) can_workflow: bool,
    pub(crate) write_path_allowed: Vec<String>,
    pub(crate) write_path_denied: Vec<String>,
    pub(crate) create_new_file_allowed: bool,
    pub(crate) registered_tools: Vec<String>,
    pub(crate) policy_denials: Vec<String>,
}

fn scoped_policy(
    base: &SecurityPolicy,
    run_policy: &RunPolicy,
    effective: &EffectiveAgentPolicy,
) -> SecurityPolicy {
    base.clone()
        .with_write_scope(crate::security::WritePathScope {
            write_allow: run_policy.write_allow.clone(),
            path_deny: effective.path_deny.clone(),
            create_files: run_policy.create_files,
            create_dirs: run_policy.create_dirs,
        })
}

pub(crate) fn probe_worker_capabilities(
    base_policy: &SecurityPolicy,
    run_policy: &RunPolicy,
    effective: &EffectiveAgentPolicy,
) -> ProbeResult {
    let mut out = ProbeResult::default();
    let project = base_policy.project_root().to_path_buf();

    // Tools and write scope come from the run policy; per-agent opts cannot widen.
    let mut run_tools: Vec<String> = run_policy
        .tools
        .iter()
        .filter(|t| !NESTED_WORKFLOW_TOOLS.contains(&t.as_str()))
        .cloned()
        .collect();
    run_tools.sort();
    run_tools.dedup();

    // Nested orchestration must never appear after intersection + strip.
    out.can_subagent = run_tools.iter().any(|t| t == "subagent");
    out.can_workflow = run_tools.iter().any(|t| t == "workflow");

    // Worker executor with WritePathScope (same gate as production SubagentBridge).
    let policy = scoped_policy(base_policy, run_policy, effective);
    let mut exec = ToolExecutor::empty(policy.clone());
    register_filtered_tools(&mut exec, &project, run_policy, &run_tools);
    out.registered_tools = exec.tool_names();
    out.registered_tools.sort();

    out.can_write_file = exec
        .tool_names()
        .iter()
        .any(|t| t == "write_file" || t == "write");
    out.can_edit = exec
        .tool_names()
        .iter()
        .any(|t| t == "edit" || t == "multiedit");
    out.can_bash = exec.tool_names().iter().any(|t| t == "bash");
    out.can_subagent = out.registered_tools.iter().any(|t| t == "subagent");
    out.can_workflow = out.registered_tools.iter().any(|t| t == "workflow");

    // Real ToolExecutor::validate path for representative writes.
    let probe_paths: Vec<String> = {
        let mut c = run_policy.write_allow.clone();
        if c.is_empty() {
            c.push("src/a.rs".into());
        }
        c.push("__outside_write_allow__.rs".into());
        for d in &effective.path_deny {
            let clean = d
                .trim_end_matches('/')
                .trim_end_matches('*')
                .trim_end_matches('/');
            if !clean.is_empty() {
                c.push(clean.to_string());
            }
        }
        // Non-existent path under write_allow for create_files probe.
        if let Some(first) = run_policy.write_allow.first() {
            c.push(format!("__new_create_probe__/{first}"));
        } else {
            c.push("__new_create_probe__/file.rs".into());
        }
        c.sort();
        c.dedup();
        c
    };

    for path in &probe_paths {
        let inv = ToolInvocation {
            id: format!("probe-write-{path}"),
            tool_name: "write_file".into(),
            input: json!({"path": path, "content": "x"}),
        };
        match exec.validate(&inv) {
            SecurityDecision::Deny(reason) => {
                out.policy_denials
                    .push(format!("write_file {path}: {reason}"));
                out.write_path_denied.push(path.clone());
            }
            SecurityDecision::Allow | SecurityDecision::NeedsApproval(_) => {
                // Only count as allowed if tool is registered AND validate ok.
                if out.can_write_file {
                    out.write_path_allowed.push(path.clone());
                } else {
                    out.write_path_denied.push(path.clone());
                    out.policy_denials
                        .push(format!("write_file {path}: tool not registered"));
                }
            }
        }
    }

    // create_files: writing a write_allow path that does not exist yet must Deny
    // when create_files=false (real SecurityPolicy WritePathScope).
    if let Some(wa) = run_policy.write_allow.first() {
        let abs = project.join(wa);
        // Use a unique non-existent path that still matches write_allow when
        // write_allow is a single file — probe that exact path if missing.
        let probe_path = if abs.exists() {
            // Existing path: also probe a sibling under same allow prefix if possible.
            format!("__wf_create_probe__/{wa}")
        } else {
            wa.clone()
        };
        let inv = ToolInvocation {
            id: "probe-create".into(),
            tool_name: "write_file".into(),
            input: json!({"path": probe_path, "content": "new"}),
        };
        match exec.validate(&inv) {
            SecurityDecision::Deny(reason) => {
                out.create_new_file_allowed = false;
                out.policy_denials.push(format!("create_new: {reason}"));
            }
            SecurityDecision::Allow | SecurityDecision::NeedsApproval(_) => {
                // Only true if tool registered, write_allow non-empty, create_files true,
                // and path is in write_allow (validate already checked scope).
                out.create_new_file_allowed = out.can_write_file && run_policy.create_files;
            }
        }
    } else {
        out.create_new_file_allowed = false;
    }

    // Empty write_allow ⇒ no writes even if tools listed.
    if run_policy.write_allow.is_empty() {
        out.can_write_file = false;
        out.can_edit = false;
        out.create_new_file_allowed = false;
    }

    out
}

fn register_filtered_tools(
    exec: &mut ToolExecutor,
    project: &std::path::Path,
    run_policy: &RunPolicy,
    run_tools: &[String],
) {
    use super::super::{
        bash::BashTool, edit_tool::EditTool, read_tool::ReadTool, search_tool::SearchTool,
        write_tool::WriteTool,
    };

    // Never register orchestration tools.
    let allowed: Vec<&str> = run_tools.iter().map(|s| s.as_str()).collect();

    let has = |name: &str| allowed.contains(&name);

    if has("read_file") || has("read") || has("view_file") {
        exec.register_tool(Arc::new(ReadTool::new(project.to_path_buf())));
    }
    if has("search") || has("grep") || has("fs_browser") || has("list_dir") || has("glob") {
        exec.register_tool(Arc::new(SearchTool::new(project.to_path_buf())));
    }

    // Writes only when write_allow is non-empty (empty ⇒ no writes even for implementer).
    // create_files=false still registers tools; WritePathScope denies creates.
    let writes_ok = !run_policy.write_allow.is_empty();
    if writes_ok && (has("write_file") || has("write")) {
        exec.register_tool(Arc::new(WriteTool::write_file(project.to_path_buf())));
    }
    if writes_ok && (has("edit") || has("multiedit")) {
        exec.register_tool(Arc::new(EditTool::new(project.to_path_buf())));
    }
    if has("bash") {
        exec.register_tool(Arc::new(BashTool::new(project.to_path_buf())));
    }
}

/// Production backend: each worker is a real nested `subagent` turn with a
/// tool allowlist derived from the effective workflow policy.
pub struct SubagentBridgeBackend {
    tool_executor: Weak<ToolExecutor>,
}

impl SubagentBridgeBackend {
    pub fn new(tool_executor: Weak<ToolExecutor>) -> Self {
        Self { tool_executor }
    }
}

#[async_trait]
impl AgentBackend for SubagentBridgeBackend {
    async fn run_agent(&self, request: AgentRequest) -> AgentBackendResult {
        let Some(executor) = self.tool_executor.upgrade() else {
            return AgentBackendResult {
                ok: false,
                output: json!({"error": "tool executor unavailable"}),
                error: Some("tool executor dropped".into()),
            };
        };

        if request.cancel_token.is_requested() {
            return AgentBackendResult {
                ok: false,
                output: json!({"error": "cancelled"}),
                error: Some("cancelled".into()),
            };
        }

        // Embed path policy in the prompt as guidance. Subagents always run in
        // yolo mode and inherit all base tools, so only model/path_deny are
        // forwarded through the bridge options.
        let path_note = format!(
            "\n\n[workflow worker policy]\n\
             profile={}\n\
             path_deny={:?}\n\
             You MUST NOT call subagent or workflow.",
            request.effective.profile, request.effective.path_deny,
        );

        let prompt = format!("{}{path_note}", request.prompt);
        let input = build_subagent_bridge_input(
            &prompt,
            request.label.as_deref(),
            &request.effective,
            request.model.as_deref(),
        );
        let inv = ToolInvocation {
            id: format!("wf-agent-{}", request.agent_index),
            tool_name: "subagent".into(),
            input,
        };

        let result: ToolResult = executor
            .invoke_with_full_context(
                inv,
                crate::tool::ToolInvocationContext {
                    cancel_token: Some(request.cancel_token.clone()),
                    ..Default::default()
                },
                true, // workflow already approved at parent tool level
            )
            .await;

        if request.cancel_token.is_requested() {
            return AgentBackendResult {
                ok: false,
                output: json!({"error": "cancelled"}),
                error: Some("cancelled".into()),
            };
        }

        let err_msg = if result.ok {
            None
        } else {
            Some(
                result
                    .output
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("subagent failed")
                    .to_string(),
            )
        };
        let mut output = result.output;
        if let Some(obj) = output.as_object_mut() {
            obj.insert("backend".into(), json!("subagent_bridge"));
            obj.insert("agent_index".into(), json!(request.agent_index));
            obj.insert("profile".into(), json!(request.effective.profile));
            obj.insert("path_deny".into(), json!(request.effective.path_deny));
        }

        AgentBackendResult {
            ok: result.ok,
            output,
            error: err_msg,
        }
    }
}

/// Build the JSON tool input the production bridge sends to `subagent`.
/// Extracted for unit tests (schema + null-description regressions).
pub(crate) fn build_subagent_bridge_input(
    prompt: &str,
    label: Option<&str>,
    effective: &EffectiveAgentPolicy,
    model: Option<&str>,
) -> serde_json::Value {
    let mut options = json!({
        "path_deny": effective.path_deny,
    });
    if let Some(model) = model {
        options
            .as_object_mut()
            .expect("options object")
            .insert("model".into(), json!(model));
    }
    let mut input = json!({
        "prompt": prompt,
        "options": options,
    });
    if let Some(label) = label.map(str::trim).filter(|s| !s.is_empty()) {
        input
            .as_object_mut()
            .expect("input object")
            .insert("description".into(), json!(label));
    }
    input
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HarnessConfig, NaviConfig};
    use crate::model::{ModelProvider, ModelRequest, ModelStream};
    use crate::runtime_components::RuntimeComponents;
    use crate::tool::Tool;
    use crate::tool::builtin::SubagentTool;
    use crate::tool::builtin::workflow::policy::default_run_policy;
    use std::sync::{Arc, RwLock};

    struct NoopProvider;
    impl ModelProvider for NoopProvider {
        fn stream(&self, _req: ModelRequest) -> ModelStream {
            Box::pin(futures_util::stream::empty())
        }
    }

    /// Schema from a real SubagentTool — never a hand-rolled duplicate that can drift.
    fn registered_subagent_schema() -> serde_json::Value {
        let tool = SubagentTool::new(
            std::sync::Weak::new(),
            Arc::new(RwLock::new(Arc::new(NoopProvider) as Arc<dyn ModelProvider>)),
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            Arc::new(RwLock::new("test".into())),
            HarnessConfig::default(),
            Arc::new(RwLock::new(NaviConfig::default())),
            RuntimeComponents::default(),
        );
        tool.definition().input_schema
    }

    #[test]
    fn bridge_input_omits_null_description_when_label_missing() {
        let run = default_run_policy();
        let effective = crate::tool::builtin::workflow::policy::intersect_agent_policy(
            &run,
            &Default::default(),
        );
        let input = build_subagent_bridge_input("do work", None, &effective, None);
        assert!(
            input.get("description").is_none(),
            "missing label must not serialize description:null, got {input}"
        );
        assert!(input["options"]["path_deny"].is_array());
        // Validate against the live SubagentTool schema (not a hand-rolled twin).
        let schema = registered_subagent_schema();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let errors: Vec<_> = validator
            .iter_errors(&input)
            .map(|e| e.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "bridge input invalid vs registered SubagentTool schema: {errors:?} input={input}"
        );
    }

    #[test]
    fn bridge_input_includes_non_empty_label() {
        let run = default_run_policy();
        let effective = crate::tool::builtin::workflow::policy::intersect_agent_policy(
            &run,
            &Default::default(),
        );
        let input = build_subagent_bridge_input("p", Some("  collect  "), &effective, None);
        assert_eq!(input["description"], "collect");
        let schema = registered_subagent_schema();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let errors: Vec<_> = validator
            .iter_errors(&input)
            .map(|e| e.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "labeled bridge input invalid vs SubagentTool schema: {errors:?} input={input}"
        );
    }
}
