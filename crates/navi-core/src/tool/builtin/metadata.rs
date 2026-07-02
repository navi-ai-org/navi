use crate::tool::{ToolKind, ToolMetadata};

/// Returns the canonical metadata for a builtin tool by name and kind.
///
/// Central registry for all builtin tool metadata. Adding a new tool to
/// `register_builtin_tools` should include a corresponding entry here.
pub fn builtin_metadata(name: &str, kind: ToolKind) -> ToolMetadata {
    // Use the lookup table first; fall back to kind-based defaults.
    LOOKUP
        .get(name)
        .cloned()
        .unwrap_or_else(|| kind_defaults(kind, name))
}

/// Kind-based default metadata when no per-tool entry exists.
fn kind_defaults(kind: ToolKind, _name: &str) -> ToolMetadata {
    match kind {
        ToolKind::Read => ToolMetadata {
            namespace: "repo".to_string(),
            risk: crate::tool::ToolRisk::Low,
            is_read_only: true,
            is_concurrency_safe: true,
            capabilities: vec!["repo.read".to_string()],
            tags: vec!["read".to_string()],
            ..ToolMetadata::default()
        },
        ToolKind::Write => ToolMetadata {
            namespace: "repo".to_string(),
            risk: crate::tool::ToolRisk::Medium,
            is_read_only: false,
            is_concurrency_safe: false,
            supports_rollback: true,
            capabilities: vec!["repo.write".to_string()],
            tags: vec!["write".to_string()],
            ..ToolMetadata::default()
        },
        ToolKind::Command => ToolMetadata {
            namespace: "process".to_string(),
            risk: crate::tool::ToolRisk::High,
            is_read_only: false,
            is_concurrency_safe: false,
            capabilities: vec!["shell.exec".to_string()],
            tags: vec!["command".to_string()],
            ..ToolMetadata::default()
        },
        ToolKind::Custom => ToolMetadata {
            namespace: "custom".to_string(),
            risk: crate::tool::ToolRisk::Medium,
            tags: vec!["custom".to_string()],
            ..ToolMetadata::default()
        },
    }
}

use std::sync::LazyLock;

/// Per-tool metadata overrides for builtin tools.
static LOOKUP: LazyLock<std::collections::HashMap<&'static str, ToolMetadata>> =
    LazyLock::new(|| {
        // Using phf would be ideal but adds a dependency. Build at runtime instead.
        let mut map = std::collections::HashMap::new();
        insert(
            &mut map,
            "read",
            ToolMetadata::reader("file", &["read", "file"]),
        );
        insert(
            &mut map,
            "read_file",
            ToolMetadata::reader("file", &["read", "file", "alias"]),
        );
        insert(
            &mut map,
            "view_file",
            ToolMetadata::reader("file", &["read", "file", "alias"]),
        );
        insert(
            &mut map,
            "load_skill",
            ToolMetadata::reader("skill", &["read", "skill", "instructions"]),
        );
        insert(
            &mut map,
            "search",
            ToolMetadata {
                namespace: "repo".to_string(),
                risk: crate::tool::ToolRisk::Low,
                is_read_only: true,
                is_concurrency_safe: true,
                exposure: crate::tool::ToolExposure::Direct,
                capabilities: vec!["repo.read".to_string()],
                tags: vec!["search", "grep", "find", "file"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "grep",
            ToolMetadata {
                namespace: "repo".to_string(),
                risk: crate::tool::ToolRisk::Low,
                is_read_only: true,
                is_concurrency_safe: true,
                exposure: crate::tool::ToolExposure::Direct,
                capabilities: vec!["repo.read".to_string()],
                tags: vec!["grep", "search", "text", "alias"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "fs_browser",
            ToolMetadata {
                namespace: "repo".to_string(),
                risk: crate::tool::ToolRisk::Low,
                is_read_only: true,
                is_concurrency_safe: true,
                exposure: crate::tool::ToolExposure::Direct,
                capabilities: vec!["repo.read".to_string()],
                tags: vec!["filesystem", "list", "tree", "find", "stat", "alias"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "list_dir",
            ToolMetadata::reader("repo", &["filesystem", "list", "directory"]),
        );
        insert(
            &mut map,
            "glob",
            ToolMetadata::reader("repo", &["filesystem", "glob", "find"]),
        );
        insert(
            &mut map,
            "write",
            ToolMetadata::writer("file", &["write", "edit", "patch"]),
        );
        insert(
            &mut map,
            "write_file",
            ToolMetadata::writer("file", &["write", "file", "alias"]),
        );
        insert(
            &mut map,
            "apply_patch",
            ToolMetadata::writer("file", &["patch", "diff", "edit", "alias"]),
        );
        insert(
            &mut map,
            "process",
            ToolMetadata {
                namespace: "process".to_string(),
                risk: crate::tool::ToolRisk::High,
                is_read_only: false,
                is_concurrency_safe: false,
                exposure: crate::tool::ToolExposure::Direct,
                capabilities: vec!["shell.exec".to_string(), "process.manage".to_string()],
                tags: vec!["process", "command", "exec"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                max_output_bytes: Some(65536),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "bash",
            ToolMetadata {
                namespace: "process".to_string(),
                risk: crate::tool::ToolRisk::High,
                is_read_only: false,
                is_concurrency_safe: false,
                exposure: crate::tool::ToolExposure::Direct,
                capabilities: vec!["shell.exec".to_string(), "shell.bash".to_string()],
                tags: vec!["shell", "bash", "command"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                max_output_bytes: Some(65536),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "code",
            ToolMetadata {
                namespace: "code".to_string(),
                risk: crate::tool::ToolRisk::Low,
                is_read_only: true,
                is_concurrency_safe: true,
                capabilities: vec!["repo.read".to_string(), "code.analyze".to_string()],
                tags: vec!["code", "symbols", "analysis"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "code_edit",
            ToolMetadata {
                namespace: "code".to_string(),
                risk: crate::tool::ToolRisk::Medium,
                is_read_only: false,
                is_concurrency_safe: false,
                supports_rollback: true,
                capabilities: vec!["repo.write".to_string(), "code.edit".to_string()],
                tags: vec!["code", "edit", "symbol"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "code_exec",
            ToolMetadata {
                namespace: "code".to_string(),
                risk: crate::tool::ToolRisk::High,
                is_read_only: false,
                is_concurrency_safe: false,
                supports_rollback: true,
                exposure: crate::tool::ToolExposure::Deferred,
                capabilities: vec![
                    "repo.read".to_string(),
                    "repo.write.src".to_string(),
                    "code.exec".to_string(),
                    "verifier.run".to_string(),
                ],
                tags: vec!["code", "exec", "sdk", "nested-tools", "verifier"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                max_output_bytes: Some(131072),
                ..ToolMetadata::default()
            },
        );
        for name in [
            "ast_search",
            "symbol_goto",
            "symbol_references",
            "dependency_graph_query",
            "test_discovery",
            "ownership_churn_query",
        ] {
            insert(
                &mut map,
                name,
                ToolMetadata {
                    namespace: "repo_intelligence".to_string(),
                    risk: crate::tool::ToolRisk::Low,
                    is_read_only: true,
                    is_concurrency_safe: true,
                    exposure: crate::tool::ToolExposure::Direct,
                    capabilities: vec!["repo.read".to_string(), "code.analyze".to_string()],
                    tags: vec!["ast", "symbol", "dependency", "test", "repo"]
                        .into_iter()
                        .map(|s| s.to_string())
                        .collect(),
                    max_output_bytes: Some(32768),
                    ..ToolMetadata::default()
                },
            );
        }
        insert(
            &mut map,
            "branch_race_start",
            ToolMetadata {
                namespace: "orchestration".to_string(),
                risk: crate::tool::ToolRisk::Medium,
                is_read_only: true,
                is_concurrency_safe: true,
                exposure: crate::tool::ToolExposure::Deferred,
                capabilities: vec!["orchestration.branch_race".to_string()],
                tags: vec!["branch", "race", "hypothesis", "verifier"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                max_output_bytes: Some(32768),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "question",
            ToolMetadata {
                namespace: "interactive".to_string(),
                risk: crate::tool::ToolRisk::Low,
                is_read_only: true,
                is_concurrency_safe: true,
                capabilities: vec!["interactive.ask".to_string()],
                tags: vec!["interactive", "question"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "plan",
            ToolMetadata {
                namespace: "plan".to_string(),
                risk: crate::tool::ToolRisk::Low,
                is_read_only: true,
                is_concurrency_safe: true,
                capabilities: vec!["plan.manage".to_string()],
                tags: vec!["plan", "checklist"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "init_session",
            ToolMetadata::reader("session", &["session", "init"]),
        );
        insert(
            &mut map,
            "mark_feature_done",
            ToolMetadata {
                namespace: "session".to_string(),
                risk: crate::tool::ToolRisk::Medium,
                is_read_only: false,
                is_concurrency_safe: false,
                capabilities: vec!["session.feature".to_string()],
                tags: vec!["session", "feature", "verify"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "current_time",
            ToolMetadata::reader("system", &["time", "clock"]),
        );
        insert(
            &mut map,
            "sleep",
            ToolMetadata::deferred("system", crate::tool::ToolRisk::Low, &["sleep", "delay"]),
        );
        insert(
            &mut map,
            "get_context_remaining",
            ToolMetadata::reader("system", &["context", "tokens"]),
        );
        insert(
            &mut map,
            "request_user_input",
            ToolMetadata::reader("interactive", &["input", "user"]),
        );
        insert(
            &mut map,
            "new_context_window",
            ToolMetadata {
                namespace: "system".to_string(),
                risk: crate::tool::ToolRisk::Low,
                is_read_only: false,
                is_concurrency_safe: true,
                capabilities: vec!["context.compact".to_string()],
                tags: vec!["context", "compact"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "view_image",
            ToolMetadata::reader("file", &["image", "view"]),
        );
        insert(
            &mut map,
            "inspect_image",
            ToolMetadata::reader("file", &["image", "inspect"]),
        );
        insert(
            &mut map,
            "append_note",
            ToolMetadata {
                namespace: "memory".to_string(),
                risk: crate::tool::ToolRisk::Low,
                is_read_only: false,
                is_concurrency_safe: true,
                capabilities: vec!["memory.write".to_string()],
                tags: vec!["memory", "note"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "history_ops",
            ToolMetadata {
                namespace: "memory".to_string(),
                risk: crate::tool::ToolRisk::Low,
                is_read_only: true,
                is_concurrency_safe: true,
                capabilities: vec!["memory.read".to_string()],
                tags: vec!["memory", "history"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "package_manager",
            ToolMetadata {
                namespace: "package".to_string(),
                risk: crate::tool::ToolRisk::High,
                is_read_only: false,
                is_concurrency_safe: false,
                capabilities: vec![
                    "network.package".to_string(),
                    "repo.write.lockfile".to_string(),
                ],
                tags: vec!["package", "dependency"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "workflow",
            ToolMetadata {
                namespace: "orchestration".to_string(),
                risk: crate::tool::ToolRisk::Medium,
                is_read_only: false,
                is_concurrency_safe: false,
                capabilities: vec!["orchestration.workflow".to_string()],
                tags: vec!["workflow", "script"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "repo_explore",
            ToolMetadata::deferred(
                "repo",
                crate::tool::ToolRisk::Low,
                &["explore", "structure"],
            ),
        );
        insert(
            &mut map,
            "subagent",
            ToolMetadata {
                namespace: "agent".to_string(),
                risk: crate::tool::ToolRisk::High,
                is_read_only: false,
                is_concurrency_safe: false,
                capabilities: vec!["agent.spawn".to_string()],
                tags: vec!["agent", "subprocess"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "wait",
            ToolMetadata::deferred("file", crate::tool::ToolRisk::Low, &["lock", "wait"]),
        );
        insert(
            &mut map,
            "runtime_info",
            ToolMetadata {
                namespace: "system".to_string(),
                risk: crate::tool::ToolRisk::Low,
                is_read_only: true,
                is_concurrency_safe: true,
                exposure: crate::tool::ToolExposure::Internal,
                capabilities: vec!["system.info".to_string()],
                tags: vec!["runtime", "config"]
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..ToolMetadata::default()
            },
        );
        insert(
            &mut map,
            "sandbox",
            ToolMetadata::deferred(
                "sandbox",
                crate::tool::ToolRisk::Medium,
                &["sandbox", "snapshot", "rollback"],
            ),
        );
        insert(
            &mut map,
            "tool_search",
            ToolMetadata::reader("system", &["search", "discovery", "tools"])
                .with_capability(&["tool.discovery"]),
        );
        insert(
            &mut map,
            "verifier",
            ToolMetadata::deferred(
                "verifier",
                crate::tool::ToolRisk::Medium,
                &["verifier", "build", "test", "check"],
            )
            .with_capability(&["verifier.run"]),
        );
        map
    });

fn insert(
    map: &mut std::collections::HashMap<&'static str, ToolMetadata>,
    name: &'static str,
    meta: ToolMetadata,
) {
    map.insert(name, meta);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_builtin_tools_have_metadata() {
        let tool_names = [
            "read",
            "read_file",
            "search",
            "grep",
            "fs_browser",
            "list_dir",
            "glob",
            "write",
            "write_file",
            "apply_patch",
            "bash",
            "process",
            "code",
            "code_edit",
            "code_exec",
            "ast_search",
            "symbol_goto",
            "symbol_references",
            "dependency_graph_query",
            "test_discovery",
            "ownership_churn_query",
            "branch_race_start",
            "question",
            "plan",
            "init_session",
            "mark_feature_done",
            "current_time",
            "sleep",
            "get_context_remaining",
            "request_user_input",
            "new_context_window",
            "view_image",
            "inspect_image",
            "append_note",
            "history_ops",
            "package_manager",
            "workflow",
            "repo_explore",
            "subagent",
            "wait",
            "runtime_info",
            "sandbox",
            "tool_search",
            "verifier",
        ];
        for name in &tool_names {
            let meta = builtin_metadata(name, ToolKind::Read);
            assert!(!meta.namespace.is_empty(), "missing namespace for {name}");
        }
    }

    #[test]
    fn metadata_for_unknown_tool_falls_back_to_kind_defaults() {
        let meta = builtin_metadata("nonexistent_tool", ToolKind::Read);
        assert!(meta.is_read_only);
        assert!(meta.is_concurrency_safe);
        assert_eq!(meta.risk, crate::tool::ToolRisk::Low);
        assert_eq!(meta.namespace, "repo");
    }

    #[test]
    fn write_tool_metadata_has_writer_defaults() {
        let meta = builtin_metadata("write", ToolKind::Write);
        assert!(!meta.is_read_only);
        assert!(meta.supports_rollback);
        assert_eq!(meta.namespace, "file");

        let patch = builtin_metadata("apply_patch", ToolKind::Write);
        assert!(!patch.is_read_only);
        assert!(patch.supports_rollback);
    }

    #[test]
    fn bash_has_high_risk_and_max_output() {
        let meta = builtin_metadata("bash", ToolKind::Command);
        assert_eq!(meta.risk, crate::tool::ToolRisk::High);
        assert_eq!(meta.max_output_bytes, Some(65536));
    }

    #[test]
    fn deferred_tools_not_direct() {
        let meta = builtin_metadata("sleep", ToolKind::Command);
        assert_eq!(meta.exposure, crate::tool::ToolExposure::Deferred);

        let meta2 = builtin_metadata("repo_explore", ToolKind::Read);
        assert_eq!(meta2.exposure, crate::tool::ToolExposure::Deferred);

        let meta3 = builtin_metadata("wait", ToolKind::Read);
        assert_eq!(meta3.exposure, crate::tool::ToolExposure::Deferred);
    }

    #[test]
    fn internal_tools_have_internal_exposure() {
        let meta = builtin_metadata("runtime_info", ToolKind::Read);
        assert_eq!(meta.exposure, crate::tool::ToolExposure::Internal);
    }

    #[test]
    fn tool_search_is_direct_so_deferred_tools_are_discoverable() {
        let meta = builtin_metadata("tool_search", ToolKind::Read);
        assert_eq!(meta.exposure, crate::tool::ToolExposure::Direct);
        assert!(meta.is_read_only);
    }
}
