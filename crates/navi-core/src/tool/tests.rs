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
    assert!(executor.invoke(write).await.ok);

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
    assert!(executor.invoke(write_multiline).await.ok);

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
