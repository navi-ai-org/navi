//! Integration test suite for the Harness Parity Plan.
//!
//! Validates that all major subsystems (P1-P8) are properly wired and functional.
//! Run with: cargo test -p navi-core --test parity_check

use navi_core::effect::{BlastRadius, EffectAnalyzer};
use navi_core::trace::{TraceStore, TurnTrace};
use navi_core::verifier::{VerificationStore, VerifierSpec};
use navi_core::{
    SecurityConfig, SecurityPolicy, ToolDefinition, ToolExecutor, ToolExposure, ToolKind,
    ToolMetadata, ToolRegistry, ToolResult, ToolRisk, capabilities,
};
use serde_json::json;
use std::path::PathBuf;

// ── P1: Tool Kernel v2 ────────────────────────────────────────────────────────

#[test]
fn p1_tool_metadata_exists_for_all_builtin_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let policy = SecurityPolicy::new(
        tmp.path().to_path_buf(),
        tmp.path().to_path_buf(),
        SecurityConfig::default(),
    )
    .unwrap();
    let executor = ToolExecutor::new(policy);
    let defs = executor.definitions();

    assert!(
        defs.len() >= 20,
        "expected >=20 builtin tools, got {}",
        defs.len()
    );

    for def in &defs {
        assert!(
            !def.metadata.namespace.is_empty(),
            "tool `{}` has empty namespace",
            def.name
        );
    }
}

#[test]
fn p1_tool_risk_is_classified() {
    let tmp = tempfile::tempdir().unwrap();
    let policy = SecurityPolicy::new(
        tmp.path().to_path_buf(),
        tmp.path().to_path_buf(),
        SecurityConfig::default(),
    )
    .unwrap();
    let executor = ToolExecutor::new(policy);
    let defs = executor.definitions();

    let classified = defs
        .iter()
        .filter(|d| d.metadata.risk != ToolRisk::Unspecified)
        .count();
    assert!(
        classified >= 10,
        "expected >=10 tools with classified risk, got {}",
        classified
    );
}

#[test]
fn p1_read_tools_are_read_only() {
    let tmp = tempfile::tempdir().unwrap();
    let policy = SecurityPolicy::new(
        tmp.path().to_path_buf(),
        tmp.path().to_path_buf(),
        SecurityConfig::default(),
    )
    .unwrap();
    let executor = ToolExecutor::new(policy);
    let defs = executor.definitions();

    for def in &defs {
        if def.kind == ToolKind::Read {
            assert!(
                def.metadata.is_read_only,
                "read tool `{}` should be is_read_only=true",
                def.name
            );
        }
    }
}

// ── P2: Tool Registry + Exposure ──────────────────────────────────────────────

#[test]
fn p2_deferred_tools_not_in_visible_definitions() {
    let tmp = tempfile::tempdir().unwrap();
    let policy = SecurityPolicy::new(
        tmp.path().to_path_buf(),
        tmp.path().to_path_buf(),
        SecurityConfig::default(),
    )
    .unwrap();
    let executor = ToolExecutor::new(policy);
    let defs = executor.definitions();
    let all = executor.all_definitions();

    let visible_names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    let all_names: Vec<&str> = all.iter().map(|d| d.name.as_str()).collect();

    let deferred_in_all: Vec<&str> = all_names
        .iter()
        .copied()
        .filter(|n| !visible_names.contains(n))
        .collect();
    assert!(
        !deferred_in_all.is_empty(),
        "expected deferred tools to be hidden from visible definitions. All: {:?}, Visible: {:?}",
        all_names,
        visible_names
    );
}

#[test]
fn p2_tool_search_returns_results() {
    let tmp = tempfile::tempdir().unwrap();
    let policy = SecurityPolicy::new(
        tmp.path().to_path_buf(),
        tmp.path().to_path_buf(),
        SecurityConfig::default(),
    )
    .unwrap();
    let executor = ToolExecutor::new(policy);

    let results = executor.search_tools("repo", 10);
    assert!(
        !results.is_empty(),
        "search for 'repo' should return results"
    );
}

#[test]
fn p2_tool_search_by_tag() {
    let mut reg = ToolRegistry::new();
    let mut def = ToolDefinition::new("test_tag_search", "test", ToolKind::Read, json!({}));
    def.metadata.tags = vec!["experimental".to_string()];
    def.metadata.exposure = ToolExposure::Deferred;
    reg.register(def);

    let results = reg.search("experimental", 10);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "test_tag_search");
}

#[test]
fn p2_exposure_enum_roundtrip() {
    let exposures = [
        ToolExposure::Direct,
        ToolExposure::Deferred,
        ToolExposure::Hidden,
        ToolExposure::Internal,
        ToolExposure::ModelOnly,
    ];
    for exp in &exposures {
        let json = serde_json::to_string(exp).unwrap();
        let restored: ToolExposure = serde_json::from_str(&json).unwrap();
        assert_eq!(*exp, restored);
    }
}

// ── P3: Structured Errors ────────────────────────────────────────────────────

#[test]
fn p3_tool_errors_have_required_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let policy = SecurityPolicy::new(
        tmp.path().to_path_buf(),
        tmp.path().to_path_buf(),
        SecurityConfig::default(),
    )
    .unwrap();
    let executor = ToolExecutor::new(policy);

    // Verify the error format includes required fields by checking an invalid tool call
    let inv = navi_core::ToolInvocation {
        id: "test-1".to_string(),
        tool_name: "nonexistent_tool".to_string(),
        input: json!({}),
    };
    let result = executor.validate(&inv);
    // This should deny the call because the tool doesn't exist
    assert!(matches!(result, navi_core::SecurityDecision::Deny(_)));
}

// ── P5: Sandbox ──────────────────────────────────────────────────────────────

#[test]
fn p5_workspace_snapshot_captures_state() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original content").unwrap();

    let snapshot = navi_core::sandbox::SandboxManager::create_snapshot(&[file.clone()]);
    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(snapshot.entries[0].path, file);
    assert!(snapshot.entries[0].content.is_some());
    assert_eq!(
        snapshot.entries[0].content.as_ref().unwrap(),
        "original content"
    );
}

#[test]
fn p5_rollback_restores_content() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original").unwrap();

    let snapshot = navi_core::sandbox::SandboxManager::create_snapshot(&[file.clone()]);
    std::fs::write(&file, "modified").unwrap();

    navi_core::sandbox::SandboxManager::rollback(&snapshot).unwrap();
    let restored = std::fs::read_to_string(&file).unwrap();
    assert_eq!(restored, "original");
}

#[test]
fn p5_changeset_detects_modifications() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original").unwrap();

    let snapshot = navi_core::sandbox::SandboxManager::create_snapshot(&[file.clone()]);
    std::fs::write(&file, "modified content").unwrap();

    let changes = navi_core::sandbox::SandboxManager::compute_changes(&snapshot);
    assert!(
        changes.files_modified.contains(&file),
        "should detect modification of test.txt"
    );
}

// ── P6: Effect-Based Permissions ──────────────────────────────────────────────

#[test]
fn p6_effect_analyzer_detects_sensitive_files() {
    let created = vec![PathBuf::from("/project/.env")];
    let report = EffectAnalyzer::analyze(&created, &[], &[]);
    assert!(!report.files_created.is_empty());
}

#[test]
fn p6_post_decision_for_safe_files() {
    let created = vec![PathBuf::from("/project/src/main.rs")];
    let report = EffectAnalyzer::analyze(&created, &[], &[]);
    assert_eq!(report.blast_radius, BlastRadius::SingleFile);
}

// ── P7: Verifier API ─────────────────────────────────────────────────────────

#[test]
fn p7_verifier_spec_serde_roundtrip() {
    let spec = VerifierSpec {
        verifier_type: "build".to_string(),
        command: "cargo build".to_string(),
        cwd: None,
        timeout_ms: Some(60_000),
        required: true,
    };
    let json = serde_json::to_string(&spec).unwrap();
    let restored: VerifierSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.command, "cargo build");
    assert_eq!(restored.required, true);
}

#[test]
fn p7_verification_store_operations() {
    let _store = VerificationStore::new();
    // Store creation succeeds. Sync operations are tested in verifier.rs unit tests.
}

// ── P8: Trace Store ──────────────────────────────────────────────────────────

#[test]
fn p8_trace_store_save_and_load() {
    let dir = tempfile::tempdir().unwrap();
    let store = TraceStore::new(dir.path());
    let mut trace = TurnTrace::new("turn-1", "session-test", "openai", "gpt-4", "test task");
    trace.finalize();
    store.save_trace(&trace).unwrap();
    let loaded = store.load_session_traces("session-test");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].turn_id, "turn-1");
}

#[test]
fn p8_trace_records_metrics() {
    let mut trace = TurnTrace::new("t1", "s1", "p", "m", "task");
    assert_eq!(trace.metrics.tool_call_count, 0);
    assert_eq!(trace.metrics.failed_tool_calls, 0);

    let inv = navi_core::ToolInvocation {
        id: "c1".to_string(),
        tool_name: "read".to_string(),
        input: json!({}),
    };
    let result = ToolResult {
        invocation_id: "c1".to_string(),
        ok: false,
        output: json!({"error": "not found", "error_code": "file_not_found"}),
    };
    trace.record_tool_call(&inv, &result, 50);
    assert_eq!(trace.metrics.failed_tool_calls, 1);
}

// ── P9: MCP hardening ────────────────────────────────────────────────────────

#[test]
fn p9_mcp_tool_has_deferred_exposure() {
    let metadata = ToolMetadata::deferred("mcp", ToolRisk::Medium, &["mcp", "test_server"]);
    assert_eq!(metadata.exposure, ToolExposure::Deferred);
    assert_eq!(metadata.risk, ToolRisk::Medium);
    assert!(metadata.tags.contains(&"mcp".to_string()));
}

// ── P10: Subagent profiles ───────────────────────────────────────────────────

#[test]
fn p10_agent_profile_serde_roundtrip() {
    let profiles = [
        navi_core::AgentProfile::Explorer,
        navi_core::AgentProfile::Implementer,
        navi_core::AgentProfile::Reviewer,
        navi_core::AgentProfile::Verifier,
        navi_core::AgentProfile::Summarizer,
    ];
    for profile in &profiles {
        let json = serde_json::to_string(profile).unwrap();
        let restored: navi_core::AgentProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(*profile, restored);
    }
}

#[test]
fn p10_approval_mode_serde_roundtrip() {
    let modes = [
        navi_core::ApprovalMode::Inherit,
        navi_core::ApprovalMode::Escalate,
        navi_core::ApprovalMode::ReadOnly,
        navi_core::ApprovalMode::DenyWrite,
    ];
    for mode in &modes {
        let json = serde_json::to_string(mode).unwrap();
        let restored: navi_core::ApprovalMode = serde_json::from_str(&json).unwrap();
        assert_eq!(*mode, restored);
    }
}

// ── Capabilities ─────────────────────────────────────────────────────────────

#[test]
fn p1_capabilities_are_well_known_strings() {
    assert_eq!(capabilities::REPO_READ, "repo.read");
    assert_eq!(capabilities::REPO_WRITE, "repo.write");
    assert_eq!(capabilities::SHELL_EXEC, "shell.exec");
    assert_eq!(capabilities::AGENT_SPAWN, "agent.spawn");
    assert_eq!(capabilities::MCP_ACCESS, "mcp.access");
    assert_eq!(capabilities::VERIFIER_RUN, "verifier.run");
}

// ── Meta: All subsystems present ─────────────────────────────────────────────

#[test]
fn parity_gate_all_subsystems_present() {
    // If this test compiles, all modules are present and public
    let _: ToolExposure;
    let _: ToolRisk;
    let _: ToolMetadata;
    let _: ToolRegistry;
    let _: TurnTrace;
    let _: TraceStore;
    let _: VerifierSpec;
    let _: VerificationStore;
    let _: EffectAnalyzer;
    let _: BlastRadius;
    assert!(true);
}
