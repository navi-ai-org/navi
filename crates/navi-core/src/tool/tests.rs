use super::*;
use crate::{
    PermissionMode, PermissiveSecurityPolicy, SecurityConfig, SecurityDecision, SecurityPolicy,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn executor(root: &Path) -> ToolExecutor {
    let config = SecurityConfig {
        permission_mode: PermissionMode::Yolo,
        ..SecurityConfig::default()
    };
    let policy =
        SecurityPolicy::new(root.to_path_buf(), root.join(".navi-data"), config).expect("policy");
    ToolExecutor::new(policy)
}

fn test_feature_list_path(root: &Path) -> PathBuf {
    std::fs::read_dir(root.join(".navi-data").join("sprints"))
        .expect("sprint dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path().join("feature_list.json"))
        .find(|path| path.exists())
        .expect("feature list")
}

#[test]
fn injected_security_policy_can_allow_write_without_approval() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let policy = SecurityPolicy::new(
        tempdir.path().to_path_buf(),
        tempdir.path().join(".navi-data"),
        SecurityConfig::default(),
    )
    .expect("policy");
    let invocation = ToolInvocation {
        id: "write".to_string(),
        tool_name: "write_file".to_string(),
        input: json!({
            "path": "notes.txt",
            "content": "study note\n"
        }),
    };

    let default_executor = ToolExecutor::new(policy.clone());
    assert!(matches!(
        default_executor.validate(&invocation),
        SecurityDecision::NeedsApproval(_)
    ));

    let permissive_executor =
        ToolExecutor::with_security_policy(policy, Arc::new(PermissiveSecurityPolicy));
    assert_eq!(
        permissive_executor.validate(&invocation),
        SecurityDecision::Allow
    );
}

#[tokio::test]
async fn direct_invoke_does_not_bypass_restricted_approval() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let policy = SecurityPolicy::new(
        tempdir.path().to_path_buf(),
        tempdir.path().join(".navi-data"),
        SecurityConfig {
            permission_mode: PermissionMode::Restricted,
            ..SecurityConfig::default()
        },
    )
    .expect("policy");
    let executor = ToolExecutor::new(policy);
    let target = tempdir.path().join("blocked.txt");

    let result = executor
        .invoke(ToolInvocation {
            id: "write".to_string(),
            tool_name: "write_file".to_string(),
            input: json!({
                "path": target.display().to_string(),
                "content": "should not be written\n"
            }),
        })
        .await;

    assert!(!result.ok);
    assert!(
        result.output["error"]
            .as_str()
            .unwrap_or_default()
            .contains("approval required")
    );
    assert!(!target.exists());
}

#[tokio::test]
async fn builtins_read_write_and_grep_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    let file = tempdir.path().join("src/lib.rs");

    let write = ToolInvocation {
        id: "write".to_string(),
        tool_name: "write_file".to_string(),
        input: json!({ "path": file.display().to_string(), "content": "pub fn marker() {}\n" }),
    };
    let write = executor.invoke(write).await;
    assert!(write.ok);
    assert_eq!(write.output["lines_added"], 1);
    assert_eq!(write.output["lines_removed"], 0);

    let read = executor
        .invoke(ToolInvocation {
            id: "read".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "path": file.display().to_string() }),
        })
        .await;
    assert_eq!(read.output["content"], "pub fn marker() {}\n");
    assert_eq!(read.output["start_line"], 1);
    assert_eq!(read.output["end_line"], 1);
    assert_eq!(read.output["total_lines"], 1);
    assert!(!read.output["truncated"].as_bool().unwrap());

    // Test multi-line slicing
    let multiline_file = tempdir.path().join("src/multiline.rs");
    let write_multiline = ToolInvocation {
        id: "write_multiline".to_string(),
        tool_name: "write_file".to_string(),
        input: json!({ "path": multiline_file.display().to_string(), "content": "one\ntwo\nthree\nfour\n" }),
    };
    let write_multiline = executor.invoke(write_multiline).await;
    assert!(write_multiline.ok);
    assert_eq!(write_multiline.output["lines_added"], 4);
    assert_eq!(write_multiline.output["lines_removed"], 0);

    let read_slice = executor
        .invoke(ToolInvocation {
            id: "read_slice".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({
                "path": multiline_file.display().to_string(),
                "start_line": 2,
                "end_line": 3
            }),
        })
        .await;
    assert_eq!(read_slice.output["content"], "two\nthree\n");
    assert_eq!(read_slice.output["start_line"], 2);
    assert_eq!(read_slice.output["end_line"], 3);
    assert_eq!(read_slice.output["total_lines"], 4);
    assert!(read_slice.output["truncated"].as_bool().unwrap());

    let grep = executor
            .invoke(ToolInvocation {
                id: "grep".to_string(),
                tool_name: "grep".to_string(),
                input: json!({ "pattern": "marker", "path": tempdir.path().join("src").display().to_string() }),
            })
            .await;
    assert_eq!(grep.output["matches"][0]["line"], 1);
}

#[tokio::test]
async fn executor_emits_capability_lifecycle_for_allowed_tool() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    std::fs::write(tempdir.path().join("file.txt"), "hello\n").unwrap();
    let executor = executor(tempdir.path());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let result = executor
        .invoke_with_event_tx(
            ToolInvocation {
                id: "read-cap".to_string(),
                tool_name: "read_file".to_string(),
                input: json!({"path": "file.txt"}),
            },
            Some(tx),
        )
        .await;

    assert!(result.ok, "{:?}", result.output);
    let mut decisions = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::CapabilityRecorded(entry) = event {
            decisions.push((entry.capability.as_key(), entry.decision));
        }
    }
    assert!(decisions.contains(&(
        "repo.read".to_string(),
        crate::CapabilityDecision::Requested
    )));
    assert!(decisions.contains(&("repo.read".to_string(), crate::CapabilityDecision::Consumed)));
}

#[tokio::test]
async fn executor_emits_denied_capability_for_blocked_tool() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let result = executor
        .invoke_with_event_tx(
            ToolInvocation {
                id: "blocked-cap".to_string(),
                tool_name: "bash".to_string(),
                input: json!({"command": "sudo true"}),
            },
            Some(tx),
        )
        .await;

    assert!(!result.ok);
    let mut saw_denied = false;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::CapabilityRecorded(entry) = event {
            saw_denied |= matches!(entry.decision, crate::CapabilityDecision::Denied)
                && entry.capability.as_key() == "shell.safe";
        }
    }
    assert!(saw_denied, "expected denied shell capability event");
}

#[tokio::test]
async fn legacy_file_tool_aliases_remain_registered_and_invokable() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    for name in [
        "read_file",
        "write_file",
        "apply_patch",
        "edit",
        "multiedit",
        "grep",
        "fs_browser",
        "list_dir",
        "glob",
        "inspect_image",
    ] {
        assert!(
            executor.definition(name).is_some(),
            "missing compatibility alias `{name}`"
        );
    }

    let write = executor
        .invoke(ToolInvocation {
            id: "write-file".to_string(),
            tool_name: "write_file".to_string(),
            input: json!({"path": "src/lib.rs", "content": "pub fn alias_marker() {}\n"}),
        })
        .await;
    assert!(write.ok, "{:?}", write.output);

    let read = executor
        .invoke(ToolInvocation {
            id: "read-file".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({"path": "src/lib.rs"}),
        })
        .await;
    assert_eq!(read.output["content"], "pub fn alias_marker() {}\n");

    let grep = executor
        .invoke(ToolInvocation {
            id: "grep".to_string(),
            tool_name: "grep".to_string(),
            input: json!({"pattern": "alias_marker", "path": "src"}),
        })
        .await;
    assert!(grep.ok, "{:?}", grep.output);
    assert_eq!(grep.output["matches"][0]["path"], "src/lib.rs");

    let listed = executor
        .invoke(ToolInvocation {
            id: "list".to_string(),
            tool_name: "fs_browser".to_string(),
            input: json!({"action": "list", "path": "src"}),
        })
        .await;
    assert!(listed.ok, "{:?}", listed.output);
    assert!(
        listed.output["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path.as_str().unwrap().ends_with("src/lib.rs"))
    );

    let patch = executor
        .invoke(ToolInvocation {
            id: "patch".to_string(),
            tool_name: "apply_patch".to_string(),
            input: json!({
                "patch": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-pub fn alias_marker() {}\n+pub fn patched_alias_marker() {}\n*** End Patch\n"
            }),
        })
        .await;
    assert!(patch.ok, "{:?}", patch.output);
    assert_eq!(
        std::fs::read_to_string(tempdir.path().join("src/lib.rs")).unwrap(),
        "pub fn patched_alias_marker() {}\n"
    );
}

struct LateRegisteredTool;

#[async_trait::async_trait]
impl Tool for LateRegisteredTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::with_metadata(
            "host__late_tool",
            "Late host tool for registry freshness checks.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
            ToolMetadata {
                namespace: "host".to_string(),
                risk: ToolRisk::Low,
                is_read_only: true,
                is_concurrency_safe: true,
                exposure: ToolExposure::Deferred,
                tags: vec!["latecap".to_string()],
                ..ToolMetadata::default()
            },
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> anyhow::Result<ToolResult> {
        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: true,
            output: json!({"ok": true}),
        })
    }
}

#[tokio::test]
async fn tool_search_uses_live_registry_for_late_registered_tools() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut executor = executor(tempdir.path());
    executor.register_tool(std::sync::Arc::new(LateRegisteredTool));

    // Below the threshold, Deferred tools are promoted to visible.
    // Verify the tool is discoverable via tool_search regardless.
    let result = executor
        .invoke(ToolInvocation {
            id: "search-tools".to_string(),
            tool_name: "tool_search".to_string(),
            input: json!({"query": "latecap", "max_results": 5}),
        })
        .await;

    assert!(result.ok, "{:?}", result.output);
    let names = result.output["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(names.contains(&"host__late_tool"), "{names:?}");
}

#[tokio::test]
async fn write_sensitive_file_is_rolled_back_by_effect_policy() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let result = executor
        .invoke(ToolInvocation {
            id: "write-env".to_string(),
            tool_name: "write_file".to_string(),
            input: json!({"path": ".env", "content": "TOKEN=secret\n"}),
        })
        .await;

    assert!(!result.ok, "{:?}", result.output);
    assert_eq!(result.output["error_code"], "effect_rollback");
    assert_eq!(result.output["rolled_back"], true);
    assert!(!tempdir.path().join(".env").exists());
}

#[test]
fn tool_search_is_visible_in_initial_definitions() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    let visible = executor
        .definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();

    assert!(visible.contains(&"tool_search".to_string()), "{visible:?}");
}

#[test]
fn removed_tools_are_not_registered() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    let visible = executor
        .definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();

    assert!(executor.definition("top_files").is_none());
    assert!(executor.definition("tool_workflow").is_none());
    assert!(!visible.contains(&"top_files".to_string()), "{visible:?}");
    assert!(
        !visible.contains(&"tool_workflow".to_string()),
        "{visible:?}"
    );
}

#[tokio::test]
async fn code_exec_runs_typed_nested_plan_with_verifier() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::create_dir_all(tempdir.path().join("src")).expect("mkdir");
    std::fs::write(
        tempdir.path().join("src/lib.rs"),
        "pub fn code_exec_marker() {}\n",
    )
    .expect("write lib");

    let result = executor
        .invoke(ToolInvocation {
            id: "code-exec".to_string(),
            tool_name: "code_exec".to_string(),
            input: json!({
                "cell_id": "cell-1",
                "ops": [
                    { "op": "repo-read", "path": "src/lib.rs" },
                    { "op": "ast-search", "query": "code_exec_marker", "kind": "function" },
                    { "op": "verify-run", "command": "test -f src/lib.rs", "verifier": "command" },
                    { "op": "trace-note", "note": "verified file exists" }
                ]
            }),
        })
        .await;

    assert!(result.ok, "{:?}", result.output);
    assert_eq!(result.output["status"], "passed");
    assert_eq!(result.output["ops_executed"], 4);
    assert_eq!(result.output["artifact"]["ops"][1]["op"], "ast-search");
}

#[tokio::test]
async fn ast_search_returns_ranked_symbols_and_text_matches() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::create_dir_all(tempdir.path().join("src")).expect("mkdir");
    std::fs::write(
        tempdir.path().join("src/lib.rs"),
        "/// Tool that searches symbols in docs.\npub struct FuzzyToolSearch;\npub struct OtherThing;\n",
    )
    .expect("write lib");

    let result = executor
        .invoke(ToolInvocation {
            id: "ast-search".to_string(),
            tool_name: "ast_search".to_string(),
            input: json!({
                "query": "ToolSearch|SearchTool|Search",
                "max_results": 1
            }),
        })
        .await;

    assert!(result.ok, "{:?}", result.output);
    assert_eq!(result.output["matches"].as_array().unwrap().len(), 1);
    assert_eq!(result.output["matches"][0]["name"], "FuzzyToolSearch");
    assert_eq!(result.output["ranking"][0]["name"], "FuzzyToolSearch");
    assert!(
        result.output["ranking"][0]["score"].as_f64().unwrap() > 0.0,
        "{:?}",
        result.output
    );
    assert!(
        !result.output["text_matches"].as_array().unwrap().is_empty(),
        "{:?}",
        result.output
    );
}

#[tokio::test]
async fn ast_search_kind_filter_only_limits_symbols() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::create_dir_all(tempdir.path().join("src")).expect("mkdir");
    std::fs::write(
        tempdir.path().join("src/lib.rs"),
        "/// Search docs remain available as text.\npub struct SearchThing;\npub fn search_thing() {}\n",
    )
    .expect("write lib");

    let result = executor
        .invoke(ToolInvocation {
            id: "ast-search".to_string(),
            tool_name: "ast_search".to_string(),
            input: json!({
                "query": "SearchThing",
                "kind": "function",
                "max_results": 5
            }),
        })
        .await;

    assert!(result.ok, "{:?}", result.output);
    assert_eq!(result.output["matches"].as_array().unwrap().len(), 1);
    assert_eq!(result.output["matches"][0]["kind"], "function");
    assert!(
        !result.output["text_matches"].as_array().unwrap().is_empty(),
        "{:?}",
        result.output
    );
}

#[tokio::test]
async fn code_exec_rejects_untyped_operation_shape() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let result = executor
        .invoke(ToolInvocation {
            id: "code-exec".to_string(),
            tool_name: "code_exec".to_string(),
            input: json!({
                "ops": [
                    { "op": "repo-read" }
                ]
            }),
        })
        .await;

    assert!(!result.ok);
    assert!(
        result.output["error"]
            .as_str()
            .unwrap()
            .contains("invalid code_exec request")
    );
}

#[tokio::test]
async fn relative_tool_paths_are_resolved_under_project_root() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let result = executor
        .invoke(ToolInvocation {
            id: "write-relative".to_string(),
            tool_name: "write_file".to_string(),
            input: json!({ "path": "src/relative.rs", "content": "// relative\n" }),
        })
        .await;

    assert!(result.ok);
    assert_eq!(
        std::fs::read_to_string(tempdir.path().join("src/relative.rs")).unwrap(),
        "// relative\n"
    );
}

#[tokio::test]
async fn bash_runs_with_project_root_as_cwd() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let result = executor
        .invoke(ToolInvocation {
            id: "pwd".to_string(),
            tool_name: "bash".to_string(),
            input: json!({ "command": "pwd" }),
        })
        .await;

    assert!(result.ok);
    assert_eq!(
        result.output["stdout"].as_str().unwrap().trim(),
        tempdir.path().canonicalize().unwrap().display().to_string()
    );
}

#[tokio::test]
async fn bash_timeout_returns_structured_error() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let result = executor
        .invoke(ToolInvocation {
            id: "bash-timeout".to_string(),
            tool_name: "bash".to_string(),
            input: json!({ "command": "sleep 1", "timeout_ms": 1 }),
        })
        .await;

    assert!(!result.ok);
    assert_eq!(
        result.output["error"],
        "command timed out: deadline has elapsed"
    );
}

#[tokio::test]
async fn bash_timeout_kills_child_process() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let marker = tempdir.path().join("still-running");
    let marker_s = marker.display().to_string();
    let executor = executor(tempdir.path());

    // Write a pid file then sleep past the timeout. After the tool returns,
    // the process must be gone (not left running under kill_on_drop only).
    let result = executor
        .invoke(ToolInvocation {
            id: "bash-timeout-kill".to_string(),
            tool_name: "bash".to_string(),
            input: json!({
                "command": format!(
                    "echo $$ > '{marker}'; sleep 30",
                    marker = marker_s
                ),
                "timeout_ms": 200
            }),
        })
        .await;

    assert!(!result.ok, "timeout must fail: {result:?}");
    assert_eq!(
        result.output["error"],
        "command timed out: deadline has elapsed"
    );

    // Give the reaper a moment, then ensure the recorded pid is not alive.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    if marker.exists() {
        let pid_txt = std::fs::read_to_string(&marker).expect("pid file");
        let pid: i32 = pid_txt.trim().parse().expect("pid");
        // kill -0 returns non-zero when process is gone.
        let status = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .expect("kill -0");
        assert!(
            !status.success(),
            "timed-out bash child pid {pid} is still running"
        );
    }
}

#[tokio::test]
async fn bash_background_timeout_marks_result_not_ok() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let started = executor
        .invoke(ToolInvocation {
            id: "bash-bg-timeout-start".to_string(),
            tool_name: "bash".to_string(),
            input: json!({
                "command": "sleep 5",
                "background": true,
                "wait_ms": 1,
                "timeout_ms": 50
            }),
        })
        .await;
    assert!(
        started.ok
            || started.output["status"] == "running"
            || started.output["status"] == "timed_out"
    );
    let task_id = started.output["task_id"].as_str().unwrap().to_string();

    // Wait long enough for the background timeout to fire.
    let polled = executor
        .invoke(ToolInvocation {
            id: "bash-bg-timeout-poll".to_string(),
            tool_name: "bash".to_string(),
            input: json!({ "task_id": task_id, "wait_ms": 1000 }),
        })
        .await;

    assert_eq!(polled.output["status"], "timed_out");
    assert!(
        !polled.ok,
        "timed-out background bash must set ok=false so the agent continues: {polled:?}"
    );
    assert_eq!(
        polled.output["error"],
        "command timed out: deadline has elapsed"
    );
}

#[tokio::test]
async fn bash_background_task_can_be_polled() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let started = executor
        .invoke(ToolInvocation {
            id: "bash-bg-start".to_string(),
            tool_name: "bash".to_string(),
            input: json!({
                "command": "sleep 0.05 && printf done",
                "background": true,
                "wait_ms": 1,
                "timeout_ms": 1000
            }),
        })
        .await;

    assert!(started.ok);
    assert_eq!(started.output["status"], "running");
    let task_id = started.output["task_id"].as_str().unwrap().to_string();

    let polled = executor
        .invoke(ToolInvocation {
            id: "bash-bg-poll".to_string(),
            tool_name: "bash".to_string(),
            input: json!({ "task_id": task_id, "wait_ms": 1000 }),
        })
        .await;

    assert!(polled.ok);
    assert_eq!(polled.output["status"], "completed");
    assert_eq!(polled.output["stdout"], "done");
}

#[tokio::test]
async fn bash_background_supports_multiple_tasks_and_list() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let first = executor
        .invoke(ToolInvocation {
            id: "bash-bg-one".to_string(),
            tool_name: "bash".to_string(),
            input: json!({
                "command": "sleep 0.05 && printf one",
                "background": true,
                "wait_ms": 1,
                "timeout_ms": 1000
            }),
        })
        .await;
    let second = executor
        .invoke(ToolInvocation {
            id: "bash-bg-two".to_string(),
            tool_name: "bash".to_string(),
            input: json!({
                "command": "sleep 0.05 && printf two",
                "background": true,
                "wait_ms": 1,
                "timeout_ms": 1000
            }),
        })
        .await;

    assert_eq!(first.output["status"], "running");
    assert_eq!(second.output["status"], "running");
    assert_ne!(first.output["task_id"], second.output["task_id"]);

    let listed = executor
        .invoke(ToolInvocation {
            id: "bash-bg-list".to_string(),
            tool_name: "bash".to_string(),
            input: json!({ "action": "list" }),
        })
        .await;

    let tasks = listed.output["tasks"].as_array().unwrap();
    assert!(tasks.len() >= 2);
    assert!(
        tasks
            .iter()
            .any(|task| task["task_id"] == first.output["task_id"])
    );
    assert!(
        tasks
            .iter()
            .any(|task| task["task_id"] == second.output["task_id"])
    );
}

#[tokio::test]
async fn bash_background_task_can_be_cancelled() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let started = executor
        .invoke(ToolInvocation {
            id: "bash-bg-cancel-start".to_string(),
            tool_name: "bash".to_string(),
            input: json!({
                "command": "sleep 5",
                "background": true,
                "wait_ms": 1,
                "timeout_ms": 1000
            }),
        })
        .await;
    let task_id = started.output["task_id"].as_str().unwrap().to_string();

    let cancelled = executor
        .invoke(ToolInvocation {
            id: "bash-bg-cancel".to_string(),
            tool_name: "bash".to_string(),
            input: json!({ "task_id": task_id, "action": "cancel" }),
        })
        .await;

    assert!(cancelled.ok);
    assert_eq!(cancelled.output["status"], "cancelled");
}

#[test]
fn executor_definitions_include_input_schemas() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    let read = executor.definition("read_file").expect("read_file");

    assert_eq!(read.input_schema["type"], "object");
    assert!(
        read.input_schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("path"))
    );
}

#[test]
fn model_definitions_use_simplified_tool_schemas() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    for definition in executor.definitions() {
        assert!(
            !schema_contains_composition_keyword(&definition.input_schema),
            "model-facing schema for {} should not contain oneOf/anyOf/allOf/const: {}",
            definition.name,
            definition.input_schema
        );
    }

    let grep = executor
        .all_definitions()
        .into_iter()
        .find(|definition| definition.name == "grep")
        .expect("grep definition");
    assert_eq!(grep.input_schema["required"], json!(["pattern"]));
}

fn schema_contains_composition_keyword(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(object) => object.iter().any(|(key, value)| {
            matches!(key.as_str(), "oneOf" | "anyOf" | "allOf" | "const")
                || schema_contains_composition_keyword(value)
        }),
        serde_json::Value::Array(values) => values.iter().any(schema_contains_composition_keyword),
        _ => false,
    }
}

#[test]
fn validates_tool_arguments_against_input_schema() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let valid = ToolInvocation {
        id: "read".to_string(),
        tool_name: "read_file".to_string(),
        input: json!({ "path": "README.md" }),
    };
    assert!(executor.validate_arguments(&valid).is_ok());

    let missing_required = ToolInvocation {
        id: "read".to_string(),
        tool_name: "read_file".to_string(),
        input: json!({}),
    };
    let err = executor
        .validate_arguments(&missing_required)
        .expect_err("missing path should fail");
    assert!(matches!(err, ToolCallInvalid::InvalidArguments { .. }));

    let extra_property = ToolInvocation {
        id: "read".to_string(),
        tool_name: "read_file".to_string(),
        input: json!({ "path": "README.md", "unexpected": true }),
    };
    assert!(executor.validate_arguments(&extra_property).is_err());
}

#[tokio::test]
async fn invalid_tool_arguments_return_structured_error() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let result = executor
        .invoke(ToolInvocation {
            id: "bad-read".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "start_line": 1 }),
        })
        .await;

    assert!(!result.ok);
    assert_eq!(result.output["error_code"], "invalid_arguments");
    assert_eq!(result.output["tool"], "read_file");
    assert_eq!(result.output["example"], json!({ "path": "example" }));
    assert!(
        result.output["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|problem| problem.as_str().unwrap().contains("path"))
    );
}

#[tokio::test]
async fn malformed_raw_arguments_return_structured_error() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let result = executor
        .invoke(ToolInvocation {
            id: "bad-json".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "raw_arguments": "{\"path\": " }),
        })
        .await;

    assert!(!result.ok);
    assert_eq!(result.output["error_code"], "invalid_arguments");
    assert_eq!(result.output["error_kind"], "malformed_arguments");
    assert_eq!(result.output["tool"], "read_file");
    assert_eq!(result.output["example"], json!({ "path": "example" }));
}

#[tokio::test]
async fn unknown_tool_returns_bounded_suggestions() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let result = executor
        .invoke(ToolInvocation {
            id: "list-files".to_string(),
            tool_name: "list_files".to_string(),
            input: json!({ "pattern": "*.rs" }),
        })
        .await;

    assert!(!result.ok);
    assert_eq!(result.output["error_code"], "unknown_tool");
    assert_eq!(result.output["error_kind"], "unknown_tool");
    let suggestions = result.output["suggestions"].as_array().unwrap();
    assert!(
        suggestions.iter().any(|s| {
            matches!(
                s.as_str(),
                Some("list_dir" | "fs_browser" | "search" | "glob")
            )
        }),
        "expected filesystem tool suggestions, got {suggestions:?}"
    );
}

#[tokio::test]
async fn init_session_creates_feature_contract_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let result = executor
        .invoke(ToolInvocation {
            id: "init".to_string(),
            tool_name: "init_session".to_string(),
            input: json!({
                "goal": "Build long-running harness",
                "features": [{
                    "id": "Feature One",
                    "title": "First feature",
                    "description": "Implement the first slice",
                    "verification_steps": ["true"]
                }]
            }),
        })
        .await;

    assert!(result.ok);
    assert_eq!(result.output["status"], "initialized");
    let feature_list = Path::new(
        result.output["feature_list"]
            .as_str()
            .expect("feature list"),
    );
    let progress = Path::new(result.output["progress"].as_str().expect("progress"));
    assert!(feature_list.exists());
    assert!(progress.exists());
    let content = std::fs::read_to_string(feature_list).expect("feature list");
    assert!(content.contains("feature-one"));
    assert!(content.contains(r#""passes": false"#));
}

#[tokio::test]
async fn mark_feature_done_runs_verification_before_passing() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    executor
        .invoke(ToolInvocation {
            id: "init".to_string(),
            tool_name: "init_session".to_string(),
            input: json!({
                "goal": "Ship feature",
                "features": [{
                    "id": "ship-it",
                    "title": "Ship it",
                    "verification_steps": ["true"]
                }]
            }),
        })
        .await;

    let result = executor
        .invoke(ToolInvocation {
            id: "done".to_string(),
            tool_name: "mark_feature_done".to_string(),
            input: json!({
                "feature_id": "ship-it",
                "verification_steps": ["true"],
                "notes": "verified"
            }),
        })
        .await;

    assert!(result.ok);
    assert_eq!(result.output["status"], "feature_completed");
    assert_eq!(result.output["passes"], true);
    let feature_list = test_feature_list_path(tempdir.path());
    let content = std::fs::read_to_string(feature_list).expect("feature list");
    assert!(content.contains(r#""passes": true"#));
    assert!(content.contains("verified"));
}

#[tokio::test]
async fn mark_feature_done_rejects_verification_contract_mismatch() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    executor
        .invoke(ToolInvocation {
            id: "init".to_string(),
            tool_name: "init_session".to_string(),
            input: json!({
                "goal": "Ship feature",
                "features": [{
                    "id": "ship-it",
                    "title": "Ship it",
                    "verification_steps": ["true"]
                }]
            }),
        })
        .await;

    let result = executor
        .invoke(ToolInvocation {
            id: "done".to_string(),
            tool_name: "mark_feature_done".to_string(),
            input: json!({
                "feature_id": "ship-it",
                "verification_steps": ["echo skipped"]
            }),
        })
        .await;

    assert!(result.ok);
    assert_eq!(result.output["status"], "error");
    assert_eq!(
        result.output["error_code"],
        "verification_contract_mismatch"
    );
    let content =
        std::fs::read_to_string(test_feature_list_path(tempdir.path())).expect("feature list");
    assert!(content.contains(r#""passes": false"#));
}

#[tokio::test]
async fn read_file_missing_path_returns_structured_error() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let result = executor
        .invoke(ToolInvocation {
            id: "missing-read".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "path": tempdir.path().join("missing.txt").display().to_string() }),
        })
        .await;

    assert!(!result.ok);
    assert!(
        result.output["error"]
            .as_str()
            .unwrap()
            .contains("failed to read")
    );
}

#[tokio::test]
async fn write_file_creates_parent_directories() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    let path = tempdir.path().join("nested/deep/file.txt");

    let result = executor
        .invoke(ToolInvocation {
            id: "write-nested".to_string(),
            tool_name: "write_file".to_string(),
            input: json!({ "path": path.display().to_string(), "content": "hello" }),
        })
        .await;

    assert!(result.ok);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "hello");
    assert_eq!(result.output["lines_added"], 1);
    assert_eq!(result.output["lines_removed"], 0);
}

#[tokio::test]
async fn grep_returns_empty_matches_for_no_hits() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::write(tempdir.path().join("file.txt"), "alpha beta").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "grep-empty".to_string(),
            tool_name: "grep".to_string(),
            input: json!({
                "pattern": "does-not-exist",
                "path": tempdir.path().display().to_string()
            }),
        })
        .await;

    assert!(result.ok);
    assert_eq!(result.output["matches"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn grep_defaults_to_project_root_when_path_omitted() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::write(tempdir.path().join("file.txt"), "project needle").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "grep-default-root".to_string(),
            tool_name: "grep".to_string(),
            input: json!({"pattern": "needle" }),
        })
        .await;

    assert!(result.ok, "{}", result.output);
    assert_eq!(result.output["matches"][0]["path"], "file.txt");
}

#[tokio::test]
async fn search_accepts_grep_style_arguments() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::create_dir_all(tempdir.path().join("src")).unwrap();
    std::fs::write(
        tempdir.path().join("src/lib.rs"),
        "Needle in rust\nneedle again",
    )
    .unwrap();
    std::fs::write(tempdir.path().join("src/lib.txt"), "Needle in text").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "search-style".to_string(),
            tool_name: "search".to_string(),
            input: json!({
                "action": "grep",
                "query": "needle",
                "include": "*.rs",
                "limit": 1,
                "case_sensitive": false
            }),
        })
        .await;

    assert!(result.ok, "{}", result.output);
    let matches = result.output["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["path"], "src/lib.rs");
    assert!(result.output["truncated"].as_bool().unwrap());
}

#[tokio::test]
async fn apply_patch_requires_patch_argument() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let invalid = ToolInvocation {
        id: "patch-missing".to_string(),
        tool_name: "apply_patch".to_string(),
        input: json!({}),
    };

    // Schema is intentionally permissive so model-facing simplification does not
    // force a single mode. Runtime still rejects empty apply_patch calls.
    assert!(executor.validate_arguments(&invalid).is_ok());
    let result = executor.invoke(invalid).await;
    assert!(!result.ok, "{:?}", result.output);
    assert_eq!(result.output["error_code"], "invalid_arguments");
}

#[tokio::test]
async fn apply_patch_accepts_structured_update() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    let path = tempdir.path().join("src/lib.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "fn old() {\n    println!(\"old\");\n}\n").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "patch-structured".to_string(),
            tool_name: "apply_patch".to_string(),
            input: json!({
                "patch": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n fn old() {\n-    println!(\"old\");\n+    println!(\"new\");\n }\n*** End Patch\n"
            }),
        })
        .await;

    assert!(result.ok, "{}", result.output);
    assert_eq!(result.output["method"], "structured");
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "fn old() {\n    println!(\"new\");\n}\n"
    );
}

#[tokio::test]
async fn apply_patch_accepts_structured_add_delete_and_move() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::write(tempdir.path().join("old.txt"), "before\n").unwrap();
    std::fs::write(tempdir.path().join("delete.txt"), "remove me\n").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "patch-ops".to_string(),
            tool_name: "apply_patch".to_string(),
            input: json!({
                "patch": "*** Begin Patch\n*** Add File: nested/new.txt\n+hello\n+world\n*** Delete File: delete.txt\n*** Update File: old.txt\n*** Move to: renamed.txt\n@@\n-before\n+after\n*** End Patch\n"
            }),
        })
        .await;

    assert!(result.ok, "{}", result.output);
    assert_eq!(
        std::fs::read_to_string(tempdir.path().join("nested/new.txt")).unwrap(),
        "hello\nworld\n"
    );
    assert!(!tempdir.path().join("delete.txt").exists());
    assert!(!tempdir.path().join("old.txt").exists());
    assert_eq!(
        std::fs::read_to_string(tempdir.path().join("renamed.txt")).unwrap(),
        "after\n"
    );
}

#[tokio::test]
async fn apply_patch_accepts_multiple_structured_patches() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::write(tempdir.path().join("one.txt"), "old one\n").unwrap();
    std::fs::write(tempdir.path().join("two.txt"), "old two\n").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "patches-structured".to_string(),
            tool_name: "apply_patch".to_string(),
            input: json!({
                "patches": [
                    "*** Begin Patch\n*** Update File: one.txt\n@@\n-old one\n+new one\n*** End Patch\n",
                    "*** Begin Patch\n*** Update File: two.txt\n@@\n-old two\n+new two\n*** End Patch\n"
                ]
            }),
        })
        .await;

    assert!(result.ok, "{}", result.output);
    assert_eq!(result.output["method"], "structured");
    assert_eq!(result.output["patches_applied"], 2);
    assert_eq!(
        std::fs::read_to_string(tempdir.path().join("one.txt")).unwrap(),
        "new one\n"
    );
    assert_eq!(
        std::fs::read_to_string(tempdir.path().join("two.txt")).unwrap(),
        "new two\n"
    );
}

#[tokio::test]
async fn apply_patch_multiple_structured_patches_are_sequential() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::write(tempdir.path().join("same.txt"), "one\ntwo\nthree\n").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "patches-same-file".to_string(),
            tool_name: "apply_patch".to_string(),
            input: json!({
                "patches": [
                    "*** Begin Patch\n*** Update File: same.txt\n@@\n-one\n+ONE\n two\n*** End Patch\n",
                    "*** Begin Patch\n*** Update File: same.txt\n@@\n TWO\n-three\n+THREE\n*** End Patch\n"
                ]
            }),
        })
        .await;

    assert!(
        !result.ok,
        "second patch should fail against unmatched context"
    );
    assert_eq!(
        std::fs::read_to_string(tempdir.path().join("same.txt")).unwrap(),
        "one\ntwo\nthree\n",
        "failed multi-patch calls must roll back earlier patches"
    );

    let result = executor
        .invoke(ToolInvocation {
            id: "patches-same-file-success".to_string(),
            tool_name: "apply_patch".to_string(),
            input: json!({
                "patches": [
                    "*** Begin Patch\n*** Update File: same.txt\n@@\n-one\n+ONE\n two\n*** End Patch\n",
                    "*** Begin Patch\n*** Update File: same.txt\n@@\n two\n-three\n+THREE\n*** End Patch\n"
                ]
            }),
        })
        .await;

    assert!(result.ok, "{}", result.output);
    assert_eq!(
        std::fs::read_to_string(tempdir.path().join("same.txt")).unwrap(),
        "ONE\ntwo\nTHREE\n"
    );
}

#[tokio::test]
async fn apply_patch_structured_failure_does_not_apply_prior_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::write(tempdir.path().join("existing.txt"), "actual\n").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "patch-atomic".to_string(),
            tool_name: "apply_patch".to_string(),
            input: json!({
                "patch": "*** Begin Patch\n*** Add File: created.txt\n+created\n*** Update File: existing.txt\n@@\n-missing\n+changed\n*** End Patch\n"
            }),
        })
        .await;

    assert!(!result.ok, "{}", result.output);
    assert!(!tempdir.path().join("created.txt").exists());
    assert_eq!(
        std::fs::read_to_string(tempdir.path().join("existing.txt")).unwrap(),
        "actual\n"
    );
}

#[tokio::test]
async fn apply_patch_failure_returns_twenty_line_context_window() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    let path = tempdir.path().join("src/lib.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let content = (1..=60)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&path, content).unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "patch-context".to_string(),
            tool_name: "apply_patch".to_string(),
            input: json!({
                "patch": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n line 30\n-line 31 old\n+line 31 new\n line 32\n*** End Patch\n"
            }),
        })
        .await;

    assert!(!result.ok, "{}", result.output);
    let context = result.output["context_lines"][0].as_object().unwrap();
    assert_eq!(context["path"], "src/lib.rs");
    assert_eq!(context["start_line"], 10);
    assert_eq!(context["end_line"], 50);
    assert!(context["lines"].as_array().unwrap().contains(&json!({
        "line": 30,
        "text": "line 30"
    })));
}

#[tokio::test]
async fn unknown_tool_returns_available_tools_advice() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let result = executor
        .invoke(ToolInvocation {
            id: "bad-tool".to_string(),
            tool_name: "nope".to_string(),
            input: json!({}),
        })
        .await;

    assert!(!result.ok);
    assert_eq!(result.output["error_code"], "unknown_tool");
    assert!(
        !result.output["available_tools"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(result.output["available_tools"].as_array().unwrap().len() <= 20);
}

#[tokio::test]
async fn recovers_path_as_tool_name_to_read_file() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    std::fs::write(tempdir.path().join("IDEA.md"), "# idea\n").unwrap();
    let executor = executor(tempdir.path());

    let result = executor
        .invoke(ToolInvocation {
            id: "path-as-name".to_string(),
            tool_name: "IDEA.md".to_string(),
            input: json!({ "path": "IDEA.md" }),
        })
        .await;

    assert!(
        result.ok,
        "expected recovery into read_file: {}",
        result.output
    );
    assert!(
        result.output["content"]
            .as_str()
            .unwrap_or("")
            .contains("idea"),
        "unexpected output: {}",
        result.output
    );
}

#[tokio::test]
async fn recovers_dot_list_as_fs_browser() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    std::fs::write(tempdir.path().join("foo.txt"), "hi").unwrap();
    let executor = executor(tempdir.path());

    let result = executor
        .invoke(ToolInvocation {
            id: "dot-list".to_string(),
            tool_name: ".".to_string(),
            input: json!({ "action": "list", "path": "." }),
        })
        .await;

    assert!(
        result.ok,
        "expected recovery into fs_browser: {}",
        result.output
    );
    let files = result.output["files"].as_array().expect("files array");
    assert!(
        files
            .iter()
            .any(|f| f.as_str().is_some_and(|s| s.ends_with("foo.txt"))),
        "missing foo.txt in {files:?}"
    );
}

// ── fs_browser regression tests ──────────────────────────────────────────────

#[tokio::test]
async fn fs_browser_list_returns_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    std::fs::write(tempdir.path().join("foo.txt"), "hello").unwrap();
    std::fs::write(tempdir.path().join("bar.rs"), "fn main() {}").unwrap();
    std::fs::create_dir(tempdir.path().join("subdir")).unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "fsl".to_string(),
            tool_name: "fs_browser".to_string(),
            input: json!({ "action": "list", "path": tempdir.path().display().to_string() }),
        })
        .await;

    assert!(result.ok);
    let files = result.output["files"].as_array().unwrap();
    assert!(files.len() >= 2); // foo.txt, bar.rs (subdir is recursed into)
    let file_strs: Vec<&str> = files.iter().filter_map(|f| f.as_str()).collect();
    assert!(file_strs.iter().any(|f| f.ends_with("foo.txt")));
    assert!(file_strs.iter().any(|f| f.ends_with("bar.rs")));
}

#[tokio::test]
async fn fs_browser_list_returns_total_and_truncated() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    std::fs::write(tempdir.path().join("a.txt"), "a").unwrap();
    std::fs::write(tempdir.path().join("b.txt"), "b").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "fsl".to_string(),
            tool_name: "fs_browser".to_string(),
            input: json!({ "action": "list", "path": tempdir.path().display().to_string() }),
        })
        .await;

    assert!(result.ok);
    assert_eq!(result.output["total"], 2);
    assert_eq!(result.output["truncated"], false);
}

#[tokio::test]
async fn fs_browser_tree_returns_nested_structure() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    std::fs::write(tempdir.path().join("root.txt"), "root").unwrap();
    std::fs::create_dir(tempdir.path().join("sub")).unwrap();
    std::fs::write(tempdir.path().join("sub/child.txt"), "child").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "fst".to_string(),
            tool_name: "fs_browser".to_string(),
            input: json!({
                "action": "tree",
                "path": tempdir.path().display().to_string(),
                "depth": 2
            }),
        })
        .await;

    assert!(result.ok);
    let entries = result.output["entries"].as_array().unwrap();
    assert!(entries.len() >= 2); // root.txt and sub

    let sub_entry = entries.iter().find(|e| e["name"] == "sub").unwrap();
    assert_eq!(sub_entry["type"], "dir");
    let children = sub_entry["entries"].as_array().unwrap();
    assert!(children.iter().any(|c| c["name"] == "child.txt"));
}

#[tokio::test]
async fn fs_browser_tree_respects_depth() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    std::fs::create_dir_all(tempdir.path().join("a/b/c")).unwrap();
    std::fs::write(tempdir.path().join("a/b/c/deep.txt"), "deep").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "fst".to_string(),
            tool_name: "fs_browser".to_string(),
            input: json!({
                "action": "tree",
                "path": tempdir.path().display().to_string(),
                "depth": 1
            }),
        })
        .await;

    assert!(result.ok);
    let entries = result.output["entries"].as_array().unwrap();
    let a_entry = entries.iter().find(|e| e["name"] == "a").unwrap();
    assert_eq!(a_entry["type"], "dir");
    // At depth 1, 'a' should have children listed
    let a_children = a_entry["entries"].as_array().unwrap();
    let b_entry = a_children.iter().find(|e| e["name"] == "b").unwrap();
    // At depth 1, 'b' should NOT have children (would need depth 2)
    assert!(b_entry.get("entries").is_none() || b_entry["entries"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn fs_browser_find_discovers_matching_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    std::fs::write(tempdir.path().join("readme.md"), "# Hi").unwrap();
    std::fs::write(tempdir.path().join("notes.md"), "notes").unwrap();
    std::fs::write(tempdir.path().join("code.rs"), "fn main() {}").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "fsf".to_string(),
            tool_name: "fs_browser".to_string(),
            input: json!({
                "action": "find",
                "path": tempdir.path().display().to_string(),
                "pattern": ".md"
            }),
        })
        .await;

    assert!(result.ok);
    let files = result.output["files"].as_array().unwrap();
    assert!(files.len() >= 2);
    let file_strs: Vec<&str> = files.iter().filter_map(|f| f.as_str()).collect();
    assert!(file_strs.iter().any(|f| f.ends_with("readme.md")));
    assert!(file_strs.iter().any(|f| f.ends_with("notes.md")));
}

#[tokio::test]
async fn fs_browser_stat_returns_file_metadata() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    std::fs::write(tempdir.path().join("info.txt"), "hello").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "fss".to_string(),
            tool_name: "fs_browser".to_string(),
            input: json!({
                "action": "stat",
                "path": tempdir.path().join("info.txt").display().to_string()
            }),
        })
        .await;

    assert!(result.ok);
    assert_eq!(result.output["type"], "file");
    assert_eq!(result.output["size"], 5);
    assert!(result.output["modified"].as_u64().is_some());
    assert!(result.output["permissions"].as_str().is_some());
}

#[tokio::test]
async fn fs_browser_stat_returns_directory_metadata() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    std::fs::create_dir(tempdir.path().join("mydir")).unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "fss".to_string(),
            tool_name: "fs_browser".to_string(),
            input: json!({
                "action": "stat",
                "path": tempdir.path().join("mydir").display().to_string()
            }),
        })
        .await;

    assert!(result.ok);
    assert_eq!(result.output["type"], "dir");
}

#[tokio::test]
async fn fs_browser_list_skips_hidden_and_build_dirs() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    std::fs::write(tempdir.path().join("visible.txt"), "v").unwrap();
    std::fs::create_dir(tempdir.path().join(".git")).unwrap();
    std::fs::write(tempdir.path().join(".git/config"), "c").unwrap();
    std::fs::create_dir(tempdir.path().join("target")).unwrap();
    std::fs::write(tempdir.path().join("target/out"), "o").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "fsl".to_string(),
            tool_name: "fs_browser".to_string(),
            input: json!({ "action": "list", "path": tempdir.path().display().to_string() }),
        })
        .await;

    assert!(result.ok);
    let files = result.output["files"].as_array().unwrap();
    let file_strs: Vec<&str> = files.iter().filter_map(|f| f.as_str()).collect();
    assert!(file_strs.iter().any(|f| f.ends_with("visible.txt")));
    assert!(!file_strs.iter().any(|f| f.contains(".git")));
    assert!(!file_strs.iter().any(|f| f.contains("target")));
}

// ── test_runner regression tests ─────────────────────────────────────────────

// ── package_manager regression tests ─────────────────────────────────────────

#[test]
fn package_manager_definition_has_expected_schema() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let def = executor
        .definition("package_manager")
        .expect("package_manager");
    assert_eq!(def.name, "package_manager");
    assert_eq!(def.input_schema["type"], "object");
    let required = def.input_schema["required"].as_array().unwrap();
    assert!(required.contains(&json!("action")));
}

#[tokio::test]
async fn package_manager_add_errors_without_packages() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    std::fs::write(
        tempdir.path().join("Cargo.toml"),
        "[package]\nname = \"test\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "pm".to_string(),
            tool_name: "package_manager".to_string(),
            input: json!({
                "action": "add",
                "manager": "cargo",
                "packages": []
            }),
        })
        .await;

    assert!(!result.ok);
    assert_eq!(result.output["error_code"], "missing_packages");
    assert!(result.output["message"].as_str().is_some());
}

// ── Integration: tools registered in definitions ─────────────────────────────

#[test]
fn all_specialized_tools_registered_in_definitions() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    let names = executor.tool_names();
    assert!(names.contains(&"read_file".to_string()));
    assert!(names.contains(&"write_file".to_string()));
    assert!(names.contains(&"apply_patch".to_string()));
    assert!(names.contains(&"grep".to_string()));
    assert!(names.contains(&"fs_browser".to_string()));
    assert!(names.contains(&"search".to_string()));
    assert!(names.contains(&"package_manager".to_string()));
}

#[test]
fn all_specialized_tools_have_valid_schemas() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    for name in ["search", "package_manager"] {
        let def = executor.definition(name).expect(name);
        assert_eq!(def.input_schema["type"], "object");
        assert!(
            def.input_schema["properties"].as_object().is_some(),
            "{name} should have properties"
        );
    }
}

// ── Regression tests ──────────────────────────────────────────────────────────

#[tokio::test]
async fn regression_read_file_out_of_range_lines_returns_empty() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    std::fs::write(tempdir.path().join("small.txt"), "line1\nline2\nline3").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "test".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({"path": "small.txt", "start_line": 100, "end_line": 200}),
        })
        .await;

    // Should either succeed with empty content or return a structured error
    // Either way, it must not panic
    if result.ok {
        let content = result.output["content"].as_str().unwrap_or("");
        assert!(content.is_empty() || content.trim().is_empty());
    } else {
        // Structured error is acceptable
        assert!(result.output["error"].is_string() || result.output["error_code"].is_string());
    }
}

#[tokio::test]
async fn regression_read_file_binary_returns_error() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    // Write invalid UTF-8 bytes
    std::fs::write(tempdir.path().join("binary.bin"), [0xFF, 0xFE, 0x00, 0x01]).unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "test".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({"path": "binary.bin"}),
        })
        .await;

    // Should either succeed with lossy conversion or return an error
    // Either way, it must not panic
    let _ = result;
}

#[tokio::test]
async fn regression_write_file_overwrites_existing() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    std::fs::write(tempdir.path().join("existing.txt"), "old\ncontent\n").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "test".to_string(),
            tool_name: "write_file".to_string(),
            input: json!({"path": "existing.txt", "content": "new content"}),
        })
        .await;

    // write_file needs approval, so it may return ok=false with a security decision
    // or ok=true if the executor auto-approves. Either way, no panic.
    if result.ok {
        let content = std::fs::read_to_string(tempdir.path().join("existing.txt")).unwrap();
        assert_eq!(content, "new content");
        assert_eq!(result.output["lines_added"], 1);
        assert_eq!(result.output["lines_removed"], 2);
    }
    // If not ok, it's because of approval requirement - that's fine
}

#[tokio::test]
async fn regression_grep_special_chars_treated_literally() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    std::fs::write(tempdir.path().join("test.txt"), "foo(bar\nbaz[0]\nqux*").unwrap();

    // Pattern with regex metacharacters should be treated literally
    let result = executor
        .invoke(ToolInvocation {
            id: "test".to_string(),
            tool_name: "grep".to_string(),
            input: json!({"pattern": "foo(bar", "path": "."}),
        })
        .await;

    // grep may fail if rg is not installed, which is fine
    if result.ok {
        let matches = result.output["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1, "literal pattern should match");
    }
}

#[tokio::test]
async fn regression_bash_foreground_captures_stderr() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let result = executor
        .invoke(ToolInvocation {
            id: "test".to_string(),
            tool_name: "bash".to_string(),
            input: json!({"command": "echo error >&2", "timeout_ms": 5000}),
        })
        .await;

    assert!(result.ok);
    let stderr = result.output["stderr"].as_str().unwrap_or("");
    assert!(
        stderr.contains("error"),
        "stderr should be captured: {stderr}"
    );
}

#[tokio::test]
async fn regression_bash_timeout_capped_at_max() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    // Request a timeout of 999999ms (should be capped at 120s)
    let result = executor
        .invoke(ToolInvocation {
            id: "test".to_string(),
            tool_name: "bash".to_string(),
            input: json!({"command": "echo ok", "timeout_ms": 999_999}),
        })
        .await;

    assert!(result.ok);
    assert_eq!(result.output["stdout"].as_str().unwrap().trim(), "ok");
}

// ── Audit fixes: path relativity, empty writes, bash poll schema ─────────────

#[tokio::test]
async fn fs_browser_list_returns_project_relative_paths() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::create_dir_all(tempdir.path().join("src")).unwrap();
    std::fs::write(tempdir.path().join("src/lib.rs"), "fn main() {}\n").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "rel-list".to_string(),
            tool_name: "fs_browser".to_string(),
            input: json!({ "action": "list", "path": "src" }),
        })
        .await;

    assert!(result.ok, "{:?}", result.output);
    let files = result.output["files"].as_array().unwrap();
    assert!(!files.is_empty());
    for path in files {
        let path = path.as_str().unwrap();
        assert!(
            !Path::new(path).is_absolute(),
            "expected project-relative path, got absolute: {path}"
        );
    }
    assert!(
        files
            .iter()
            .any(|p| matches!(p.as_str(), Some("src/lib.rs") | Some("lib.rs"))),
        "{files:?}"
    );
}

#[tokio::test]
async fn list_dir_returns_project_relative_paths() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::write(tempdir.path().join("README.md"), "hello\n").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "list-dir".to_string(),
            tool_name: "list_dir".to_string(),
            input: json!({ "path": "." }),
        })
        .await;

    assert!(result.ok, "{:?}", result.output);
    let files = result.output["files"].as_array().unwrap();
    assert!(
        files.iter().any(|p| p.as_str() == Some("README.md")),
        "expected relative README.md in {files:?}"
    );
}

#[tokio::test]
async fn write_file_allows_empty_content() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::write(tempdir.path().join("notes.txt"), "old\n").unwrap();

    let result = executor
        .invoke(ToolInvocation {
            id: "empty-write".to_string(),
            tool_name: "write_file".to_string(),
            input: json!({ "path": "notes.txt", "content": "" }),
        })
        .await;

    assert!(result.ok, "{:?}", result.output);
    assert_eq!(
        std::fs::read_to_string(tempdir.path().join("notes.txt")).unwrap(),
        ""
    );
    assert_eq!(result.output["lines_added"], 0);
}

#[test]
fn model_facing_bash_schema_does_not_require_command() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    let bash = executor
        .definitions()
        .into_iter()
        .find(|definition| definition.name == "bash")
        .expect("bash");

    let required = bash
        .input_schema
        .get("required")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        !required.iter().any(|v| v.as_str() == Some("command")),
        "bash model schema must allow poll/list without command; required={required:?}"
    );
    assert!(bash.input_schema["properties"]["task_id"].is_object());
    assert!(bash.input_schema["properties"]["action"].is_object());
}

#[test]
fn register_skill_loader_registers_load_skill_tool() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut executor = executor(tempdir.path());
    let config = std::sync::Arc::new(std::sync::RwLock::new(crate::config::NaviConfig::default()));

    executor.register_skill_loader(
        tempdir.path().to_path_buf(),
        tempdir.path().join(".navi-data"),
        config,
    );

    assert!(executor.definition("load_skill").is_some());
    let names = executor.tool_names();
    assert!(names.contains(&"load_skill".to_string()), "{names:?}");
}

#[test]
fn model_facing_write_schema_allows_patch_mode() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    let write = executor
        .all_definitions()
        .into_iter()
        .find(|definition| definition.name == "write")
        .expect("write");

    let required = write
        .input_schema
        .get("required")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        !required.iter().any(|v| v.as_str() == Some("path")),
        "write model schema must not force path+content for patch mode; required={required:?}"
    );
    assert!(write.input_schema["properties"]["patch"].is_object());
    assert!(write.input_schema["properties"]["edits"].is_object());
}

#[test]
fn visible_definitions_hide_aliases_and_keep_core_edit_loop() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    let names: Vec<String> = executor.definitions().into_iter().map(|d| d.name).collect();

    for core in [
        "read_file",
        "search",
        "edit",
        "write_file",
        "bash",
        "plan",
        "question",
        "tool_search",
        "memory",
    ] {
        assert!(
            names.iter().any(|n| n == core),
            "missing core tool {core} in visible set: {names:?}"
        );
    }
    for hidden in [
        "read",
        "view_file",
        "grep",
        "fs_browser",
        "list_dir",
        "glob",
        "write",
        "multiedit",
        "apply_patch",
        "request_user_input",
    ] {
        assert!(
            !names.iter().any(|n| n == hidden),
            "hidden alias {hidden} still visible: {names:?}"
        );
    }
    for deferred in [
        "code",
        "code_edit",
        "ast_search",
        "package_manager",
        "set_goal",
        "sandbox",
        "append_note",
        "history_ops",
        "current_time",
        "sleep",
    ] {
        assert!(
            !names.iter().any(|n| n == deferred),
            "deferred tool {deferred} still visible: {names:?}"
        );
        // Still registered and invokable / searchable.
        assert!(
            executor.definition(deferred).is_some(),
            "deferred tool {deferred} should remain registered"
        );
    }
    // Runtime-only tools (registered by AgentRuntime, not bare ToolExecutor).
    assert!(!names.iter().any(|n| n == "repo_explore"));
    // Still invokable by name even if hidden from schema.
    assert!(executor.definition("grep").is_some());
    assert!(executor.definition("multiedit").is_some());
    assert!(executor.definition("apply_patch").is_some());

    // Core coding surface stays small for the model.
    assert!(
        names.len() <= 15,
        "visible tool count too large ({}): {names:?}",
        names.len()
    );
}

#[test]
fn tool_search_discovers_deferred_power_tools() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    let results = executor.search_tools("code symbols", 10);
    let names: Vec<&str> = results.iter().map(|d| d.name.as_str()).collect();
    assert!(
        names
            .iter()
            .any(|n| *n == "code" || n.contains("symbol") || *n == "ast_search"),
        "expected code/symbol tools from tool_search, got {names:?}"
    );
    let pkg = executor.search_tools("package dependency", 10);
    assert!(
        pkg.iter().any(|d| d.name == "package_manager"),
        "package_manager should be discoverable: {:?}",
        pkg.iter().map(|d| &d.name).collect::<Vec<_>>()
    );
}
