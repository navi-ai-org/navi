use std::process::Command;
use tempfile::TempDir;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn headless_cli_runs_engine_provider_and_read_tool() {
    let mock_server = MockServer::start().await;
    let project = TempDir::new().expect("project tempdir");
    let config_home = TempDir::new().expect("config tempdir");
    let data_home = TempDir::new().expect("data tempdir");

    std::fs::create_dir(project.path().join(".navi")).expect("create .navi");
    std::fs::write(project.path().join("fixture.txt"), "from fixture\n").expect("write fixture");
    std::fs::write(
        project.path().join(".navi").join("config.toml"),
        format!(
            r#"
[model]
provider = "mock-openai"
name = "mock-model"

[[providers]]
id = "mock-openai"
label = "Mock OpenAI"
kind = "open-ai-chat-completions"
api_key_env = "PATH"
base_url = "{}"
request_timeout_ms = 5000
stream_idle_timeout_ms = 5000
request_max_retries = 1
stream_max_retries = 1

[[providers.models]]
name = "mock-model"
task_size = "small"
"#,
            mock_server.uri()
        ),
    )
    .expect("write config");

    let tool_call = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[",
        "{\"index\":0,\"id\":\"call-read\",\"function\":{",
        "\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"fixture.txt\\\"}\"",
        "}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let final_answer = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"read: from fixture\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(|request: &wiremock::Request| {
            let body = serde_json::from_slice::<serde_json::Value>(&request.body).ok();
            let messages = body
                .as_ref()
                .and_then(|body| body.get("messages"))
                .and_then(serde_json::Value::as_array);
            body.as_ref()
                .and_then(|body| body.get("model"))
                .and_then(serde_json::Value::as_str)
                == Some("mock-model")
                && messages.is_some_and(|messages| {
                    messages.iter().any(|message| {
                        message.get("role").and_then(serde_json::Value::as_str) == Some("user")
                            && message.get("content").and_then(serde_json::Value::as_str)
                                == Some("read the fixture")
                    }) && !messages.iter().any(|message| {
                        message.get("role").and_then(serde_json::Value::as_str) == Some("tool")
                    })
                })
        })
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(tool_call)
                .insert_header("content-type", "text/event-stream"),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_string_contains("from fixture"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(final_answer)
                .insert_header("content-type", "text/event-stream"),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let output = Command::new(env!("CARGO_BIN_EXE_navi"))
        .arg("--no-tui")
        .arg("--no-log-file")
        .arg("--log-level")
        .arg("error")
        .arg("read the fixture")
        .current_dir(project.path())
        .env("XDG_CONFIG_HOME", config_home.path())
        .env("XDG_DATA_HOME", data_home.path())
        .output()
        .expect("run navi");

    assert!(
        output.status.success(),
        "navi failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "read: from fixture\n"
    );

    let sessions_dir = data_home.path().join("navi").join("sessions");
    let saved_sessions = std::fs::read_dir(&sessions_dir)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", sessions_dir.display()))
        .count();
    assert_eq!(saved_sessions, 1);
}
