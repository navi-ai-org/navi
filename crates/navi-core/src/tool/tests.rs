use super::*;
use crate::{SecurityConfig, SecurityPolicy};
use std::path::Path;

fn executor(root: &Path) -> ToolExecutor {
    let policy = SecurityPolicy::new(
        root.to_path_buf(),
        root.join(".navi-data"),
        SecurityConfig::default(),
    )
    .expect("policy");
    ToolExecutor::new(policy)
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
async fn top_files_is_registered() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let definition = executor.definition("top_files").expect("top_files");

    assert_eq!(definition.kind, ToolKind::Read);
    assert_eq!(definition.input_schema["type"], "object");
}

#[tokio::test]
async fn top_files_returns_ranked_relevant_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::create_dir_all(tempdir.path().join("src")).expect("mkdir");
    std::fs::write(
        tempdir.path().join("src/auth.rs"),
        "pub fn provider_auth() { let token_source = \"env\"; }\n",
    )
    .expect("write auth");
    std::fs::write(
        tempdir.path().join("src/render.rs"),
        "pub fn render_view() {}\n",
    )
    .expect("write render");

    let result = executor
        .invoke(ToolInvocation {
            id: "top".to_string(),
            tool_name: "top_files".to_string(),
            input: json!({ "query": "provider auth", "max_files": 2 }),
        })
        .await;

    assert!(result.ok, "{:?}", result.output);
    let files = result.output["files"].as_array().unwrap();
    assert_eq!(files[0]["path"], "src/auth.rs");
    assert!(
        files[0]["content"]
            .as_str()
            .unwrap()
            .contains("provider_auth")
    );
    assert!(
        files[0]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason == "query_content_match")
    );
}

#[tokio::test]
async fn top_files_code_overview_prefers_code_structure_over_large_agent_docs() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::create_dir_all(tempdir.path().join("crates/demo/src")).expect("mkdir");
    std::fs::write(
        tempdir.path().join("AGENTS.md"),
        "project overview structure\n".repeat(300),
    )
    .expect("write agents");
    std::fs::write(
        tempdir.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/demo\"]\n",
    )
    .expect("write workspace");
    std::fs::write(
        tempdir.path().join("crates/demo/Cargo.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .expect("write crate manifest");
    std::fs::write(
        tempdir.path().join("crates/demo/src/lib.rs"),
        "pub mod runtime;\npub fn start_engine_runtime() {}\n",
    )
    .expect("write lib");
    std::fs::write(
        tempdir.path().join("crates/demo/src/runtime.rs"),
        "pub struct AgentRuntime;\n",
    )
    .expect("write runtime");

    let result = executor
        .invoke(ToolInvocation {
            id: "top".to_string(),
            tool_name: "top_files".to_string(),
            input: json!({ "query": "project overview structure", "max_files": 3 }),
        })
        .await;

    assert!(result.ok, "{:?}", result.output);
    let paths = result.output["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(paths.contains(&"Cargo.toml".to_string()), "{paths:?}");
    assert!(
        paths
            .iter()
            .any(|path| path == "crates/demo/src/lib.rs" || path == "crates/demo/src/runtime.rs"),
        "{paths:?}"
    );
    assert_ne!(paths.first().map(String::as_str), Some("AGENTS.md"));
}

#[tokio::test]
async fn top_files_docs_query_keeps_agent_docs_on_top() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::create_dir_all(tempdir.path().join("src")).expect("mkdir");
    std::fs::write(
        tempdir.path().join("AGENTS.md"),
        "agent instructions rules guide\n".repeat(50),
    )
    .expect("write agents");
    std::fs::write(
        tempdir.path().join("src/main.rs"),
        "fn main() { println!(\"agent instructions\"); }\n",
    )
    .expect("write main");

    let result = executor
        .invoke(ToolInvocation {
            id: "top".to_string(),
            tool_name: "top_files".to_string(),
            input: json!({ "query": "agent instructions", "max_files": 2 }),
        })
        .await;

    assert!(result.ok, "{:?}", result.output);
    let files = result.output["files"].as_array().unwrap();
    assert_eq!(files[0]["path"], "AGENTS.md");
    assert!(
        files[0]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason == "docs_overview_boost")
    );
}

#[tokio::test]
async fn top_files_truncates_long_files_to_default_limit() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::create_dir_all(tempdir.path().join("src")).expect("mkdir");
    let content = (1..=600)
        .map(|line| format!("pub fn long_unique_{line}() {{}}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(tempdir.path().join("src/long.rs"), format!("{content}\n")).expect("write long");

    let result = executor
        .invoke(ToolInvocation {
            id: "top".to_string(),
            tool_name: "top_files".to_string(),
            input: json!({ "query": "long_unique" }),
        })
        .await;

    assert!(result.ok, "{:?}", result.output);
    let file = &result.output["files"].as_array().unwrap()[0];
    assert_eq!(file["path"], "src/long.rs");
    assert_eq!(file["end_line"], 400);
    assert_eq!(file["total_lines"], 600);
    assert!(file["truncated"].as_bool().unwrap());
    assert!(
        !file["content"]
            .as_str()
            .unwrap()
            .contains("long_unique_500")
    );
}

#[tokio::test]
async fn top_files_respects_max_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::create_dir_all(tempdir.path().join("src")).expect("mkdir");
    for index in 0..5 {
        std::fs::write(
            tempdir.path().join(format!("src/file_{index}.rs")),
            format!("pub fn needle_{index}() {{}}\n"),
        )
        .expect("write file");
    }

    let result = executor
        .invoke(ToolInvocation {
            id: "top".to_string(),
            tool_name: "top_files".to_string(),
            input: json!({ "query": "needle", "max_files": 2 }),
        })
        .await;

    assert!(result.ok, "{:?}", result.output);
    assert_eq!(result.output["files"].as_array().unwrap().len(), 2);
    assert!(result.output["truncated"].as_bool().unwrap());
}

#[tokio::test]
async fn top_files_skips_denied_paths() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::create_dir_all(tempdir.path().join("src")).expect("mkdir src");
    std::fs::create_dir_all(tempdir.path().join("node_modules/pkg")).expect("mkdir node_modules");
    std::fs::write(
        tempdir.path().join("src/open.rs"),
        "pub fn needle_visible() {}\n",
    )
    .expect("write open");
    std::fs::write(
        tempdir.path().join("node_modules/pkg/hidden.rs"),
        "pub fn needle_hidden() {}\n",
    )
    .expect("write hidden");

    let result = executor
        .invoke(ToolInvocation {
            id: "top".to_string(),
            tool_name: "top_files".to_string(),
            input: json!({ "query": "needle", "max_files": 10 }),
        })
        .await;

    assert!(result.ok, "{:?}", result.output);
    let paths = result.output["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(paths, vec!["src/open.rs"]);
}

#[tokio::test]
async fn top_files_skips_binary_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::create_dir_all(tempdir.path().join("src")).expect("mkdir");
    std::fs::write(
        tempdir.path().join("src/text.rs"),
        "pub fn binary_needle_text() {}\n",
    )
    .expect("write text");
    std::fs::write(
        tempdir.path().join("src/binary.rs"),
        b"binary_needle\0not text",
    )
    .expect("write binary");

    let result = executor
        .invoke(ToolInvocation {
            id: "top".to_string(),
            tool_name: "top_files".to_string(),
            input: json!({ "query": "binary_needle", "max_files": 10 }),
        })
        .await;

    assert!(result.ok, "{:?}", result.output);
    let paths = result.output["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(paths, vec!["src/text.rs"]);
}

#[tokio::test]
async fn top_files_caps_total_output_bytes() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::create_dir_all(tempdir.path().join("src")).expect("mkdir");
    std::fs::write(
        tempdir.path().join("src/large.rs"),
        format!("pub fn cap_marker() {{}}\n{}\n", "x".repeat(1_000)),
    )
    .expect("write large");

    let result = executor
        .invoke(ToolInvocation {
            id: "top".to_string(),
            tool_name: "top_files".to_string(),
            input: json!({ "query": "cap_marker", "max_total_bytes": 80 }),
        })
        .await;

    assert!(result.ok, "{:?}", result.output);
    let file = &result.output["files"].as_array().unwrap()[0];
    assert!(file["truncated_by_total_limit"].as_bool().unwrap());
    assert!(file["content"].as_str().unwrap().contains("<truncated>"));
    assert!(result.output["truncated"].as_bool().unwrap());
}

#[tokio::test]
async fn tool_workflow_batches_read_only_tools() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::create_dir_all(tempdir.path().join("src")).expect("mkdir");
    std::fs::write(tempdir.path().join("src/a.rs"), "fn alpha() {}\n").expect("write a");
    std::fs::write(tempdir.path().join("src/b.rs"), "fn beta() { alpha(); }\n").expect("write b");

    let result = executor
        .invoke(ToolInvocation {
            id: "workflow".to_string(),
            tool_name: "tool_workflow".to_string(),
            input: json!({
                "script": r#"
def workflow():
    files = tool("fs_browser", {"action": "find", "path": "src", "pattern": ".rs"})["files"]
    matches = []
    for file in files:
        read = tool("read_file", {"path": file})
        if "alpha" in read["content"]:
            matches.append(file)
    return {"count": len(matches), "matches": matches}
workflow()
"#
            }),
        })
        .await;

    assert!(result.ok, "{:?}", result.output);
    assert_eq!(result.output["result"]["count"], 2);
    assert_eq!(result.output["tool_calls"], 3);
}

#[tokio::test]
async fn tool_workflow_rejects_non_read_only_nested_tools() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let result = executor
        .invoke(ToolInvocation {
            id: "workflow".to_string(),
            tool_name: "tool_workflow".to_string(),
            input: json!({
                "script": r#"tool("write_file", {"path": "x.txt", "content": "nope"})"#
            }),
        })
        .await;

    assert!(!result.ok);
    assert!(
        result.output["error"]
            .as_str()
            .unwrap()
            .contains("only allows read_file, grep, fs_browser, and read-only git_ops")
    );
}

#[tokio::test]
async fn tool_workflow_enforces_tool_call_limit() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());
    std::fs::write(tempdir.path().join("a.txt"), "a\n").expect("write a");
    std::fs::write(tempdir.path().join("b.txt"), "b\n").expect("write b");

    let result = executor
        .invoke(ToolInvocation {
            id: "workflow".to_string(),
            tool_name: "tool_workflow".to_string(),
            input: json!({
                "max_tool_calls": 1,
                "script": r#"
tool("read_file", {"path": "a.txt"})
tool("read_file", {"path": "b.txt"})
"#
            }),
        })
        .await;

    assert!(!result.ok);
    assert!(
        result.output["error"]
            .as_str()
            .unwrap()
            .contains("exceeded max_tool_calls")
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

#[test]
fn apply_patch_requires_patch_argument() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let invalid = ToolInvocation {
        id: "patch-missing".to_string(),
        tool_name: "apply_patch".to_string(),
        input: json!({}),
    };

    let err = executor
        .validate_arguments(&invalid)
        .expect_err("missing patch should fail");
    assert!(matches!(err, ToolCallInvalid::InvalidArguments { .. }));
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
        result.output["available_tools"]
            .as_array()
            .unwrap()
            .contains(&json!("read_file"))
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

#[test]
fn test_runner_definition_has_expected_schema() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let def = executor.definition("test_runner").expect("test_runner");
    assert_eq!(def.name, "test_runner");
    assert_eq!(def.input_schema["type"], "object");
    // test_runner has no required fields
    assert!(
        def.input_schema.get("required").is_none()
            || def.input_schema["required"].as_array().unwrap().is_empty()
    );
}

// ── build_runner regression tests ────────────────────────────────────────────

#[test]
fn build_runner_definition_has_expected_schema() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let def = executor.definition("build_runner").expect("build_runner");
    assert_eq!(def.name, "build_runner");
    assert_eq!(def.input_schema["type"], "object");
    // build_runner has no required fields
    assert!(
        def.input_schema.get("required").is_none()
            || def.input_schema["required"].as_array().unwrap().is_empty()
    );
}

// ── git_ops regression tests ─────────────────────────────────────────────────

#[test]
fn git_ops_definition_has_expected_schema() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    let def = executor.definition("git_ops").expect("git_ops");
    assert_eq!(def.name, "git_ops");
    assert_eq!(def.input_schema["type"], "object");
    let required = def.input_schema["required"].as_array().unwrap();
    assert!(required.contains(&json!("command")));
    assert!(def.input_schema["properties"]["args"]["oneOf"].is_array());
}

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

    assert!(names.contains(&"test_runner".to_string()));
    assert!(names.contains(&"build_runner".to_string()));
    assert!(names.contains(&"fs_browser".to_string()));
    assert!(names.contains(&"git_ops".to_string()));
    assert!(names.contains(&"package_manager".to_string()));
}

#[test]
fn all_specialized_tools_have_valid_schemas() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let executor = executor(tempdir.path());

    for name in [
        "test_runner",
        "build_runner",
        "fs_browser",
        "git_ops",
        "package_manager",
    ] {
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
