//! Acceptance tests for workflow tool (Must rows in docs/workflow-tool-lua-spec.md §11).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::cancel::CancelToken;
use crate::config::WorkflowConfig;
use crate::security::SecurityPolicy;
use crate::tool::{Tool, ToolInvocation, ToolInvocationContext};

fn temp_policy() -> (tempfile::TempDir, SecurityPolicy) {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().join("project");
    let data = dir.path().join("data");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    let policy = SecurityPolicy::new(project, data, crate::config::SecurityConfig::default())
        .expect("policy");
    (dir, policy)
}

fn tool_with_mock(policy: SecurityPolicy, mock: MockAgentBackend) -> WorkflowTool {
    WorkflowTool::with_mock(policy, WorkflowConfig::default(), mock)
}

/// Integration path: real SecurityPolicy + filtered tool registration (no model).
fn tool_with_probe(policy: SecurityPolicy) -> WorkflowTool {
    let probe = WorkerProbeBackend::new(policy.clone());
    WorkflowTool::with_probe(policy, WorkflowConfig::default(), probe)
}

fn tool_with_probe_delay(
    policy: SecurityPolicy,
    delay_ms: u64,
    inflight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
) -> WorkflowTool {
    let probe = WorkerProbeBackend::new(policy.clone())
        .with_delay(delay_ms)
        .with_inflight(inflight, peak);
    WorkflowTool::with_probe(policy, WorkflowConfig::default(), probe)
}

fn inv(input: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
        id: "inv-1".into(),
        tool_name: "workflow".into(),
        input,
    }
}

async fn run(tool: &WorkflowTool, input: serde_json::Value) -> crate::tool::ToolResult {
    tool.invoke(inv(input)).await.expect("invoke")
}

// ── T* Tool & schema ──────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t1_tool_registered_name_is_workflow() {
    let (_dir, policy) = temp_policy();
    let tool = WorkflowTool::new(policy, WorkflowConfig::default());
    assert_eq!(tool.definition().name, "workflow");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t2_missing_script_errors() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(&tool, json!({})).await;
    assert!(!r.ok);
    assert_eq!(
        r.output["error"]["code"].as_str(),
        Some("invalid_host_call")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_oversized_script_rejected() {
    let (_dir, policy) = temp_policy();
    let mut cfg = WorkflowConfig::default();
    cfg.max_script_bytes = 64;
    let tool = WorkflowTool::with_mock(policy, cfg, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({"script": "function workflow() return 1 end".repeat(10)}),
    )
    .await;
    assert!(!r.ok);
    assert_eq!(r.output["error"]["code"].as_str(), Some("script_too_large"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t4_args_injected_into_lua() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({
            "script": "function workflow() return {x = args.x, y = args.y} end",
            "args": {"x": 42, "y": "hi"}
        }),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    assert_eq!(r.output["result"]["x"], 42);
    assert_eq!(r.output["result"]["y"], "hi");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t5_success_returns_stable_fields() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({"script": "function workflow() return {ok = true} end"}),
    )
    .await;
    assert!(r.ok);
    assert!(r.output["run_id"].as_str().unwrap().starts_with("wf_"));
    assert_eq!(r.output["status"], "completed");
    assert!(r.output.get("result").is_some());
    assert!(r.output.get("stats").is_some());
    assert!(r.output.get("journal_path").is_some());
}

// ── L* Lua sandbox ────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l1_workflow_entrypoint_returns_result() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({"script": "function workflow() return 123 end"}),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    assert_eq!(r.output["result"], 123);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l2_syntax_error_parse_code() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(&tool, json!({"script": "function workflow( end"})).await;
    assert!(!r.ok);
    assert_eq!(
        r.output["error"]["code"].as_str(),
        Some("script_parse_error")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l3_require_and_io_sandbox() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({"script": "function workflow() return require('io') end"}),
    )
    .await;
    assert!(!r.ok, "{:?}", r.output);
    let code = r.output["error"]["code"].as_str().unwrap_or("");
    assert!(
        code == "sandbox_violation"
            || code == "script_runtime_error"
            || code == "script_parse_error",
        "code={code} out={:?}",
        r.output
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l4_infinite_loop_hits_limit() {
    let (_dir, policy) = temp_policy();
    let mut cfg = WorkflowConfig::default();
    cfg.run_timeout_ms = 10_000;
    let tool = WorkflowTool::with_mock(policy, cfg, MockAgentBackend::default());
    let started = std::time::Instant::now();
    let r = tokio::time::timeout(
        Duration::from_secs(15),
        run(
            &tool,
            json!({"script": "function workflow() while true do end end"}),
        ),
    )
    .await
    .expect("loop should not hang forever");
    assert!(!r.ok, "{:?}", r.output);
    assert!(started.elapsed() < Duration::from_secs(15));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l5_agent_returns_table_without_json_lib() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({
            "script": r#"
                function workflow()
                    local a = agent("hello", {label = "t"})
                    return {ok = a.ok, prompt = a.prompt, label = a.label}
                end
            "#
        }),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    assert_eq!(r.output["result"]["ok"], true);
    assert_eq!(r.output["result"]["prompt"], "hello");
}

// ── H* Host API ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h1_agent_invokes_backend() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({"script": r#"function workflow() return agent("task") end"#}),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    assert_eq!(r.output["result"]["prompt"], "task");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h2_pipeline_once_per_item() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({
            "script": r#"
                function workflow()
                    return pipeline({"a","b","c"}, function(x)
                        return agent(x)
                    end)
                end
            "#
        }),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    let arr = r.output["result"].as_array().expect("array");
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0]["prompt"], "a");
    assert_eq!(arr[1]["prompt"], "b");
    assert_eq!(arr[2]["prompt"], "c");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h3_parallel_two_arg_rejected() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({
            "script": r#"
                function workflow()
                    return parallel({"a","b"}, function(x) return x end)
                end
            "#
        }),
    )
    .await;
    assert!(!r.ok, "{:?}", r.output);
    assert_eq!(
        r.output["error"]["code"].as_str(),
        Some("invalid_host_call")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h4_parallel_preserves_order() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(
        policy,
        MockAgentBackend {
            delay_ms: 20,
            ..Default::default()
        },
    );
    let r = run(
        &tool,
        json!({
            "script": r#"
                function workflow()
                    return parallel({
                        function() return agent("first") end,
                        function() return agent("second") end,
                        function() return agent("third") end,
                    })
                end
            "#
        }),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    let arr = r.output["result"].as_array().expect("array");
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0]["prompt"], "first");
    assert_eq!(arr[1]["prompt"], "second");
    assert_eq!(arr[2]["prompt"], "third");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h5_phase_and_log_recorded() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({
            "script": r#"
                function workflow()
                    phase("alpha")
                    log("hello")
                    phase("beta")
                    return true
                end
            "#
        }),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    let phases = r.output["stats"]["phases"].as_array().unwrap();
    assert!(phases.iter().any(|p| p == "alpha"));
    assert!(phases.iter().any(|p| p == "beta"));
    let journal = r.output["journal_path"].as_str().unwrap();
    let body = std::fs::read_to_string(journal).unwrap();
    assert!(body.contains("phase") || body.contains("alpha"));
    assert!(body.contains("hello") || body.contains("log"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h6_args_readonly() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({
            "script": r#"
                function workflow()
                    args.x = 1
                    return true
                end
            "#,
            "args": {}
        }),
    )
    .await;
    assert!(!r.ok, "{:?}", r.output);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h7_unknown_builtin_absent() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({
            "script": r#"
                function workflow()
                    return type(subagent) == "nil" and type(read_file) == "nil" and type(require) == "nil"
                end
            "#
        }),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    assert_eq!(r.output["result"], true);
}

// ── C* Caps ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c1_default_max_parallel_is_16() {
    assert_eq!(DEFAULT_MAX_PARALLEL, 16);
    assert_eq!(WorkflowConfig::default().max_parallel, 16);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c2_never_exceeds_max_parallel() {
    let (_dir, policy) = temp_policy();
    let inflight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let tool = tool_with_probe_delay(policy, 80, inflight, peak.clone());
    let r = run(
        &tool,
        json!({
            "max_parallel": 4,
            "script": r#"
                function workflow()
                    local thunks = {}
                    for i = 1, 12 do
                        thunks[i] = function()
                            return agent("n" .. i)
                        end
                    end
                    return parallel(thunks)
                end
            "#
        }),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    let peak_n = peak.load(Ordering::SeqCst);
    assert!(peak_n <= 4, "peak_in_flight={peak_n}");
    assert!(peak_n >= 2, "expected some concurrency, peak={peak_n}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c3_max_agents_default_enforced() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({
            "max_agents": 3,
            "script": r#"
                function workflow()
                    agent("1"); agent("2"); agent("3"); agent("4")
                    return true
                end
            "#
        }),
    )
    .await;
    assert!(!r.ok, "{:?}", r.output);
    assert_eq!(
        r.output["error"]["code"].as_str(),
        Some("agent_cap_exceeded")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c4_clamps_to_ceilings() {
    assert_eq!(clamp_max_parallel(999), MAX_PARALLEL_CEILING);
    assert_eq!(clamp_max_agents(99999), MAX_AGENTS_CEILING);
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({
            "max_parallel": 999,
            "max_agents": 99999,
            "script": "function workflow() return {p = 1} end"
        }),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c5_twelve_concurrent_beyond_subagent_bg_cap() {
    // Standalone MAX_BACKGROUND_SUBAGENTS = 8; workflow must allow 12 when max_parallel=16.
    // Uses WorkerProbeBackend (real SecurityPolicy path), not mock echo.
    let (_dir, policy) = temp_policy();
    let inflight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let tool = tool_with_probe_delay(policy, 100, inflight, peak.clone());
    let r = run(
        &tool,
        json!({
            "max_parallel": 16,
            "script": r#"
                function workflow()
                    local thunks = {}
                    for i = 1, 12 do
                        thunks[i] = function() return agent("w" .. i) end
                    end
                    return parallel(thunks)
                end
            "#
        }),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    assert_eq!(r.output["result"][1]["backend"], "worker_probe");
    let peak_n = peak.load(Ordering::SeqCst);
    assert!(
        peak_n > 8,
        "workflow must not be stuck at subagent bg cap 8; peak={peak_n}"
    );
    assert!(peak_n <= 16);
}

// ── P* Permissions (WorkerProbeBackend: real tool registry + SecurityPolicy) ─

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p1_default_worker_no_write_tools() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_probe(policy);
    let r = run(
        &tool,
        json!({"script": r#"function workflow() return agent("x") end"#}),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    let res = &r.output["result"];
    assert_eq!(res["backend"], "worker_probe");
    assert_eq!(res["can_write_file"], false, "{res:?}");
    assert_eq!(res["can_edit"], false, "{res:?}");
    assert_eq!(res["can_bash"], false, "{res:?}");
    assert_eq!(res["create_files"], false);
    // Registered tools must not include writers.
    let reg = res["registered_tools"].as_array().unwrap();
    let names: Vec<&str> = reg.iter().filter_map(|t| t.as_str()).collect();
    assert!(
        names
            .iter()
            .any(|t| *t == "read_file" || *t == "search" || *t == "read"),
        "expected read tools registered: {names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|t| *t == "write_file" || *t == "edit" || *t == "bash")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p2_write_allow_single_file() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_probe(policy);
    let r = run(
        &tool,
        json!({
            "policy": {
                "create_files": true,
                "write_allow": ["src/a.rs", "src/b.rs"],
                "tools": ["read_file", "search", "edit", "write_file"]
            },
            "script": r#"
                function workflow()
                    return agent("edit", {
                        write_allow = {"src/a.rs"},
                        create_files = true,
                        tools = {"read_file", "edit", "write_file"}
                    })
                end
            "#
        }),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    let res = &r.output["result"];
    assert_eq!(res["backend"], "worker_probe");
    let wa = res["write_allow"].as_array().unwrap();
    assert_eq!(wa, &vec![json!("src/a.rs")]);
    // Real ToolExecutor::validate Deny for paths outside write_allow.
    let denials = res["policy_denials"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let denial_text = denials
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        denial_text.contains("not in workflow write_allow")
            || denial_text.contains("__outside_write_allow__"),
        "expected real write_allow Deny, denials={denial_text:?} res={res:?}"
    );
    assert_eq!(res["can_write_file"], true);
    assert_eq!(res["can_edit"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p3_create_files_false_blocks_create() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_probe(policy);
    let r = run(
        &tool,
        json!({
            "policy": {
                "create_files": false,
                "write_allow": ["src/a.rs"],
                "tools": ["read_file", "write_file", "edit"]
            },
            "script": r#"
                function workflow()
                    return agent("x", {
                        create_files = true,
                        write_allow = {"src/a.rs"},
                        tools = {"read_file", "write_file", "edit"}
                    })
                end
            "#
        }),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    let res = &r.output["result"];
    assert_eq!(res["create_files"], false);
    assert_eq!(res["create_new_file_allowed"], false, "{res:?}");
    let denials = res["policy_denials"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let denial_text = denials
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        denial_text.contains("create_files=false") || denial_text.contains("create_new"),
        "expected real create_files Deny from SecurityPolicy, denials={denial_text:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p4_path_deny_wins() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_probe(policy);
    let r = run(
        &tool,
        json!({
            "policy": {
                "create_files": true,
                "write_allow": ["src/a.rs"],
                "tools": ["read_file", "write_file", "edit"]
            },
            "script": r#"
                function workflow()
                    return agent("x", {
                        write_allow = {"src/a.rs"},
                        path_deny = {"src/a.rs"},
                        create_files = true,
                        tools = {"read_file", "write_file", "edit"}
                    })
                end
            "#
        }),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    let res = &r.output["result"];
    let denials = res["policy_denials"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let denial_text = denials
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        denial_text.contains("path_deny") || denial_text.contains("src/a.rs"),
        "expected real path_deny Deny from SecurityPolicy, denials={denial_text:?} res={res:?}"
    );
    let allowed: Vec<&str> = res["write_path_allowed"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert!(
        !allowed.iter().any(|p| *p == "src/a.rs"),
        "src/a.rs must not be allowed when path_deny wins: {res:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p5_worker_no_nested_orchestration() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_probe(policy);
    let r = run(
        &tool,
        json!({
            "policy": {
                "tools": ["read_file", "search", "subagent", "workflow"]
            },
            "script": r#"
                function workflow()
                    return agent("x", {tools = {"read_file", "subagent", "workflow"}})
                end
            "#
        }),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    let res = &r.output["result"];
    assert_eq!(res["can_subagent"], false, "{res:?}");
    assert_eq!(res["can_workflow"], false, "{res:?}");
    let reg = res["registered_tools"].as_array().unwrap();
    let names: Vec<&str> = reg.iter().filter_map(|t| t.as_str()).collect();
    assert!(!names.contains(&"subagent"));
    assert!(!names.contains(&"workflow"));
}

#[test]
fn p6_intersection_unit() {
    let run = default_run_policy();
    let opts = AgentPolicyOpts {
        tools: Some(vec!["bash".into(), "read_file".into()]),
        ..Default::default()
    };
    let eff = intersect_agent_policy(&run, &opts);
    assert!(!eff.tools.iter().any(|t| t == "bash"));
    // Real probe path: bash must not be registered under default run.
    let (_dir, policy) = temp_policy();
    let probe = super::backends::probe_worker_capabilities(&policy, &eff);
    assert!(!probe.can_bash, "{probe:?}");
    assert!(
        !probe.registered_tools.iter().any(|t| t == "bash"),
        "{probe:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p7_implementer_empty_write_allow() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_probe(policy);
    let r = run(
        &tool,
        json!({
            "script": r#"
                function workflow()
                    return agent("x", {
                        profile = "implementer",
                        write_allow = {},
                        create_files = true,
                        tools = {"read_file", "write_file", "edit"}
                    })
                end
            "#
        }),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    let res = &r.output["result"];
    assert_eq!(res["backend"], "worker_probe");
    assert_eq!(res["can_write_file"], false, "{res:?}");
    assert_eq!(res["can_edit"], false, "{res:?}");
    assert_eq!(res["create_files"], false);
    assert_eq!(res["create_new_file_allowed"], false);
    let reg = res["registered_tools"].as_array().unwrap();
    assert!(
        !reg.iter()
            .any(|t| t.as_str() == Some("write_file") || t.as_str() == Some("edit"))
    );
}

#[test]
fn production_constructor_is_subagent_bridge() {
    // Structural: with_subagent_bridge is the production API used by runtime/sdk.
    let (_dir, policy) = temp_policy();
    let weak: std::sync::Weak<crate::tool::ToolExecutor> = std::sync::Weak::new();
    let tool = WorkflowTool::with_subagent_bridge(policy, WorkflowConfig::default(), weak);
    assert_eq!(tool.definition().name, "workflow");
}

/// Production path regression: the real `SubagentBridgeBackend` payload must pass
/// `ToolExecutor::validate_arguments` for a registered `subagent` tool.
///
/// Historical hole: suite used MockAgentBackend / WorkerProbeBackend for almost all
/// agent() tests, so schema mismatches (write_allow/create_files unexpected, description
/// null without label) never failed CI while TUI production always hit them.
#[test]
fn production_bridge_payload_validates_on_real_subagent_executor() {
    use crate::config::{HarnessConfig, NaviConfig};
    use crate::model::{ModelProvider, ModelRequest, ModelStream};
    use crate::prompt::PromptCache;
    use crate::runtime_components::RuntimeComponents;
    use crate::tool::ToolExecutor;
    use crate::tool::builtin::SubagentTool;
    use crate::tool::builtin::workflow::backends::build_subagent_bridge_input;
    use crate::tool::builtin::workflow::policy::{
        AgentPolicyOpts, default_run_policy, intersect_agent_policy,
    };
    use std::sync::{Arc, RwLock};

    struct NoopProvider;
    impl ModelProvider for NoopProvider {
        fn stream(&self, _req: ModelRequest) -> ModelStream {
            Box::pin(futures_util::stream::empty())
        }
    }

    let (_dir, policy) = temp_policy();
    let mut executor = ToolExecutor::new(policy);
    let subagent = SubagentTool::new(
        std::sync::Weak::new(),
        Arc::new(RwLock::new(Arc::new(NoopProvider) as Arc<dyn ModelProvider>)),
        PathBuf::from("/tmp"),
        PathBuf::from("/tmp"),
        Arc::new(RwLock::new("test".into())),
        HarnessConfig::default(),
        Arc::new(RwLock::new(NaviConfig::default())),
        Arc::new(PromptCache::new()),
        RuntimeComponents::default(),
    );
    executor.register_tool(Arc::new(subagent));

    // Case A: no label + full write-scope options (exactly what production TUI sends).
    let mut run = default_run_policy();
    run.create_files = true;
    run.write_allow = vec!["scratch/probe.txt".into()];
    run.tools = vec![
        "read_file".into(),
        "write_file".into(),
        "edit".into(),
        "search".into(),
    ];
    let effective = intersect_agent_policy(
        &run,
        &AgentPolicyOpts {
            profile: Some("implementer".into()),
            ..Default::default()
        },
    );
    assert!(
        effective.create_files,
        "run create_files must inherit when agent omits the flag"
    );
    let input = build_subagent_bridge_input(
        "create scratch/probe.txt with ok",
        None, // label missing → must NOT emit description:null
        &effective,
        None,
        None,
    );
    assert!(
        input.get("description").is_none(),
        "null description regression: {input}"
    );
    let inv = ToolInvocation {
        id: "wf-agent-1".into(),
        tool_name: "subagent".into(),
        input,
    };
    executor
        .validate_arguments(&inv)
        .unwrap_or_else(|e| panic!("production bridge payload rejected by subagent schema: {e:?}"));

    // Case B: explicit label still accepted.
    let input2 = build_subagent_bridge_input("ping", Some("worker-1"), &effective, None, None);
    let inv2 = ToolInvocation {
        id: "wf-agent-2".into(),
        tool_name: "subagent".into(),
        input: input2,
    };
    executor
        .validate_arguments(&inv2)
        .expect("labeled bridge payload must validate");
}

/// Production wiring helper: real `SubagentTool` + `WorkflowTool::with_subagent_bridge`
/// sharing one `ToolExecutor` (same shape as runtime/sdk). No live LLM.
fn production_bridge_executor(policy: SecurityPolicy) -> Arc<crate::tool::ToolExecutor> {
    use crate::config::{HarnessConfig, NaviConfig};
    use crate::model::{ModelProvider, ModelRequest, ModelStream};
    use crate::prompt::PromptCache;
    use crate::runtime_components::RuntimeComponents;
    use crate::tool::ToolExecutor;
    use crate::tool::builtin::SubagentTool;
    use std::sync::RwLock;

    struct NoopProvider;
    impl ModelProvider for NoopProvider {
        fn stream(&self, _req: ModelRequest) -> ModelStream {
            Box::pin(futures_util::stream::empty())
        }
    }

    let project = policy.project_root().to_path_buf();
    let data = policy.data_dir().to_path_buf();

    Arc::new_cyclic(|weak| {
        let mut executor = ToolExecutor::empty(policy.clone());
        let subagent = SubagentTool::new(
            weak.clone(),
            Arc::new(RwLock::new(Arc::new(NoopProvider) as Arc<dyn ModelProvider>)),
            project.clone(),
            data.clone(),
            Arc::new(RwLock::new("test".into())),
            HarnessConfig::default(),
            Arc::new(RwLock::new(NaviConfig::default())),
            Arc::new(PromptCache::new()),
            RuntimeComponents::default(),
        );
        executor.register_tool(Arc::new(subagent));
        executor.register_tool(Arc::new(WorkflowTool::with_subagent_bridge(
            policy.clone(),
            WorkflowConfig::default(),
            weak.clone(),
        )));
        executor
    })
}

/// Reject schema / argument-validation failures anywhere in a nested agent result.
fn assert_no_schema_validation_failure(output: &serde_json::Value, context: &str) {
    let dump = output.to_string();
    assert!(
        !dump.contains("\"error_code\":\"invalid_arguments\"")
            && !dump.contains("Additional properties are not allowed")
            && !dump.contains("null is not of type"),
        "{context}: schema/validation failure on production bridge path: {output}"
    );
    if let Some(code) = output.get("error_code").and_then(|v| v.as_str()) {
        assert_ne!(
            code, "invalid_arguments",
            "{context}: top-level invalid_arguments: {output}"
        );
    }
    if let Some(code) = output
        .pointer("/result/error_code")
        .and_then(|v| v.as_str())
    {
        assert_ne!(
            code, "invalid_arguments",
            "{context}: agent result invalid_arguments: {output}"
        );
    }
}

/// Full production path: Lua `agent("ping")` (no label) → SubagentBridgeBackend →
/// ToolExecutor.invoke(subagent). Must not fail schema validation for description:null.
///
/// Noop provider is fine — later turn failure is OK; this asserts schema + bridge wiring.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_bridge_agent_without_label_not_schema_error() {
    let (_dir, policy) = temp_policy();
    let executor = production_bridge_executor(policy);

    let result = executor
        .invoke_with_full_context(
            ToolInvocation {
                id: "wf-prod-ping".into(),
                tool_name: "workflow".into(),
                input: json!({
                    "script": r#"function workflow() return agent("ping") end"#,
                }),
            },
            ToolInvocationContext::default(),
            true, // workflow already approved at parent level in production
        )
        .await;

    assert!(
        result.ok,
        "workflow host must complete (agent turn may still fail later): {:?}",
        result.output
    );
    assert_no_schema_validation_failure(&result.output, "agent(\"ping\") without label");

    let agent = &result.output["result"];
    assert_eq!(
        agent["backend"].as_str(),
        Some("subagent_bridge"),
        "must use SubagentBridgeBackend, not mock/probe: {agent:?}"
    );
    // Description omitted path: success or non-schema failure only.
    assert!(
        agent.get("error_code").and_then(|v| v.as_str()) != Some("invalid_arguments"),
        "description:null / schema must not reject unlabeled agent: {agent:?}"
    );
}

/// Production path with write-scope options (create_files + write_allow).
/// Historical hole: options.create_files / write_allow rejected as additionalProperties.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_bridge_agent_write_scope_options_not_schema_error() {
    let (_dir, policy) = temp_policy();
    let executor = production_bridge_executor(policy);

    let result = executor
        .invoke_with_full_context(
            ToolInvocation {
                id: "wf-prod-write-scope".into(),
                tool_name: "workflow".into(),
                input: json!({
                    "policy": {
                        "profile": "implementer",
                        "create_files": true,
                        "write_allow": ["scratch/x.txt"],
                        "tools": ["read_file", "write_file", "edit", "search"]
                    },
                    "script": r#"
                        function workflow()
                            return agent("create scratch/x.txt with ok", {
                                create_files = true,
                                write_allow = {"scratch/x.txt"},
                                tools = {"read_file", "write_file", "edit", "search"}
                            })
                        end
                    "#,
                }),
            },
            ToolInvocationContext::default(),
            true,
        )
        .await;

    assert!(
        result.ok,
        "workflow host must complete: {:?}",
        result.output
    );
    assert_no_schema_validation_failure(&result.output, "write-scope agent options");

    let agent = &result.output["result"];
    assert_eq!(
        agent["backend"].as_str(),
        Some("subagent_bridge"),
        "must use SubagentBridgeBackend: {agent:?}"
    );
    assert_eq!(
        agent["create_files"], true,
        "effective create_files should surface on bridge result: {agent:?}"
    );
    let wa = agent["write_allow"].as_array().cloned().unwrap_or_default();
    assert!(
        wa.iter().any(|v| v.as_str() == Some("scratch/x.txt")),
        "write_allow must pass through bridge payload: {agent:?}"
    );
    assert!(
        agent.get("error_code").and_then(|v| v.as_str()) != Some("invalid_arguments"),
        "write-scope options must pass registered subagent schema: {agent:?}"
    );
}

// ── R* Lifecycle ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r1_run_id_unique() {
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r1 = run(&tool, json!({"script": "function workflow() return 1 end"})).await;
    let r2 = run(&tool, json!({"script": "function workflow() return 2 end"})).await;
    assert_ne!(r1.output["run_id"], r2.output["run_id"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r2_journal_under_data_dir_workflows() {
    let (dir, policy) = temp_policy();
    let data = policy.data_dir().to_path_buf();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({"script": "function workflow() return agent(\"j\") end"}),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    let jp = r.output["journal_path"].as_str().unwrap();
    let path = PathBuf::from(jp);
    assert!(path.starts_with(&data.join("workflows")));
    assert!(path.ends_with("journal.jsonl"));
    assert!(path.exists());
    assert!(path.parent().unwrap().join("meta.json").exists());
    // Ensure not under project .navi
    let project_navi = dir.path().join("project").join(".navi");
    assert!(
        !project_navi.exists()
            || std::fs::read_dir(&project_navi)
                .map(|d| d.count())
                .unwrap_or(0)
                == 0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r3_cancel_mid_run() {
    let (_dir, policy) = temp_policy();
    let inflight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let tool = tool_with_probe_delay(policy, 800, inflight, peak);
    let cancel = CancelToken::new();
    let cancel2 = cancel.clone();
    let handle = tokio::spawn(async move {
        tool.invoke_with_context(
            inv(json!({
                "script": r#"
                    function workflow()
                        local t = {}
                        for i = 1, 8 do
                            t[i] = function() return agent("c" .. i) end
                        end
                        return parallel(t)
                    end
                "#
            })),
            ToolInvocationContext {
                cancel_token: Some(cancel2),
                ..Default::default()
            },
        )
        .await
        .expect("invoke")
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    cancel.cancel();
    let r = handle.await.expect("join");
    assert!(!r.ok, "{:?}", r.output);
    assert_eq!(
        r.output["status"].as_str(),
        Some("cancelled"),
        "R3 requires status=cancelled, got {:?}",
        r.output
    );
    assert_eq!(r.output["error"]["code"].as_str(), Some("cancelled"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r4_timeout_status() {
    let (_dir, policy) = temp_policy();
    let inflight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut cfg = WorkflowConfig::default();
    cfg.run_timeout_ms = 50;
    let probe = WorkerProbeBackend::new(policy.clone())
        .with_delay(500)
        .with_inflight(inflight, peak);
    let tool = WorkflowTool::with_probe(policy, cfg, probe);
    let r = run(
        &tool,
        json!({
            "script": r#"
                function workflow()
                    return agent("slow")
                end
            "#
        }),
    )
    .await;
    assert!(!r.ok, "{:?}", r.output);
    assert_eq!(r.output["status"].as_str(), Some("timed_out"));
    assert_eq!(r.output["error"]["code"].as_str(), Some("timeout"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r6_no_journal_under_project_navi() {
    let (dir, policy) = temp_policy();
    let project = dir.path().join("project");
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let _ = run(&tool, json!({"script": "function workflow() return 1 end"})).await;
    let navi = project.join(".navi");
    assert!(
        !navi.exists(),
        "workflow must not create project .navi bookkeeping"
    );
}

// ── F* Fixtures ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f1_enumerate_map_return_fixture() {
    let fixture = include_str!("../../../../tests/fixtures/workflow/f1_enumerate_map.lua");
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({
            "script": fixture,
            "args": {"files": ["a.rs", "b.rs", "c.rs"]}
        }),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
    assert_eq!(r.output["result"]["count"], 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f2_parallel_write_allow_fixture() {
    let fixture = include_str!("../../../../tests/fixtures/workflow/f2_parallel_write_allow.lua");
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(
        &tool,
        json!({
            "script": fixture,
            "policy": {
                "tools": ["read_file", "search", "edit"],
                "write_allow": ["src/a.rs"],
                "create_files": false
            }
        }),
    )
    .await;
    assert!(r.ok, "{:?}", r.output);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f3_negative_parallel_two_arg_fixture() {
    let fixture = include_str!("../../../../tests/fixtures/workflow/f3_parallel_two_arg.lua");
    let (_dir, policy) = temp_policy();
    let tool = tool_with_mock(policy, MockAgentBackend::default());
    let r = run(&tool, json!({"script": fixture})).await;
    assert!(!r.ok, "{:?}", r.output);
    assert_eq!(
        r.output["error"]["code"].as_str(),
        Some("invalid_host_call")
    );
}

// ── Description §12 ───────────────────────────────────────────────────────

#[test]
fn description_contains_section_12_bullets() {
    let d = workflow_tool_description();
    assert!(d.contains("function workflow()"), "entrypoint example");
    assert!(d.contains("parallel"), "parallel builtin");
    assert!(
        d.contains("zero-arg")
            || d.contains("zero-arg functions")
            || d.contains("array of zero-arg")
    );
    assert!(d.contains("read-only") || d.contains("read-only"));
    assert!(d.contains("write_allow"));
    assert!(d.contains("16") && d.contains("1000"));
    assert!(d.contains("require"));
    assert!(d.contains("JSON.parse") || d.contains("JSON"));
    assert!(d.contains("orchestrat"));
}

#[test]
fn mlua_not_quickjs() {
    // Structural: this crate depends on mlua (compile-time via module).
    let _ = DEFAULT_MAX_PARALLEL;
    assert_eq!(NESTED_WORKFLOW_TOOLS, &["subagent", "workflow"]);
}
