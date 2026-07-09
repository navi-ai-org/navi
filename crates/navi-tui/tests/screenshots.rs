//! Integration tests for the TUI that drive the real input + rendering path
//! against a `TestBackend` and compare against golden snapshots in
//! `tests/snapshots/`.
//!
//! Workflow:
//!   * `cargo test -p navi-tui --test screenshots` — verify all goldens match.
//!   * `UPDATE_SNAPSHOTS=1 cargo test -p navi-tui --test screenshots` —
//!     overwrite goldens. Review the diff in `tests/snapshots/` before
//!     committing.
//!   * `just snapshot-update` — same as above, via the justfile recipe.
//!
//! Goldens are first created automatically on a fresh run; subsequent runs
//! fail with a full diff if the rendered output drifts.

use crossterm::event::{KeyCode, KeyModifiers};
use navi_sdk::{BackgroundCommandSnapshot, BackgroundTaskStatus};
use navi_tui::testing::{AgentEvent, AsyncEvent, ChatMessage, ChatRole, Harness, Mode, TestConfig};

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

fn h() -> Harness {
    Harness::new(TestConfig {
        width: 80,
        height: 24,
        ..TestConfig::default()
    })
}

fn h_size(width: u16, height: u16) -> Harness {
    Harness::new(TestConfig {
        width,
        height,
        ..TestConfig::default()
    })
}

// -----------------------------------------------------------------------------
// Welcome / empty state
// -----------------------------------------------------------------------------

#[test]
fn welcome_screen_empty_input_80x24() {
    let mut h = h();
    h.render();
    h.assert_screen("welcome_80x24");
}

#[test]
fn welcome_screen_120x40() {
    let mut h = h_size(120, 40);
    h.render();
    h.assert_screen("welcome_120x40");
}

#[test]
fn welcome_screen_with_input_typed() {
    let mut h = h();
    h.type_text("explain the routing module");
    h.render();
    h.assert_screen("welcome_with_input_80x24");
}

#[test]
fn welcome_screen_narrow_terminal() {
    let mut h = h_size(40, 12);
    h.render();
    h.assert_screen("welcome_40x12");
}

#[test]
fn welcome_screen_missing_provider_credential() {
    let mut h = Harness::new(TestConfig {
        provider_configured: false,
        ..TestConfig::default()
    });
    h.render();
    h.assert_screen("welcome_missing_credential_80x24");
}

// -----------------------------------------------------------------------------
// Modal screens (open via real keybindings)
// -----------------------------------------------------------------------------

#[test]
fn command_palette_open() {
    let mut h = h();
    h.press(KeyCode::Char('p'), KeyModifiers::CONTROL);
    assert_eq!(h.mode(), Mode::Commands);
    h.render();
    h.assert_screen("modal_command_palette_80x24");
}

#[test]
fn command_palette_filter_typed() {
    let mut h = h();
    h.press(KeyCode::Char('p'), KeyModifiers::CONTROL);
    h.type_text("mode");
    h.render();
    h.assert_screen("modal_command_palette_filtered_80x24");
}

#[test]
fn model_picker_open() {
    let mut h = h();
    h.clear_models();
    h.press(KeyCode::Char('m'), KeyModifiers::CONTROL);
    assert_eq!(h.mode(), Mode::Models);
    h.render();
    h.assert_screen("modal_model_picker_80x24");
}

#[test]
fn help_modal_open() {
    let mut h = h();
    h.press(KeyCode::Char('?'), KeyModifiers::NONE);
    assert_eq!(h.mode(), Mode::Help);
    h.render();
    h.assert_screen("modal_help_80x24");
}

#[test]
fn sessions_picker_open_via_palette() {
    let mut h = h();
    h.press(KeyCode::Char('p'), KeyModifiers::CONTROL);
    h.type_text("sessions");
    h.press(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(h.mode(), Mode::Sessions);
    h.render();
    h.assert_screen("modal_sessions_80x24");
}

#[test]
fn thinking_picker_open_via_palette() {
    let mut h = h();
    h.press(KeyCode::Char('p'), KeyModifiers::CONTROL);
    h.type_text("thinking");
    h.press(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(h.mode(), Mode::Thinking);
    h.render();
    h.assert_screen("modal_thinking_80x24");
}

#[test]
fn debug_modal_open() {
    let mut h = h();
    h.press(KeyCode::Char('d'), KeyModifiers::CONTROL);
    h.render();
    h.assert_screen("modal_debug_80x24");
}

#[test]
fn slash_command_palette_on_empty_input() {
    let mut h = h();
    h.press(KeyCode::Char('/'), KeyModifiers::NONE);
    assert_eq!(h.mode(), Mode::Commands);
    h.render();
    h.assert_screen("modal_slash_palette_80x24");
}

// -----------------------------------------------------------------------------
// Streaming response (state-driven, no real network call)
// -----------------------------------------------------------------------------

#[test]
fn streaming_response_thinking_placeholder() {
    let mut h = h();
    h.begin_thinking_response("Hello, world");
    h.render();
    h.assert_screen("stream_thinking_placeholder_80x24");
}

#[test]
fn streaming_response_after_delta() {
    let mut h = h();
    h.begin_thinking_response("Hello, world");
    h.inject(AsyncEvent::Agent(AgentEvent::ModelDelta {
        text: "Hi there!".to_string(),
    }));
    h.render();
    h.assert_screen("stream_after_delta_80x24");
}

#[test]
fn streaming_response_finalized() {
    let mut h = h();
    h.begin_thinking_response("Hello, world");
    h.inject(AsyncEvent::Agent(AgentEvent::ModelDelta {
        text: "Hi there!".to_string(),
    }));
    h.inject(AsyncEvent::TurnCompleted(Ok("Hi there!".to_string())));
    h.render();
    h.assert_screen("stream_finalized_80x24");
}

#[test]
fn streaming_response_error_turn() {
    let mut h = h();
    h.begin_thinking_response("Hello, world");
    h.inject(AsyncEvent::Agent(AgentEvent::ModelDelta {
        text: "Partial ".to_string(),
    }));
    h.inject(AsyncEvent::TurnCompleted(Err("rate limited".to_string())));
    h.render();
    h.assert_screen("stream_error_80x24");
}

#[test]
fn tool_approval_pending_in_chat() {
    let mut h = h();
    h.begin_thinking_response("read the file");
    h.inject(AsyncEvent::Agent(AgentEvent::ToolRequested(
        navi_sdk::ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({ "path": "src/main.rs" }),
        },
    )));
    h.inject(AsyncEvent::Agent(AgentEvent::ApprovalRequested(
        navi_sdk::ApprovalRequest {
            id: "ap-1".to_string(),
            summary: "read src/main.rs".to_string(),
            risk: navi_sdk::ApprovalRisk::Command,
        },
    )));
    assert_eq!(h.pending_approvals().len(), 1);
    h.render();
    h.assert_screen("stream_tool_approval_pending_80x24");
}

#[test]
fn tool_completion_message_in_chat() {
    let mut h = h();
    h.begin_thinking_response("read the file");
    h.push_message(ChatMessage {
        status: Some("tool result".to_string()),
        tool_invocation: Some(navi_sdk::ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({ "path": "src/main.rs" }),
        }),
        tool_result: Some(navi_sdk::ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!("fn main() {}"),
        }),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });
    h.inject(AsyncEvent::Agent(AgentEvent::ToolCompleted(
        navi_sdk::ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!("fn main() {}"),
        },
    )));
    h.render();
    h.assert_screen("stream_tool_completed_80x24");
}

// -----------------------------------------------------------------------------
// Visual review: tool policy, block selection, background tasks
// -----------------------------------------------------------------------------

fn review_dir() -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-review");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[test]
fn ui_review_tool_policy_auto_vs_collapsed() {
    let mut h = h_size(100, 32);
    // No sent_at here: wall-clock is TZ-dependent and covered by unit tests.
    // Layout of `›` prefix is what this screenshot locks.
    h.push_message(ChatMessage::new(
        ChatRole::User,
        "apply the patch and list files".to_string(),
    ));
    // Useful: write/patch auto-expands
    h.push_message(ChatMessage {
        status: Some("tool result".to_string()),
        tool_invocation: Some(navi_sdk::ToolInvocation {
            id: "w1".to_string(),
            tool_name: "write_file".to_string(),
            input: serde_json::json!({
                "path": "src/lib.rs",
                "content": "fn main() {\n    println!(\"hi\");\n}\n"
            }),
        }),
        tool_result: Some(navi_sdk::ToolResult {
            invocation_id: "w1".to_string(),
            ok: true,
            output: serde_json::json!({
                "path": "src/lib.rs",
                "lines_added": 3,
                "lines_removed": 0
            }),
        }),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });
    // Noise: bash stays collapsed by default
    h.push_message(ChatMessage {
        status: Some("tool result".to_string()),
        tool_invocation: Some(navi_sdk::ToolInvocation {
            id: "b1".to_string(),
            tool_name: "bash".to_string(),
            input: serde_json::json!({ "command": "cargo test -p navi-tui" }),
        }),
        tool_result: Some(navi_sdk::ToolResult {
            invocation_id: "b1".to_string(),
            ok: true,
            output: serde_json::json!({
                "stdout": "running 256 tests\nok",
                "stderr": "",
                "exit_code": 0
            }),
        }),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });
    // Noise: read collapsed
    h.push_message(ChatMessage {
        status: Some("tool result".to_string()),
        tool_invocation: Some(navi_sdk::ToolInvocation {
            id: "r1".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({ "path": "README.md" }),
        }),
        tool_result: Some(navi_sdk::ToolResult {
            invocation_id: "r1".to_string(),
            ok: true,
            output: serde_json::json!({
                "path": "README.md",
                "content": "# NAVI\n\nlocal agentic engine\n"
            }),
        }),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });
    h.push_message(ChatMessage::new(
        ChatRole::Assistant,
        "Done — write is open, shell/read stay compact.".to_string(),
    ));
    h.render();
    h.dump_screen(review_dir().join("01_tool_policy_smart.txt"));
    h.assert_screen("ui_tool_policy_smart_100x32");
}

#[test]
fn ui_review_block_selection_highlight() {
    let mut h = h_size(80, 24);
    h.push_message(ChatMessage::new(
        ChatRole::User,
        "first user block".to_string(),
    ));
    h.push_message(ChatMessage::new(
        ChatRole::Assistant,
        "assistant reply block with **markdown**.".to_string(),
    ));
    h.push_message(ChatMessage {
        status: Some("tool result".to_string()),
        tool_invocation: Some(navi_sdk::ToolInvocation {
            id: "g1".to_string(),
            tool_name: "grep".to_string(),
            input: serde_json::json!({ "pattern": "TODO", "path": "src" }),
        }),
        tool_result: Some(navi_sdk::ToolResult {
            invocation_id: "g1".to_string(),
            ok: true,
            output: serde_json::json!({ "matches": [] }),
        }),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });
    // Select the assistant message block (index 1).
    h.select_message_block(1);
    h.render();
    h.dump_screen(review_dir().join("02_block_selection.txt"));
    h.assert_screen("ui_block_selection_80x24");
}

#[test]
fn ui_review_background_tasks_modal() {
    let mut h = h_size(80, 24);
    h.set_background_commands(vec![
        BackgroundCommandSnapshot {
            task_id: "bg_1".to_string(),
            command: "cargo test -p navi-tui -- --test-threads=4".to_string(),
            description: Some("run tui tests".to_string()),
            status: BackgroundTaskStatus::Running,
            elapsed_ms: 4_700,
            timeout_ms: 300_000,
            exit_code: None,
            stdout: "running\n".to_string(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            error: None,
        },
        BackgroundCommandSnapshot {
            task_id: "bg_2".to_string(),
            command: "sleep 5".to_string(),
            description: Some("finished sleep".to_string()),
            status: BackgroundTaskStatus::Completed,
            elapsed_ms: 5_000,
            timeout_ms: 60_000,
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            error: None,
        },
    ]);
    h.press(KeyCode::Char('t'), KeyModifiers::CONTROL);
    assert_eq!(h.mode(), Mode::BackgroundCommands);
    h.render();
    h.dump_screen(review_dir().join("03_background_tasks_modal.txt"));
    h.assert_screen("ui_background_tasks_modal_80x24");
}

#[test]
fn ui_review_ctrl_o_keeps_opened_bash() {
    let mut h = h_size(100, 28);
    h.push_message(ChatMessage::new(
        ChatRole::User,
        "run tests and keep shell open".to_string(),
    ));
    h.push_message(ChatMessage {
        status: Some("tool result".to_string()),
        tool_invocation: Some(navi_sdk::ToolInvocation {
            id: "bash-open".to_string(),
            tool_name: "bash".to_string(),
            input: serde_json::json!({ "command": "echo keep-me-open" }),
        }),
        tool_result: Some(navi_sdk::ToolResult {
            invocation_id: "bash-open".to_string(),
            ok: true,
            output: serde_json::json!({
                "stdout": "keep-me-open",
                "stderr": "",
                "exit_code": 0
            }),
        }),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });
    // User opens the noisy bash tool, selects it, then cycles Ctrl+O.
    h.expand_tool("bash-open");
    h.select_tool_block("bash-open");
    h.press(KeyCode::Char('o'), KeyModifiers::CONTROL); // expand all
    h.press(KeyCode::Char('o'), KeyModifiers::CONTROL); // back to smart
    h.render();
    h.dump_screen(review_dir().join("04_ctrl_o_preserves_open_tool.txt"));
    h.assert_screen("ui_ctrl_o_preserves_open_tool_100x28");
    let text = h.buffer_text();
    assert!(
        text.contains("keep-me-open"),
        "Ctrl+O cycle must not close the opened tool:\n{text}"
    );
}
