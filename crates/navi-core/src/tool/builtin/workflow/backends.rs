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
use super::policy::EffectiveAgentPolicy;
use super::types::{AgentBackendResult, AgentRequest, NESTED_WORKFLOW_TOOLS};
use crate::security::{SecurityDecision, SecurityPolicy};
use crate::tool::{ToolExecutor, ToolInvocation, ToolResult};

/// Write-oriented tool names used when probing policy.
const WRITE_TOOLS: &[&str] = &[
    "write_file",
    "write",
    "edit",
    "multiedit",
    "apply_patch",
    "code_edit",
];
const COMMAND_TOOLS: &[&str] = &["bash", "sandbox"];

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

        let probe = probe_worker_capabilities_inner(&self.policy, &request.effective);

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
                "tools": request.effective.tools,
                "create_files": request.effective.create_files,
                "create_dirs": request.effective.create_dirs,
                "write_allow": request.effective.write_allow,
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
struct ProbeResult {
    can_write_file: bool,
    can_edit: bool,
    can_bash: bool,
    can_subagent: bool,
    can_workflow: bool,
    write_path_allowed: Vec<String>,
    write_path_denied: Vec<String>,
    create_new_file_allowed: bool,
    registered_tools: Vec<String>,
    policy_denials: Vec<String>,
}

/// Public probe summary for unit tests (fields only; no mock echo).
#[derive(Debug, Clone)]
pub struct WorkerProbeSummary {
    pub can_bash: bool,
    pub can_write_file: bool,
    pub can_edit: bool,
    pub can_subagent: bool,
    pub can_workflow: bool,
    pub registered_tools: Vec<String>,
}

/// Probe real tool allowlist + SecurityPolicy for a worker's effective policy.
pub fn probe_worker_capabilities(
    base_policy: &SecurityPolicy,
    effective: &EffectiveAgentPolicy,
) -> WorkerProbeSummary {
    let inner = probe_worker_capabilities_inner(base_policy, effective);
    WorkerProbeSummary {
        can_bash: inner.can_bash,
        can_write_file: inner.can_write_file,
        can_edit: inner.can_edit,
        can_subagent: inner.can_subagent,
        can_workflow: inner.can_workflow,
        registered_tools: inner.registered_tools,
    }
}

fn scoped_policy(base: &SecurityPolicy, effective: &EffectiveAgentPolicy) -> SecurityPolicy {
    base.clone()
        .with_write_scope(crate::security::WritePathScope {
            write_allow: effective.write_allow.clone(),
            path_deny: effective.path_deny.clone(),
            create_files: effective.create_files,
            create_dirs: effective.create_dirs,
        })
}

fn probe_worker_capabilities_inner(
    base_policy: &SecurityPolicy,
    effective: &EffectiveAgentPolicy,
) -> ProbeResult {
    let mut out = ProbeResult::default();
    let project = base_policy.project_root().to_path_buf();

    // Nested orchestration must never appear after intersection + strip.
    out.can_subagent = effective.tools.iter().any(|t| t == "subagent");
    out.can_workflow = effective.tools.iter().any(|t| t == "workflow");

    // Worker executor with WritePathScope (same gate as production SubagentBridge).
    let policy = scoped_policy(base_policy, effective);
    let mut exec = ToolExecutor::empty(policy.clone());
    register_filtered_tools(&mut exec, &project, effective);
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
        let mut c = effective.write_allow.clone();
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
        if let Some(first) = effective.write_allow.first() {
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
    if let Some(wa) = effective.write_allow.first() {
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
                out.create_new_file_allowed = out.can_write_file && effective.create_files;
            }
        }
    } else {
        out.create_new_file_allowed = false;
    }

    // Empty write_allow ⇒ no writes even if tools listed.
    if effective.write_allow.is_empty() {
        out.can_write_file = false;
        out.can_edit = false;
        out.create_new_file_allowed = false;
    }

    out
}

fn register_filtered_tools(
    exec: &mut ToolExecutor,
    project: &std::path::Path,
    effective: &EffectiveAgentPolicy,
) {
    use super::super::{
        bash::BashTool, edit_tool::EditTool, read_tool::ReadTool, search_tool::SearchTool,
        write_tool::WriteTool,
    };

    // Never register orchestration tools.
    let allowed: Vec<&str> = effective
        .tools
        .iter()
        .map(|s| s.as_str())
        .filter(|t| !NESTED_WORKFLOW_TOOLS.contains(t))
        .collect();

    let has = |name: &str| allowed.contains(&name);

    if has("read_file") || has("read") || has("view_file") {
        exec.register_tool(Arc::new(ReadTool::new(project.to_path_buf())));
    }
    if has("search") || has("grep") || has("fs_browser") || has("list_dir") || has("glob") {
        exec.register_tool(Arc::new(SearchTool::new(project.to_path_buf())));
    }

    // Writes only when write_allow is non-empty (empty ⇒ no writes even for implementer).
    // create_files=false still registers tools; WritePathScope denies creates.
    let writes_ok = !effective.write_allow.is_empty();
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

        // Embed path policy in the prompt (guidance) AND pass write scope fields
        // so SubagentTool forks a SecurityPolicy with WritePathScope (hard gate).
        let tools_for_note: Vec<String> = {
            let mut t = request.effective.tools.clone();
            t.retain(|n| !NESTED_WORKFLOW_TOOLS.contains(&n.as_str()));
            if request.effective.write_allow.is_empty() {
                t.retain(|n| {
                    !WRITE_TOOLS.contains(&n.as_str()) && !COMMAND_TOOLS.contains(&n.as_str())
                });
            }
            t
        };
        let path_note = format!(
            "\n\n[workflow worker policy]\n\
             profile={}\n\
             tools={:?}\n\
             write_allow={:?}\n\
             path_deny={:?}\n\
             create_files={}\n\
             create_dirs={}\n\
             You MUST NOT call subagent or workflow. \
             Writes are only allowed on write_allow paths (empty ⇒ no writes).",
            request.effective.profile,
            tools_for_note,
            request.effective.write_allow,
            request.effective.path_deny,
            request.effective.create_files,
            request.effective.create_dirs,
        );

        let prompt = format!("{}{path_note}", request.prompt);
        let input = build_subagent_bridge_input(
            &prompt,
            request.label.as_deref(),
            &request.effective,
            request.model.as_deref(),
            request.max_tokens,
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
            obj.insert("tools".into(), json!(request.effective.tools));
            obj.insert("write_allow".into(), json!(request.effective.write_allow));
            obj.insert("create_files".into(), json!(request.effective.create_files));
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
    max_tokens: Option<usize>,
) -> serde_json::Value {
    let mut tools = effective.tools.clone();
    tools.retain(|t| !NESTED_WORKFLOW_TOOLS.contains(&t.as_str()));
    if effective.write_allow.is_empty() {
        tools
            .retain(|t| !WRITE_TOOLS.contains(&t.as_str()) && !COMMAND_TOOLS.contains(&t.as_str()));
    }
    let approval = if effective.write_allow.is_empty() {
        "read_only"
    } else if effective.approval == "escalate" {
        "escalate"
    } else {
        effective.approval.as_str()
    };
    let mut options = json!({
        "agent_profile": effective.profile,
        "tools": tools,
        "approval": approval,
        "write_allow": effective.write_allow,
        "path_deny": effective.path_deny,
        "create_files": effective.create_files,
        "create_dirs": effective.create_dirs,
    });
    if let Some(model) = model {
        options
            .as_object_mut()
            .expect("options object")
            .insert("model".into(), json!(model));
    }
    if let Some(max_tokens) = max_tokens {
        options
            .as_object_mut()
            .expect("options object")
            .insert("max_tokens".into(), json!(max_tokens));
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
    use crate::tool::builtin::workflow::policy::default_run_policy;

    #[test]
    fn bridge_input_omits_null_description_when_label_missing() {
        let mut run = default_run_policy();
        run.create_files = true;
        run.write_allow = vec!["scratch/a.txt".into()];
        run.tools = vec![
            "read_file".into(),
            "write_file".into(),
            "edit".into(),
            "search".into(),
        ];
        let effective = crate::tool::builtin::workflow::policy::intersect_agent_policy(
            &run,
            &crate::tool::builtin::workflow::policy::AgentPolicyOpts {
                profile: Some("implementer".into()),
                ..Default::default()
            },
        );
        assert!(effective.create_files);
        let input = build_subagent_bridge_input("do work", None, &effective, None, None);
        assert!(
            input.get("description").is_none(),
            "missing label must not serialize description:null, got {input}"
        );
        assert_eq!(input["options"]["create_files"], true);
        assert_eq!(input["options"]["write_allow"], json!(["scratch/a.txt"]));
        // Must validate against live subagent schema.
        let schema = json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string" },
                "description": { "type": "string" },
                "options": {
                    "type": "object",
                    "properties": {
                        "agent_profile": { "type": "string" },
                        "tools": { "type": "array", "items": { "type": "string" } },
                        "approval": { "type": "string" },
                        "write_allow": { "type": "array", "items": { "type": "string" } },
                        "path_deny": { "type": "array", "items": { "type": "string" } },
                        "create_files": { "type": "boolean" },
                        "create_dirs": { "type": "boolean" },
                        "model": { "type": "string" },
                        "max_tokens": { "type": "integer" }
                    },
                    "additionalProperties": false
                }
            },
            "required": ["prompt"],
            "additionalProperties": false
        });
        let validator = jsonschema::validator_for(&schema).unwrap();
        let errors: Vec<_> = validator
            .iter_errors(&input)
            .map(|e| e.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "bridge input invalid: {errors:?} input={input}"
        );
    }

    #[test]
    fn bridge_input_includes_non_empty_label() {
        let run = default_run_policy();
        let effective = crate::tool::builtin::workflow::policy::intersect_agent_policy(
            &run,
            &Default::default(),
        );
        let input = build_subagent_bridge_input("p", Some("  collect  "), &effective, None, None);
        assert_eq!(input["description"], "collect");
    }
}
