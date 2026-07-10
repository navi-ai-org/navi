//! End-to-end scenario tests that drive the TUI through its real submit
//! path against a [`MockEngine`]. These tests run under `#[tokio::test]`
//! because the TUI's `start_streaming_request` spawns an async turn task.

use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers};
use navi_tui::testing::{
    EngineCall, Harness, MockEngine, RuntimeEvent, RuntimeEventKind, TestConfig,
};
use tokio::time::sleep;

/// Default poll deadline for scenario engine-call waits (was 5s; 3s fails
/// fast under CI starvation without multi-second idle waits).
const CALL_WAIT: Duration = Duration::from_secs(3);

/// Poll until the mock has recorded at least `count` calls. Panics on timeout.
async fn wait_for_calls(mock: &MockEngine, count: usize) {
    let deadline = std::time::Instant::now() + CALL_WAIT;
    while mock.call_count() < count {
        if std::time::Instant::now() >= deadline {
            panic!(
                "timed out waiting for {count} engine calls (have {})",
                mock.call_count()
            );
        }
        // Yield first so the multi-thread runtime can run the TUI turn task.
        tokio::task::yield_now().await;
        sleep(Duration::from_millis(1)).await;
    }
}

/// Let the tokio runtime process queued events, then drain them from the
/// TUI's async bridge. Loops until no new events arrive within a tick.
async fn flush_events(h: &mut Harness) {
    // Slightly more budget than the old 20×5ms worst-case, but exit early
    // when idle so wall-clock stays low on the happy path.
    for _ in 0..30 {
        tokio::task::yield_now().await;
        sleep(Duration::from_millis(2)).await;
        let processed = h.drain_async_events();
        if processed == 0 {
            sleep(Duration::from_millis(3)).await;
            if h.drain_async_events() == 0 {
                break;
            }
        }
    }
}

fn default_config() -> TestConfig {
    TestConfig::default()
}

/// Build a harness with a fresh mock, discarding construction-time call noise
/// (`list_skills`, `credential_status`, …) so `wait_for_calls` counts only
/// the turn path under test.
fn harness_with_mock() -> (Harness, Arc<MockEngine>) {
    let mock = Arc::new(MockEngine::new());
    let h = Harness::with_engine(default_config(), mock.clone());
    let _ = mock.take_calls();
    (h, mock)
}

// ─── Streaming text response ───────────────────────────────────────────────

// multi_thread: submit path uses block_in_place in stream/session helpers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_stream_text_response() {
    let (mut h, mock) = harness_with_mock();

    h.type_text("hi");
    h.submit();

    // start_session + subscribe_events + send_turn
    wait_for_calls(&mock, 3).await;

    // Push deltas + a turn-completed lifecycle event before unblocking
    // send_turn, so the trailing drain in `run_sdk_turn` forwards them.
    mock.push_event(RuntimeEvent::new(RuntimeEventKind::AssistantDelta {
        text: "Hello, ".to_string(),
    }));
    mock.push_event(RuntimeEvent::new(RuntimeEventKind::AssistantDelta {
        text: "world!".to_string(),
    }));
    mock.push_event(RuntimeEvent::new(RuntimeEventKind::TurnCompleted {
        turn_id: "t1".to_string(),
        text: "Hello, world!".to_string(),
    }));

    mock.complete_turn();
    flush_events(&mut h).await;

    // Streamed assistant text is present (a local recap may follow as a later msg).
    assert!(
        h.messages()
            .iter()
            .any(|m| m.content.contains("Hello, world!")),
        "expected streamed assistant text; messages={:?}",
        h.messages()
            .iter()
            .map(|m| (&m.content, &m.status))
            .collect::<Vec<_>>()
    );
    // The TUI should no longer be loading.
    assert!(!h.is_loading());

    // The mock recorded the calls we expected.
    let calls = mock.calls();
    assert!(matches!(calls[0], EngineCall::StartSession(_)));
    assert!(matches!(calls[1], EngineCall::SubscribeEvents(_)));
    assert!(matches!(calls[2], EngineCall::SendTurn(_)));
}

// ─── Tool approval flow ────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_tool_approval_approve() {
    let (mut h, mock) = harness_with_mock();

    h.type_text("read the file");
    h.submit();
    wait_for_calls(&mock, 3).await;

    // Engine requests a tool invocation + approval
    mock.push_event(RuntimeEvent::new(RuntimeEventKind::ToolRequested(
        navi_sdk::ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({ "path": "src/main.rs" }),
        },
    )));
    mock.push_event(RuntimeEvent::new(RuntimeEventKind::ApprovalRequired(
        navi_sdk::ApprovalRequest {
            id: "ap-1".to_string(),
            summary: "read src/main.rs".to_string(),
            risk: navi_sdk::ApprovalRisk::Command,
        },
    )));

    flush_events(&mut h).await;

    // The TUI should have one pending approval.
    assert_eq!(h.pending_approvals().len(), 1);
    assert_eq!(h.pending_approvals()[0].id, "ap-1");

    // User presses 'y' to approve.
    h.press(KeyCode::Char('y'), KeyModifiers::NONE);

    // Wait for the engine to record the resolve_approval call.
    wait_for_calls(&mock, 4).await;
    let calls = mock.calls();
    assert!(matches!(
        calls.last().unwrap(),
        EngineCall::ResolveApproval {
            decision: navi_sdk::ApprovalDecision::Approved { id },
            ..
        } if id == "ap-1"
    ));
    assert!(h.pending_approvals().is_empty());

    mock.complete_turn();
    flush_events(&mut h).await;
}

// ─── Error path ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_turn_error() {
    let (mut h, mock) = harness_with_mock();

    h.type_text("hi");
    h.submit();
    wait_for_calls(&mock, 3).await;

    // Push a delta then an error.
    mock.push_event(RuntimeEvent::new(RuntimeEventKind::AssistantDelta {
        text: "partial ".to_string(),
    }));
    mock.push_event(RuntimeEvent::new(RuntimeEventKind::Error {
        message: "rate limited".to_string(),
    }));

    mock.complete_turn();
    flush_events(&mut h).await;

    // TUI should be no longer loading.
    assert!(!h.is_loading());
    // The error should have produced an AgentEvent::Error in the message log
    // (a local recap may append after the error row).
    assert!(
        h.messages().iter().any(|m| {
            m.content.contains("rate limited")
                || m.content.contains("error")
                || m.status.as_deref() == Some("error")
        }),
        "expected error message; messages={:?}",
        h.messages()
            .iter()
            .map(|m| (&m.content, &m.status))
            .collect::<Vec<_>>()
    );
}

// ─── Multi-turn conversation history ───────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_conversation_history_after_turn() {
    let (mut h, mock) = harness_with_mock();

    let initial_history_len = h.conversation_history_len();

    h.type_text("hi");
    h.submit();
    wait_for_calls(&mock, 3).await;

    mock.push_event(RuntimeEvent::new(RuntimeEventKind::AssistantDelta {
        text: "Hello!".to_string(),
    }));
    mock.complete_turn();
    flush_events(&mut h).await;

    // After a turn completes, conversation_history should have a new
    // assistant message appended.
    let final_history_len = h.conversation_history_len();
    assert!(final_history_len > initial_history_len);
}

// ─── Cancel mid-stream ─────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_cancel_mid_stream() {
    let (mut h, mock) = harness_with_mock();

    h.type_text("hi");
    h.submit();
    wait_for_calls(&mock, 3).await;

    // The TUI is now loading. Esc opens a confirmation modal; Enter confirms.
    assert!(h.is_loading());
    h.press(KeyCode::Esc, KeyModifiers::NONE);
    assert!(h.is_loading());
    h.press(KeyCode::Enter, KeyModifiers::NONE);
    flush_events(&mut h).await;

    // Loading state should be cleared by the cancel path.
    assert!(!h.is_loading());
}

// temporary - ignore
