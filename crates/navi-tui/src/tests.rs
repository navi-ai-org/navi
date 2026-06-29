use crate::TuiApp;
use crate::chat::{
    active_assistant_message, ensure_tail_model_response, finalize_active_assistant,
    fork_from_user_message, remove_active_tool_placeholder, revert_to_user_message, submit_message,
    update_active_assistant_status,
};
use crate::commands::CommandAction;
use crate::commands::filtered_commands;
use crate::dispatch::{AsyncEvent, handle_async_event};
use crate::errors::handle_model_error;
use crate::input::insert_input_char;
use crate::keybindings::{
    handle_command_key, handle_help_key, handle_key, handle_model_key, handle_normal_key,
    handle_settings_key, open_modal, open_model_picker, run_selected_command, sync_models_tui,
};
use crate::mouse::{finish_selection, handle_mouse, selected_text};
use crate::notifications::expire_notification;
use crate::providers::{
    ListRow, build_model_rows, first_model_index, model_is_available_for_selection,
    sync_scroll_to_selection,
};
use crate::render::command_scroll_offset;
use crate::render::markdown::is_empty_tool_placeholder;
use crate::render::tool::tool_full_content;
use crate::state::{
    ChatLineSource, ChatMessage, ChatRole, ModalKind, Mode, Notification, SelectionState,
};
use crate::theme::NOTIFICATION_TTL;
use crate::tools::record_tool_requested;
use crate::ui::interaction::HitAction;
use crate::ui::text_input::{
    floor_char_boundary, next_char_boundary, next_hump_boundary, previous_char_boundary,
    previous_hump_boundary,
};
use crate::view::build_chat_lines;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use navi_sdk::{
    AgentEvent, LoadedConfig, ModelMessage, ModelOption, SessionId, SessionSnapshot,
    SubagentTranscriptItem, SubagentTranscriptKind, ToolInvocation, ToolResult,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::prelude::Line;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) fn test_app(input: &str) -> TuiApp {
    let mut app = TuiApp::new(
        LoadedConfig {
            config: navi_sdk::NaviConfig::default(),
            global_config_path: None,
            project_config_path: None,
            data_dir: PathBuf::from("/tmp/navi-test"),
        },
        PathBuf::from("/tmp/test-project"),
        None,
    )
    .expect("test app");
    let _ = app.credential_store().delete_api_key("commandcode");
    app.input = input.to_string();
    app.input_cursor = app.input.len();
    app.mode = Mode::Normal;
    app
}

fn app_with_missing_provider_key() -> TuiApp {
    let mut config = navi_sdk::NaviConfig::default();
    config.model.provider = "test-provider".to_string();
    config.model.name = "test-large".to_string();
    config.providers = vec![navi_sdk::ProviderConfig {
        id: "test-provider".to_string(),
        label: "Test Provider".to_string(),
        description: "test provider".to_string(),
        kind: navi_sdk::ProviderKind::OpenAiChatCompletions,
        api_key_env: "NAVI_TEST_MISSING_PROVIDER_KEY".to_string(),
        base_url: Some("https://example.com/v1".to_string()),
        models: vec![navi_sdk::ProviderModelConfig {
            name: "test-large".to_string(),
            task_size: navi_sdk::ModelTaskSize::Large,
            context_window_tokens: None,
            max_output_tokens: None,
            recommended_temperature: None,
            supports_thinking: None,
            tool_prompt_manifest: None,
        }],
        ..Default::default()
    }];

    TuiApp::new(
        LoadedConfig {
            config,
            global_config_path: None,
            project_config_path: None,
            data_dir: PathBuf::from("/tmp/navi-test-missing-key"),
        },
        PathBuf::from("/tmp/test-project"),
        None,
    )
    .expect("test app")
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn seed_chat_cache(app: &mut TuiApp, lines: &[&str]) {
    let mut cache = app.chat_render_cache.borrow_mut();
    cache.lines = lines
        .iter()
        .map(|line| Line::from((*line).to_string()))
        .collect();
    cache.chat_rect = Some(Rect::new(0, 0, 80, lines.len() as u16));
}

fn terminal_buffer_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let area = buffer.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(cell) = buffer.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

#[test]
fn root_render_keeps_header_inside_viewport_margin() {
    let mut app = test_app("");
    app.git_branch = Some("main".to_string());
    app.compact_state.context_window = 200_000;

    let backend = TestBackend::new(40, 12);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| crate::view::render(frame, &mut app))
        .expect("draw");

    let buffer = terminal.backend().buffer();
    assert_eq!(buffer.cell((39, 0)).expect("cell").symbol(), " ");
}

#[test]
fn finalize_active_assistant_tracks_response_as_pending_context() {
    let mut app = test_app("");
    app.compact_state.update_usage(2_000);
    app.messages.push(ChatMessage {
        status: Some("receiving".to_string()),
        ..ChatMessage::new(ChatRole::Assistant, "final response".to_string())
    });

    finalize_active_assistant(&mut app, 100, "");

    assert_eq!(
        app.compact_state.estimated_unsent_bytes,
        "final response".len()
    );
    assert_eq!(
        app.compact_state.total_estimated_tokens(0),
        2_000 + ("final response".len() as u64).div_ceil(4)
    );
}

#[test]
fn finalize_active_assistant_uses_turn_text_when_deltas_were_not_seen() {
    let mut app = test_app("");
    app.messages.push(ChatMessage {
        status: Some("thinking".to_string()),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });

    finalize_active_assistant(&mut app, 100, "final answer");

    assert_eq!(
        app.messages.last().map(|message| message.content.as_str()),
        Some("final answer")
    );
    assert!(app.events.iter().any(|event| matches!(
        event,
        AgentEvent::ModelOutput { text, .. } if text == "final answer"
    )));
}

#[test]
fn selected_text_extracts_multiline_text_from_chat_cache() {
    let mut app = test_app("");
    seed_chat_cache(&mut app, &["hello world", "second line"]);
    app.selection = Some(SelectionState {
        start: (0, 6),
        end: (1, 6),
        active: false,
    });

    assert_eq!(selected_text(&app).as_deref(), Some("world\nsecond"));
}

#[test]
fn zero_width_mouse_selection_does_not_copy_text() {
    let mut app = test_app("");
    seed_chat_cache(&mut app, &["hello world"]);
    app.selection = Some(SelectionState {
        start: (0, 3),
        end: (0, 3),
        active: true,
    });

    assert!(!finish_selection(&mut app, None));
    assert_eq!(selected_text(&app), None);
}

#[test]
fn mouse_up_outside_chat_finishes_existing_selection() {
    let mut app = test_app("");
    seed_chat_cache(&mut app, &["hello world"]);
    app.selection = Some(SelectionState {
        start: (0, 0),
        end: (0, 5),
        active: true,
    });

    assert!(finish_selection(&mut app, None));
    assert_eq!(selected_text(&app).as_deref(), Some("hello"));
    assert!(!app.selection.as_ref().unwrap().active);
}

#[test]
fn ctrl_shift_c_copies_selection_from_lowercase_shift_key_event() {
    let mut app = test_app("");
    seed_chat_cache(&mut app, &["hello world"]);
    app.selection = Some(SelectionState {
        start: (0, 0),
        end: (0, 5),
        active: false,
    });

    let should_quit = handle_key(
        &mut app,
        KeyCode::Char('c'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );

    assert!(!should_quit);
    let notification = app.notification().expect("clipboard notification");
    assert_eq!(notification.title, "Clipboard");
}

#[test]
fn ctrl_shift_c_reaches_global_shortcut_from_question_mode() {
    let mut app = test_app("");
    seed_chat_cache(&mut app, &["hello world"]);
    app.mode = Mode::Question;
    app.selection = Some(SelectionState {
        start: (0, 6),
        end: (0, 11),
        active: false,
    });

    let should_quit = handle_key(&mut app, KeyCode::Char('C'), KeyModifiers::CONTROL);

    assert!(!should_quit);
    let notification = app.notification().expect("clipboard notification");
    assert_eq!(notification.title, "Clipboard");
}

#[test]
fn mouse_down_on_user_chat_message_opens_message_actions() {
    let mut app = test_app("");
    app.messages
        .push(ChatMessage::new(ChatRole::User, "change this".to_string()));
    app.register_hit(
        Rect::new(0, 0, 20, 1),
        5,
        "user message",
        HitAction::ChatMessage(0),
    );

    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert_eq!(app.mode, Mode::MessageActions);
    assert_eq!(app.message_action_target, Some(0));
    assert_eq!(app.selected_message_action, 0);
}

#[test]
fn mouse_move_on_chat_hit_tracks_hovered_interactive_block() {
    let mut app = test_app("");
    app.messages
        .push(ChatMessage::new(ChatRole::User, "hover me".to_string()));
    app.register_hit(
        Rect::new(0, 0, 20, 1),
        5,
        "user message",
        HitAction::ChatMessage(0),
    );

    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Moved,
            column: 2,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert_eq!(app.hovered_chat_source, Some(ChatLineSource::Message(0)));

    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Moved,
            column: 40,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert_eq!(app.hovered_chat_source, None);
}

#[test]
fn revert_to_user_message_truncates_cacheable_prefix_without_rewriting_it() {
    let mut app = test_app("");
    let system = app.conversation_history[0].clone();
    app.messages = vec![
        ChatMessage::new(ChatRole::User, "first".to_string()),
        ChatMessage::new(ChatRole::Assistant, "answer one".to_string()),
        ChatMessage::new(ChatRole::User, "second".to_string()),
        ChatMessage::new(ChatRole::Assistant, "answer two".to_string()),
    ];
    app.conversation_history = vec![
        system,
        ModelMessage::user("first"),
        ModelMessage::assistant("answer one"),
        ModelMessage::user("second"),
        ModelMessage::assistant("answer two"),
    ];
    app.events = vec![
        AgentEvent::UserTaskSubmitted {
            text: "first".to_string(),
            content_parts: vec![],
        },
        AgentEvent::ModelOutput {
            text: "answer one".to_string(),
            thinking: None,
        },
        AgentEvent::UserTaskSubmitted {
            text: "second".to_string(),
            content_parts: vec![],
        },
        AgentEvent::ModelOutput {
            text: "answer two".to_string(),
            thinking: None,
        },
    ];

    revert_to_user_message(&mut app, 2).expect("revert");

    assert_eq!(app.input, "second");
    assert_eq!(app.messages.len(), 2);
    assert_eq!(app.conversation_history.len(), 3);
    assert_eq!(app.events.len(), 2);
    assert_eq!(app.conversation_history[1].content, "first");
    assert_eq!(app.conversation_history[2].content, "answer one");
}

#[test]
fn fork_from_user_message_saves_original_and_keeps_prefix_in_new_session() {
    let mut app = test_app("");
    app.session_id = SessionId::new("old-session".to_string());
    let system = app.conversation_history[0].clone();
    app.messages = vec![
        ChatMessage::new(ChatRole::User, "first".to_string()),
        ChatMessage::new(ChatRole::Assistant, "answer one".to_string()),
        ChatMessage::new(ChatRole::User, "second".to_string()),
    ];
    app.conversation_history = vec![
        system,
        ModelMessage::user("first"),
        ModelMessage::assistant("answer one"),
        ModelMessage::user("second"),
    ];
    app.events = vec![
        AgentEvent::UserTaskSubmitted {
            text: "first".to_string(),
            content_parts: vec![],
        },
        AgentEvent::ModelOutput {
            text: "answer one".to_string(),
            thinking: None,
        },
        AgentEvent::UserTaskSubmitted {
            text: "second".to_string(),
            content_parts: vec![],
        },
    ];

    fork_from_user_message(&mut app, 2).expect("fork");

    assert_ne!(app.session_id.as_str(), "old-session");
    assert_eq!(app.input, "second");
    assert_eq!(app.messages.len(), 2);
    assert_eq!(app.conversation_history.len(), 3);
    assert_eq!(app.events.len(), 2);
}

#[test]
fn ctrl_n_starts_clean_session_without_old_events_or_context() {
    let mut app = test_app("");
    app.session_id = SessionId::new("old-session".to_string());
    let system = app.conversation_history[0].clone();
    app.messages = vec![
        ChatMessage::new(ChatRole::User, "old prompt".to_string()),
        ChatMessage::new(ChatRole::Assistant, "old answer".to_string()),
    ];
    app.conversation_history = vec![
        system,
        ModelMessage::user("old prompt"),
        ModelMessage::assistant("old answer"),
    ];
    app.events = vec![
        AgentEvent::UserTaskSubmitted {
            text: "old prompt".to_string(),
            content_parts: vec![],
        },
        AgentEvent::ModelOutput {
            text: "old answer".to_string(),
            thinking: None,
        },
    ];
    app.compact_state.add_unsent_bytes(1024);
    app.session_title = Some("Old session".to_string());
    app.running_tools.insert(
        "call-1".to_string(),
        ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "bash".to_string(),
            input: serde_json::json!({"command": "echo old"}),
        },
    );

    handle_key(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);

    assert_ne!(app.session_id.as_str(), "old-session");
    assert!(app.messages.is_empty());
    assert!(app.events.is_empty());
    assert_eq!(app.conversation_history.len(), 1);
    assert!(
        app.conversation_history
            .first()
            .is_some_and(|message| matches!(&message.role, navi_sdk::ModelRole::System))
    );
    assert_eq!(app.compact_state.estimated_unsent_bytes, 0);
    assert!(app.compact_state.last_input_tokens.is_none());
    assert!(app.session_title.is_none());
    assert!(app.running_tools.is_empty());
    assert!(app.tool_invocations.is_empty());
}

#[test]
fn command_palette_new_session_uses_full_session_reset() {
    let mut app = test_app("");
    app.session_id = SessionId::new("old-session".to_string());
    let system = app.conversation_history[0].clone();
    app.messages = vec![
        ChatMessage::new(ChatRole::User, "old prompt".to_string()),
        ChatMessage::new(ChatRole::Assistant, "old answer".to_string()),
    ];
    app.conversation_history = vec![
        system,
        ModelMessage::user("old prompt"),
        ModelMessage::assistant("old answer"),
    ];
    app.events = vec![
        AgentEvent::UserTaskSubmitted {
            text: "old prompt".to_string(),
            content_parts: vec![],
        },
        AgentEvent::ModelOutput {
            text: "old answer".to_string(),
            thinking: None,
        },
    ];
    app.compact_state.context_window = 60_000;
    app.compact_state.last_input_tokens = Some(60_000);
    app.compact_state.add_unsent_bytes(1024);
    app.session_title = Some("Old session".to_string());
    app.running_tools.insert(
        "call-1".to_string(),
        ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "bash".to_string(),
            input: serde_json::json!({"command": "echo old"}),
        },
    );
    app.selected_command = 0;

    run_selected_command(&mut app);

    assert_ne!(app.session_id.as_str(), "old-session");
    assert!(app.messages.is_empty());
    assert!(app.events.is_empty());
    assert_eq!(app.conversation_history.len(), 1);
    assert_eq!(
        app.compact_state.context_window,
        navi_sdk::effective_context_window(&app.loaded_config.config)
    );
    assert_eq!(app.compact_state.estimated_unsent_bytes, 0);
    assert!(app.compact_state.last_input_tokens.is_none());
    assert!(app.session_title.is_none());
    assert!(app.running_tools.is_empty());
    assert!(app.tool_invocations.is_empty());
}

#[test]
fn stale_stream_events_do_not_mutate_new_session() {
    let mut app = test_app("");
    let current_session = app.session_id.as_str().to_string();

    handle_async_event(
        &mut app,
        AsyncEvent::AgentForSession {
            session_id: "old-session".to_string(),
            event: AgentEvent::ModelDelta {
                text: "stale".to_string(),
            },
        },
    );

    assert!(app.messages.is_empty());

    handle_async_event(
        &mut app,
        AsyncEvent::AgentForSession {
            session_id: current_session,
            event: AgentEvent::ModelDelta {
                text: "current".to_string(),
            },
        },
    );

    assert_eq!(app.messages.len(), 1);
    assert_eq!(app.messages[0].content, "current");
}

#[test]
fn loading_saved_session_restores_snapshot_session_id() {
    let mut app = test_app("");
    app.session_id = SessionId::new("transient-session".to_string());
    let snapshot = SessionSnapshot {
        version: SessionSnapshot::CURRENT_VERSION,
        id: SessionId::new("saved-session".to_string()),
        title: Some("Saved".to_string()),
        project: PathBuf::from("/tmp/test-project"),
        created_at: 1,
        updated_at: 2,
        events: vec![AgentEvent::UserTaskSubmitted {
            text: "saved prompt".to_string(),
            content_parts: vec![],
        }],
        memory: None,
        goal: None,
    };

    crate::persistence::load_session(&mut app, &snapshot);

    assert_eq!(app.session_id.as_str(), "saved-session");
    assert_eq!(app.session_title.as_deref(), Some("Saved"));
    assert_eq!(app.messages.len(), 1);
    assert_eq!(app.messages[0].content, "saved prompt");
    assert_eq!(app.conversation_history.len(), 2);
}

#[test]
fn filtered_sessions_prioritizes_current_project_sessions() {
    fn snapshot(id: &str, title: &str, project: &str, updated_at: u64) -> SessionSnapshot {
        SessionSnapshot {
            version: SessionSnapshot::CURRENT_VERSION,
            id: SessionId::new(id.to_string()),
            title: Some(title.to_string()),
            project: PathBuf::from(project),
            created_at: updated_at.saturating_sub(10),
            updated_at,
            events: vec![],
            memory: None,
            goal: None,
        }
    }

    let mut app = test_app("");
    app.project_dir = PathBuf::from("/tmp/test-project");
    app.saved_sessions = vec![
        snapshot("external-new", "External New", "/tmp/other-project", 400),
        snapshot("current-old", "Current Old", "/tmp/test-project", 100),
        snapshot("current-new", "Current New", "/tmp/test-project/.", 300),
        snapshot("external-old", "External Old", "/tmp/another-project", 200),
    ];

    let sessions = app.filtered_sessions();
    let ids = sessions
        .iter()
        .map(|snapshot| snapshot.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        ids,
        vec!["current-new", "current-old", "external-new", "external-old"]
    );
}

#[test]
fn model_delta_after_tool_result_creates_visible_response() {
    let mut app = test_app("");
    let invocation = ToolInvocation {
        id: "call-1".to_string(),
        tool_name: "read_file".to_string(),
        input: serde_json::json!({ "path": "README.md" }),
    };
    let result = ToolResult {
        invocation_id: "call-1".to_string(),
        ok: true,
        output: serde_json::json!({ "content": "readme" }),
    };
    app.messages.push(ChatMessage {
        status: Some("tool result".to_string()),
        tool_invocation: Some(invocation),
        tool_result: Some(result),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });

    let message = ensure_tail_model_response(&mut app);
    message.content.push_str("final answer");

    assert_eq!(app.messages.len(), 2);
    assert_eq!(
        app.messages.last().map(|message| message.content.as_str()),
        Some("final answer")
    );
    let text = build_chat_lines(&mut app, 80)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Read README.md"));
    assert!(text.contains("final answer"));
}

#[test]
fn update_status_after_tool_result_does_not_rewrite_tool_row() {
    let mut app = test_app("");
    app.is_loading = true;
    let invocation = ToolInvocation {
        id: "call-1".to_string(),
        tool_name: "read_file".to_string(),
        input: serde_json::json!({ "path": "README.md" }),
    };
    let result = ToolResult {
        invocation_id: "call-1".to_string(),
        ok: true,
        output: serde_json::json!({ "content": "readme" }),
    };
    app.messages.push(ChatMessage {
        status: Some("tool result".to_string()),
        tool_invocation: Some(invocation),
        tool_result: Some(result),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });

    update_active_assistant_status(&mut app);

    assert_eq!(
        app.messages
            .first()
            .and_then(|message| message.status.as_deref()),
        Some("tool result")
    );
    assert_eq!(
        app.messages
            .last()
            .and_then(|message| message.status.as_deref()),
        Some("thinking")
    );
    assert!(is_empty_tool_placeholder(app.messages.last().unwrap()));
}

#[test]
fn consecutive_tool_requests_share_one_assistant_history_message() {
    let mut app = test_app("");
    let active = ensure_tail_model_response(&mut app);
    active.content = "I need project files.".to_string();
    active.thinking_content = "hidden reasoning".to_string();

    let first = ToolInvocation {
        id: "call-1".to_string(),
        tool_name: "fs_browser".to_string(),
        input: serde_json::json!({"action": "list"}),
    };
    let second = ToolInvocation {
        id: "call-2".to_string(),
        tool_name: "read_file".to_string(),
        input: serde_json::json!({ "path": "README.md" }),
    };

    record_tool_requested(&mut app, first);
    record_tool_requested(&mut app, second);

    assert_eq!(app.conversation_history.len(), 2);
    let assistant = app
        .conversation_history
        .last()
        .expect("assistant tool call");
    assert_eq!(assistant.content, "I need project files.");
    assert_eq!(
        assistant.thinking_content.as_deref(),
        Some("hidden reasoning")
    );
    assert_eq!(assistant.tool_calls.len(), 2);
    assert_eq!(assistant.tool_calls[0].id, "call-1");
    assert_eq!(assistant.tool_calls[1].id, "call-2");
}

#[test]
fn camel_hump_next_boundary_splits_identifiers_and_words() {
    let value = "fooBar_bazQUX99 alpha";

    assert_eq!(next_hump_boundary(value, 0), 3);
    assert_eq!(next_hump_boundary(value, 3), 6);
    assert_eq!(next_hump_boundary(value, 7), 10);
    assert_eq!(next_hump_boundary(value, 10), 13);
    assert_eq!(next_hump_boundary(value, 13), 15);
    assert_eq!(next_hump_boundary(value, 15), value.len());
}

#[test]
fn camel_hump_previous_boundary_splits_identifiers_and_words() {
    let value = "fooBar_bazQUX99 alpha";

    assert_eq!(previous_hump_boundary(value, value.len()), 16);
    assert_eq!(previous_hump_boundary(value, 16), 13);
    assert_eq!(previous_hump_boundary(value, 13), 10);
    assert_eq!(previous_hump_boundary(value, 10), 7);
    assert_eq!(previous_hump_boundary(value, 7), 3);
    assert_eq!(previous_hump_boundary(value, 3), 0);
}

#[test]
fn char_boundary_helpers_handle_multibyte_input() {
    let value = "abçDef";
    let after_cedilla = "abç".len();

    assert_eq!(next_hump_boundary(value, 0), after_cedilla);
    assert_eq!(next_char_boundary(value, 2), Some(after_cedilla));
    assert_eq!(previous_char_boundary(value, after_cedilla), Some(2));
    assert_eq!(floor_char_boundary(value, after_cedilla - 1), 2);
}

#[test]
fn control_backspace_aliases_delete_previous_camel_hump() {
    for code in [
        KeyCode::Backspace,
        KeyCode::Char('h'),
        KeyCode::Char('w'),
        KeyCode::Char('\u{7f}'),
    ] {
        let mut app = test_app("cargo test -p navi_tui");
        handle_normal_key(&mut app, code, KeyModifiers::CONTROL);
        assert_eq!(app.input, "cargo test -p navi_");
        assert_eq!(app.input_cursor, "cargo test -p navi_".len());

        handle_normal_key(&mut app, code, KeyModifiers::CONTROL);
        assert_eq!(app.input, "cargo test -p ");
        assert_eq!(app.input_cursor, "cargo test -p ".len());
    }
}

#[test]
fn alt_backspace_deletes_until_previous_space_not_separator() {
    for code in [
        KeyCode::Backspace,
        KeyCode::Char('h'),
        KeyCode::Char('\u{7f}'),
    ] {
        let mut app = test_app("cargo test -p navi_tui");
        handle_normal_key(&mut app, code, KeyModifiers::ALT);
        assert_eq!(app.input, "cargo test -p ");
        assert_eq!(app.input_cursor, "cargo test -p ".len());

        handle_normal_key(&mut app, code, KeyModifiers::ALT);
        assert_eq!(app.input, "cargo test ");
        assert_eq!(app.input_cursor, "cargo test ".len());
    }
}

#[test]
fn alt_comma_and_period_move_by_camel_humps() {
    let mut app = test_app("fooBar");

    handle_normal_key(&mut app, KeyCode::Char(','), KeyModifiers::ALT);
    assert_eq!(app.input_cursor, 3);

    handle_normal_key(&mut app, KeyCode::Char('.'), KeyModifiers::ALT);
    assert_eq!(app.input_cursor, 6);
}

#[test]
fn control_arrows_stop_at_camel_humps_and_special_characters() {
    let mut app = test_app("fooBar_baz");
    app.input_cursor = 0;

    handle_normal_key(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
    assert_eq!(app.input_cursor, 3);

    handle_normal_key(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
    assert_eq!(app.input_cursor, 6);

    handle_normal_key(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
    assert_eq!(app.input_cursor, 7);

    handle_normal_key(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
    assert_eq!(app.input_cursor, 10);

    handle_normal_key(&mut app, KeyCode::Left, KeyModifiers::CONTROL);
    assert_eq!(app.input_cursor, 7);

    handle_normal_key(&mut app, KeyCode::Left, KeyModifiers::CONTROL);
    assert_eq!(app.input_cursor, 6);

    handle_normal_key(&mut app, KeyCode::Left, KeyModifiers::CONTROL);
    assert_eq!(app.input_cursor, 3);

    handle_normal_key(&mut app, KeyCode::Left, KeyModifiers::CONTROL);
    assert_eq!(app.input_cursor, 0);
}

#[test]
fn submit_without_provider_adds_error_message() {
    let mut app = test_app("hello");
    app.provider_configured = false;
    submit_message(&mut app);
    assert_eq!(app.messages.len(), 2); // user + error
    assert_eq!(app.messages[0].role, ChatRole::User);
    assert_eq!(app.messages[1].role, ChatRole::Assistant);
    assert!(app.messages[1].content.contains("No API key"));
}

#[test]
fn missing_api_key_does_not_open_prompt_on_startup() {
    let app = app_with_missing_provider_key();

    assert_eq!(app.mode, Mode::Normal);
    assert!(!app.provider_configured);
    assert!(app.pending_model_selection.is_none());
}

#[test]
fn selecting_model_without_provider_key_opens_key_prompt() {
    let mut app = app_with_missing_provider_key();
    app.mode = Mode::Models;

    // Models from unauthenticated providers (except free/public ones) are
    // hidden in the picker. The selected test-provider model is filtered out,
    // so Enter is a no-op and mode stays Models.
    handle_model_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.mode, Mode::Models);
    // Verify the test-provider model is not in the visible rows
    let rows = build_model_rows(&app);
    assert!(!rows.iter().any(|row| match row {
        ListRow::Model { index } => app.models[*index].provider_id == "test-provider",
        _ => false,
    }));
}

#[test]
fn model_picker_filters_by_model_and_provider_text() {
    let mut app = test_app("");
    app.models.push(ModelOption {
        name: "DeepSeek V4 Flash Free".to_string(),
        provider_id: "opencode".to_string(),
        provider_label: "OpenCode Zen".to_string(),
        provider_description: "Free public models".to_string(),
        task_size: navi_sdk::ModelTaskSize::Small,
        context_window_tokens: None,
    });
    open_model_picker(&mut app);

    // Only free/public models should appear without API keys
    app.model_filter = "gemini".to_string();
    let rows = build_model_rows(&app);
    // Gemini models require an API key, so they should be filtered out
    assert!(!rows.iter().any(|row| match row {
        ListRow::Header { label, .. } => label.contains("Gemini"),
        ListRow::Model { .. } => false,
    }));
    assert!(!rows.iter().any(|row| match row {
        ListRow::Model { index } => app.models[*index].name.contains("gemini"),
        ListRow::Header { .. } => false,
    }));

    // Free models should still appear
    app.model_filter = "free".to_string();
    let rows = build_model_rows(&app);
    assert!(rows.iter().any(|row| {
        match row {
            ListRow::Model { index } => app.models[*index]
                .name
                .to_ascii_lowercase()
                .contains("free"),
            ListRow::Header { .. } => false,
        }
    }));
}

#[test]
fn model_scroll_sync_does_not_underflow_near_top() {
    let mut app = test_app("");
    open_model_picker(&mut app);
    let rows = build_model_rows(&app);
    let (selected_row, selected_model) = rows
        .iter()
        .enumerate()
        .find_map(|(row, item)| match item {
            ListRow::Model { index } if row >= 13 => Some((row, *index)),
            _ => None,
        })
        .expect("model near viewport edge");
    app.selected_model = selected_model;
    app.model_scroll = 0;

    sync_scroll_to_selection(&mut app, &rows, 14);

    assert!(app.model_scroll <= selected_row);
}

#[test]
fn model_scroll_sync_clamps_large_scroll_values() {
    let mut app = test_app("");
    open_model_picker(&mut app);
    let rows = build_model_rows(&app);
    app.selected_model = first_model_index(&rows).expect("model");
    app.model_scroll = usize::MAX;

    sync_scroll_to_selection(&mut app, &rows, 14);

    assert!(app.model_scroll <= rows.len().saturating_sub(14));
}

#[test]
fn enter_sends_shift_enter_and_ctrl_j_insert_newlines() {
    let mut app = test_app("one");
    app.provider_configured = false;

    handle_normal_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.messages[0].content, "one");
    assert!(app.input.is_empty());

    let mut app = test_app("one");
    insert_input_char(&mut app, 't');
    insert_input_char(&mut app, 'w');
    insert_input_char(&mut app, 'o');
    handle_normal_key(&mut app, KeyCode::Enter, KeyModifiers::SHIFT);
    insert_input_char(&mut app, 't');
    insert_input_char(&mut app, 'h');
    insert_input_char(&mut app, 'r');
    insert_input_char(&mut app, 'e');
    insert_input_char(&mut app, 'e');

    assert_eq!(app.input, "onetwo\nthree");
    assert_eq!(app.input_cursor, app.input.len());

    handle_normal_key(&mut app, KeyCode::Char('j'), KeyModifiers::CONTROL);
    assert_eq!(app.input, "onetwo\nthree\n");
}

#[test]
fn ctrl_enter_sends_non_empty_message() {
    let mut app = test_app("one");
    app.provider_configured = false;
    handle_key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
    assert_eq!(app.messages[0].content, "one");
    assert!(app.input.is_empty());

    let mut app = test_app("two");
    app.provider_configured = false;
    handle_key(&mut app, KeyCode::Char('j'), KeyModifiers::CONTROL);
    assert_eq!(app.input, "two\n");
    assert!(app.messages.is_empty());

    let mut app = test_app("three");
    app.provider_configured = false;
    handle_key(&mut app, KeyCode::Char('\n'), KeyModifiers::CONTROL);
    assert_eq!(app.input, "three\n");
    assert!(app.messages.is_empty());

    let mut app = test_app("four");
    app.provider_configured = false;
    handle_key(&mut app, KeyCode::Char('\r'), KeyModifiers::CONTROL);
    assert_eq!(app.input, "four\n");
    assert!(app.messages.is_empty());
}

#[test]
fn ctrl_a_selects_entire_input_and_typing_replaces_it() {
    let mut app = test_app("replace me");

    handle_key(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.input_selection, Some((0, "replace me".len())));

    handle_normal_key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
    assert_eq!(app.input, "x");
    assert_eq!(app.input_cursor, 1);
    assert!(app.input_selection.is_none());
}

#[test]
fn empty_ctrl_enter_does_not_open_models() {
    let mut app = test_app("");

    handle_key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);

    assert_eq!(app.mode, Mode::Normal);
    assert!(app.messages.is_empty());
}

#[test]
fn ctrl_o_toggles_full_tool_view() {
    let mut app = test_app("");
    assert!(!app.full_tool_view);

    handle_key(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
    assert!(app.full_tool_view);
    assert!(app.notification().is_some());

    handle_key(&mut app, KeyCode::Char('O'), KeyModifiers::CONTROL);
    assert!(!app.full_tool_view);
}

#[test]
fn ctrl_o_in_provider_modal_does_not_toggle_full_tool_view() {
    let mut app = test_app("");
    app.mode = Mode::Commands;
    app.command_filter = "providers".to_string();
    assert!(!run_selected_command(&mut app));
    assert_eq!(app.mode, Mode::Providers);
    assert!(!app.full_tool_view);

    handle_key(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
    assert_eq!(app.mode, Mode::Providers);
    assert!(!app.full_tool_view);

    handle_key(&mut app, KeyCode::Char('O'), KeyModifiers::CONTROL);
    assert_eq!(app.mode, Mode::Providers);
    assert!(!app.full_tool_view);
}

#[test]
fn oauth_started_opens_modal_without_chat_message() {
    let mut app = test_app("");

    handle_async_event(
        &mut app,
        AsyncEvent::OAuthDeviceStarted {
            provider_id: "commandcode".to_string(),
            verification_uri: "https://commandcode.ai/studio/auth/cli?state=test".to_string(),
            user_code: String::new(),
        },
    );

    assert_eq!(app.mode, Mode::OAuth);
    assert!(app.oauth_state.is_some());
    assert!(app.messages.is_empty());
}

#[test]
fn oauth_modal_copy_shortcut_copies_link() {
    let mut app = test_app("");
    handle_async_event(
        &mut app,
        AsyncEvent::OAuthDeviceStarted {
            provider_id: "commandcode".to_string(),
            verification_uri: "https://commandcode.ai/studio/auth/cli?state=test".to_string(),
            user_code: String::new(),
        },
    );

    handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);

    assert_eq!(app.mode, Mode::OAuth);
    assert!(
        app.notification()
            .is_some_and(|notification| notification.title == "Clipboard")
    );
}

#[test]
fn oauth_modal_registers_clickable_link_hit_region() {
    let mut app = test_app("");
    handle_async_event(
        &mut app,
        AsyncEvent::OAuthDeviceStarted {
            provider_id: "commandcode".to_string(),
            verification_uri: "https://commandcode.ai/studio/auth/cli?state=test".to_string(),
            user_code: String::new(),
        },
    );
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| crate::view::render(frame, &mut app))
        .expect("draw");

    let hit = app
        .hit_test(4, 10)
        .or_else(|| app.hit_test(6, 10))
        .or_else(|| app.hit_test(8, 10));
    assert!(matches!(
        hit.map(|hit| hit.action),
        Some(HitAction::OAuthOpen)
    ));
}

#[test]
fn oauth_completion_clears_modal_text_from_next_frame() {
    let mut app = test_app("");
    let mut terminal = Terminal::new(TestBackend::new(96, 24)).expect("terminal");
    handle_async_event(
        &mut app,
        AsyncEvent::OAuthDeviceStarted {
            provider_id: "commandcode".to_string(),
            verification_uri: "https://example.test/stale-oauth-marker".to_string(),
            user_code: String::new(),
        },
    );
    terminal
        .draw(|frame| crate::view::render(frame, &mut app))
        .expect("draw oauth");
    let oauth_frame = terminal_buffer_text(&terminal);
    assert!(oauth_frame.contains("OAuth Login"));
    assert!(oauth_frame.contains("stale-oauth-marker"));

    handle_async_event(
        &mut app,
        AsyncEvent::OAuthCompleted {
            provider_id: "commandcode".to_string(),
            result: Ok(()),
        },
    );
    let notification = app.notification().expect("oauth notification");
    assert_eq!(notification.title, "OAuth");
    assert!(notification.message.contains("credentials saved"));
    assert!(notification.message.contains("provider plan"));
    app.clear_notification();
    terminal
        .draw(|frame| crate::view::render(frame, &mut app))
        .expect("draw normal");

    let normal_frame = terminal_buffer_text(&terminal);
    assert_eq!(app.mode, Mode::Normal);
    assert!(!normal_frame.contains("OAuth Login"));
    assert!(!normal_frame.contains("stale-oauth-marker"));
}

#[test]
fn slash_opens_commands_and_question_mark_opens_help() {
    let mut app = test_app("");
    assert_eq!(app.mode, Mode::Normal);

    // '?' with empty input opens shortcuts help
    handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
    assert_eq!(app.mode, Mode::Help);

    handle_help_key(&mut app, KeyCode::Char('?'));
    assert_eq!(app.mode, Mode::Normal);

    // Esc goes back to normal
    app.mode = Mode::Normal;

    // '/' with empty input opens command palette
    handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
    assert_eq!(app.mode, Mode::Commands);

    // Escape goes back to normal
    app.mode = Mode::Normal;

    // Pressing '?' when input is NOT empty inserts it as text
    app.input = "hello".to_string();
    app.input_cursor = 5;
    handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.input, "hello?");
}

#[test]
fn ctrl_p_opens_commands_and_tab_is_ignored_in_composer() {
    let mut app = test_app("");

    handle_key(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
    assert_eq!(app.mode, Mode::Commands);
    assert!(app.command_filter.is_empty());
    assert_eq!(app.selected_command, 0);

    app.mode = Mode::Normal;
    app.input = "draft".to_string();
    app.input_cursor = app.input.len();
    handle_key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.input, "draft");
}

#[test]
fn ctrl_t_opens_background_tasks_and_ctrl_b_opens_background_agents() {
    let mut app = test_app("");

    handle_key(&mut app, KeyCode::Char('t'), KeyModifiers::CONTROL);
    assert_eq!(app.mode, Mode::BackgroundCommands);

    app.mode = Mode::Normal;
    handle_key(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL);
    assert_eq!(app.mode, Mode::BackgroundModels);
}

#[test]
fn composer_hint_stays_visible_while_typing() {
    let mut app = test_app("typed text");
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).expect("terminal");

    terminal
        .draw(|frame| crate::view::render(frame, &mut app))
        .expect("draw normal");

    let frame = terminal_buffer_text(&terminal);
    assert!(frame.contains("ctrl+p commands"));
    assert!(frame.contains("ctrl+t background tasks"));
    assert!(frame.contains("ctrl+b background agents"));
    assert!(frame.contains("ctrl+v paste image"));
}

#[test]
fn ctrl_dot_and_ctrl_s_open_help_and_sessions() {
    let mut app = test_app("");

    handle_key(&mut app, KeyCode::Char('.'), KeyModifiers::CONTROL);
    assert_eq!(app.mode, Mode::Help);

    handle_key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert_eq!(app.mode, Mode::Help);

    handle_help_key(&mut app, KeyCode::Esc);
    assert_eq!(app.mode, Mode::Normal);

    handle_key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert_eq!(app.mode, Mode::Sessions);
}

#[test]
fn command_palette_opens_providers() {
    let mut app = test_app("");

    app.mode = Mode::Commands;
    app.command_filter = "providers".to_string();
    app.selected_command = 0;
    let provider_commands = filtered_commands(&app);
    assert!(
        provider_commands
            .iter()
            .any(|command| matches!(command.action, CommandAction::Providers))
    );
    assert!(!run_selected_command(&mut app));
    assert_eq!(app.mode, Mode::Providers);
    // The picker now lands on the first non-header (Provider) row instead of
    // the section divider.
    let rows = app.filtered_providers();
    let first_provider_pos = rows
        .iter()
        .position(|r| matches!(r, crate::providers::ProviderListRow::Provider { .. }))
        .expect("at least one provider row");
    assert_eq!(app.selected_provider_setting, first_provider_pos);
    assert_eq!(app.provider_settings_scroll, 0);
}

fn sample_question_request() -> navi_sdk::QuestionRequest {
    navi_sdk::QuestionRequest {
        id: "question-1".to_string(),
        question: "Which direction should NAVI take?".to_string(),
        options: vec![
            navi_sdk::QuestionOption {
                label: "Fast".to_string(),
                description: Some("Smallest implementation".to_string()),
            },
            navi_sdk::QuestionOption {
                label: "Thorough".to_string(),
                description: Some("More validation".to_string()),
            },
        ],
        multiple: false,
        allow_custom: false,
    }
}

async fn wait_for_question_resolution(engine: &crate::testing::MockEngine) {
    use crate::testing::EngineCall;

    for _ in 0..50 {
        if engine
            .calls()
            .iter()
            .any(|call| matches!(call, EngineCall::ResolveQuestion { .. }))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

#[test]
fn question_request_opens_modal() {
    let mut app = test_app("");

    crate::dispatch::handle_async_event(
        &mut app,
        crate::dispatch::AsyncEvent::Agent(
            AgentEvent::QuestionRequested(sample_question_request()),
        ),
    );

    assert_eq!(app.mode, Mode::Question);
    assert_eq!(app.pending_questions.len(), 1);
    assert_eq!(app.pending_questions[0].request.id, "question-1");
}

#[test]
fn question_escape_closes_without_resolving_and_ctrl_enter_reopens() {
    use crate::testing::MockEngine;

    let mut app = test_app("");
    let engine = Arc::new(MockEngine::new());
    app.set_engine(engine.clone());
    crate::dispatch::handle_async_event(
        &mut app,
        crate::dispatch::AsyncEvent::Agent(
            AgentEvent::QuestionRequested(sample_question_request()),
        ),
    );

    handle_key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
    assert_eq!(app.pending_questions[0].custom_answer, "x");

    handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.pending_questions.len(), 1);
    assert_eq!(app.pending_questions[0].custom_answer, "x");
    assert!(engine.calls().is_empty());

    handle_key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);

    assert_eq!(app.mode, Mode::Question);
    assert_eq!(app.pending_questions.len(), 1);
    assert_eq!(app.pending_questions[0].custom_answer, "x");
}

#[tokio::test(flavor = "multi_thread")]
async fn question_enter_resolves_selected_answer() {
    use crate::testing::{EngineCall, MockEngine};

    let mut app = test_app("");
    let engine = Arc::new(MockEngine::new());
    app.set_engine(engine.clone());
    crate::dispatch::handle_async_event(
        &mut app,
        crate::dispatch::AsyncEvent::Agent(
            AgentEvent::QuestionRequested(sample_question_request()),
        ),
    );
    let session_id = app.session_id.as_str().to_string();

    handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    wait_for_question_resolution(&engine).await;

    assert_eq!(app.mode, Mode::Normal);
    assert!(app.pending_questions.is_empty());
    let calls = engine.calls();
    assert!(
        calls.iter().any(|call| matches!(
            call,
            EngineCall::ResolveQuestion {
                session_id: resolved_session_id,
                response: navi_sdk::QuestionResponse::Answered { id, answers },
            } if resolved_session_id == &session_id && id == "question-1" && answers == &vec!["Thorough".to_string()]
        )),
        "calls: {calls:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn question_number_key_selects_option() {
    use crate::testing::{EngineCall, MockEngine};

    let mut app = test_app("");
    let engine = Arc::new(MockEngine::new());
    app.set_engine(engine.clone());
    crate::dispatch::handle_async_event(
        &mut app,
        crate::dispatch::AsyncEvent::Agent(
            AgentEvent::QuestionRequested(sample_question_request()),
        ),
    );
    let session_id = app.session_id.as_str().to_string();

    handle_key(&mut app, KeyCode::Char('2'), KeyModifiers::NONE);
    handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    wait_for_question_resolution(&engine).await;

    let calls = engine.calls();
    assert!(
        calls.iter().any(|call| matches!(
            call,
            EngineCall::ResolveQuestion {
                session_id: resolved_session_id,
                response: navi_sdk::QuestionResponse::Answered { id, answers },
            } if resolved_session_id == &session_id && id == "question-1" && answers == &vec!["Thorough".to_string()]
        )),
        "calls: {calls:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn question_can_be_denied_explicitly() {
    use crate::testing::{EngineCall, MockEngine};

    let mut app = test_app("");
    let engine = Arc::new(MockEngine::new());
    app.set_engine(engine.clone());
    crate::dispatch::handle_async_event(
        &mut app,
        crate::dispatch::AsyncEvent::Agent(
            AgentEvent::QuestionRequested(sample_question_request()),
        ),
    );
    let session_id = app.session_id.as_str().to_string();

    handle_key(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
    wait_for_question_resolution(&engine).await;

    assert_eq!(app.mode, Mode::Normal);
    assert!(app.pending_questions.is_empty());
    let calls = engine.calls();
    assert!(
        calls.iter().any(|call| matches!(
            call,
            EngineCall::ResolveQuestion {
                session_id: resolved_session_id,
                response: navi_sdk::QuestionResponse::Dismissed { id },
            } if resolved_session_id == &session_id && id == "question-1"
        )),
        "calls: {calls:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn question_accepts_plain_text_answer() {
    use crate::testing::{EngineCall, MockEngine};

    let mut app = test_app("");
    let engine = Arc::new(MockEngine::new());
    app.set_engine(engine.clone());
    crate::dispatch::handle_async_event(
        &mut app,
        crate::dispatch::AsyncEvent::Agent(
            AgentEvent::QuestionRequested(sample_question_request()),
        ),
    );
    let session_id = app.session_id.as_str().to_string();

    handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    for ch in "plain answer".chars() {
        handle_key(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
    }
    handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    wait_for_question_resolution(&engine).await;

    let calls = engine.calls();
    assert!(
        calls.iter().any(|call| matches!(
            call,
            EngineCall::ResolveQuestion {
                session_id: resolved_session_id,
                response: navi_sdk::QuestionResponse::Answered { id, answers },
            } if resolved_session_id == &session_id && id == "question-1" && answers == &vec!["plain answer".to_string()]
        )),
        "calls: {calls:?}"
    );
}

#[test]
fn command_palette_scroll_offset_keeps_selection_visible() {
    assert_eq!(command_scroll_offset(0, 6), 0);
    assert_eq!(command_scroll_offset(5, 6), 0);
    assert_eq!(command_scroll_offset(6, 6), 1);
    assert_eq!(command_scroll_offset(11, 6), 6);

    let mut app = test_app("");
    app.mode = Mode::Commands;
    app.selected_command = 0;
    for _ in 0..20 {
        handle_command_key(&mut app, KeyCode::Down);
    }
    assert_eq!(app.selected_command, filtered_commands(&app).len() - 1);

    handle_command_key(&mut app, KeyCode::PageUp);
    assert_eq!(
        app.selected_command,
        filtered_commands(&app).len().saturating_sub(9)
    );
}

#[test]
fn submit_sends_raw_input_text() {
    let mut app = test_app("");
    app.provider_configured = false;
    app.input = "check this change".to_string();
    app.input_cursor = app.input.len();

    submit_message(&mut app);

    assert_eq!(app.messages[0].content, "check this change");
    assert!(matches!(
        app.conversation_history.last(),
        Some(ModelMessage { content, .. }) if content == "check this change"
    ));

    app.input = "/literal inspect first".to_string();
    app.input_cursor = app.input.len();
    submit_message(&mut app);

    assert!(
        app.messages
            .iter()
            .any(|message| message.content == "/literal inspect first")
    );
}

#[test]
fn yolo_toggle_uses_notification_not_chat() {
    let mut app = test_app("");
    let message_count = app.messages.len();

    handle_key(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL);

    assert!(app.yolo_mode);
    assert_eq!(app.messages.len(), message_count);
    let notification = app.notification().expect("notification");
    assert_eq!(notification.title, "Tools");
    assert!(notification.message.contains("YOLO mode enabled"));
}

#[test]
fn notification_expires_after_ttl() {
    let mut app = test_app("");
    app.set_notification(Notification {
        title: "Tools".to_string(),
        message: "YOLO mode enabled.".to_string(),
        created_at: Instant::now() - NOTIFICATION_TTL - Duration::from_millis(1),
        ttl: NOTIFICATION_TTL,
    });

    assert!(expire_notification(&mut app));

    assert!(app.notification().is_none());
}

#[test]
fn settings_toggles_thinking_visibility() {
    let mut app = test_app("");
    app.mode = Mode::Settings;
    app.selected_setting = 0;
    assert!(app.show_thinking);

    handle_settings_key(&mut app, KeyCode::Enter);
    assert!(!app.show_thinking);
    assert!(!app.loaded_config.config.tui.show_thinking);
    assert!(app.notification().is_some());
}

#[test]
fn tui_preferences_load_from_config() {
    let mut config = navi_sdk::NaviConfig::default();
    config.tui.theme = "ember".to_string();
    config.tui.show_thinking = false;
    config.tui.full_tool_view = true;
    config.tui.compact_tool_visible_limit = 8;
    config.tui.thinking_level = "low".to_string();
    config.tui.yolo_mode = true;
    config.skills.active = vec!["demo-skill".to_string()];

    let app = TuiApp::new(
        LoadedConfig {
            config,
            global_config_path: None,
            project_config_path: None,
            data_dir: PathBuf::from("/tmp/navi-test"),
        },
        PathBuf::from("/tmp/test-project"),
        None,
    )
    .expect("test app");

    assert_eq!(app.theme_id, crate::theme::ThemeId::Ember);
    assert!(!app.show_thinking);
    assert!(app.full_tool_view);
    assert_eq!(app.compact_tool_visible_limit, 8);
    assert_eq!(app.thinking_level, crate::state::ThinkingLevel::Low);
    assert!(app.yolo_mode);
    assert_eq!(app.active_skills, vec!["demo-skill".to_string()]);
}

#[test]
fn settings_does_not_open_provider_accounts() {
    let mut app = test_app("");
    app.mode = Mode::Settings;
    app.selected_setting = 0;

    handle_settings_key(&mut app, KeyCode::Enter);
    assert_eq!(app.mode, Mode::Settings);
    assert!(!app.full_tool_view);
}

#[test]
fn settings_opens_theme_picker() {
    let mut app = test_app("");
    app.mode = Mode::Settings;
    app.selected_setting = 3;
    assert_eq!(app.theme_id, crate::theme::ThemeId::Default);

    handle_settings_key(&mut app, KeyCode::Enter);
    assert_eq!(app.mode, Mode::ThemePicker);
    assert_eq!(app.theme_id, crate::theme::ThemeId::Default);
}

#[test]
fn esc_closes_modal_without_canceling_active_model() {
    let mut app = test_app("");
    app.mode = Mode::Settings;
    app.is_loading = true;

    assert!(!handle_key(&mut app, KeyCode::Esc, KeyModifiers::empty()));

    assert_eq!(app.mode, Mode::Normal);
    assert!(app.is_loading);
}

#[test]
fn ctrl_d_opens_debug_modal() {
    let mut app = test_app("");

    assert!(!handle_key(
        &mut app,
        KeyCode::Char('d'),
        KeyModifiers::CONTROL
    ));

    assert_eq!(app.mode, Mode::Debug);
    assert!(app.log_path().ends_with("logs/navi.log"));
}

#[test]
fn write_file_tool_full_content_uses_edit_summary() {
    let invocation = ToolInvocation {
        id: "call-1".to_string(),
        tool_name: "write_file".to_string(),
        input: serde_json::json!({
            "path": "src/index.html",
            "content": "<!doctype html>\n<html></html>\n"
        }),
    };
    let result = ToolResult {
        invocation_id: "call-1".to_string(),
        ok: true,
        output: serde_json::json!({
            "path": "src/index.html",
            "bytes": 16,
            "lines_added": 2,
            "lines_removed": 1,
        }),
    };

    let content = tool_full_content(&invocation, &result);

    assert!(content.contains("Write src/index.html (+2 -1 lines)"));
    assert!(content.contains("Edited src/index.html (+2 -1 lines)"));
    assert!(!content.contains("Input"));
    assert!(!content.contains("Output"));
    assert!(!content.contains("```json"));
    assert!(!content.contains("<!doctype html>"));
}

#[test]
fn completed_tool_removes_empty_tool_placeholder() {
    let mut app = test_app("");
    app.messages.push(ChatMessage {
        model_label: Some("model".to_string()),
        provider_label: Some("provider".to_string()),
        status: Some("tool: read_file".to_string()),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });

    let invocation = ToolInvocation {
        id: "call-1".to_string(),
        tool_name: "read_file".to_string(),
        input: serde_json::json!({ "path": "Cargo.toml" }),
    };
    let result = ToolResult {
        invocation_id: "call-1".to_string(),
        ok: true,
        output: serde_json::json!({ "path": "Cargo.toml", "content": "" }),
    };

    app.tool_invocations
        .insert(invocation.id.clone(), invocation.clone());
    app.running_tools
        .insert(invocation.id.clone(), invocation.clone());

    // Process ToolCompleted event logic directly as in the main event loop
    app.running_tools.remove(&result.invocation_id);
    if let Some(invocation) = app.tool_invocations.get(&result.invocation_id).cloned() {
        remove_active_tool_placeholder(&mut app);
        app.messages.push(ChatMessage {
            status: Some("tool result".to_string()),
            tool_invocation: Some(invocation.clone()),
            tool_result: Some(result.clone()),
            ..ChatMessage::new(ChatRole::Assistant, String::new())
        });
    }

    assert_eq!(
        app.messages
            .iter()
            .filter(|message| message.status.as_deref() == Some("tool result"))
            .count(),
        1
    );
    assert!(!app.messages.iter().any(is_empty_tool_placeholder));
}

#[test]
fn compact_tool_render_hides_full_input_and_output() {
    let mut app = test_app("");
    let invocation = ToolInvocation {
        id: "call-1".to_string(),
        tool_name: "fs_browser".to_string(),
        input: serde_json::json!({ "action": "list", "path": "/tmp/project" }),
    };
    let result = ToolResult {
        invocation_id: "call-1".to_string(),
        ok: true,
        output: serde_json::json!({
            "files": ["/tmp/project/package.json", "/tmp/project/src/App.tsx"]
        }),
    };
    app.messages.push(ChatMessage {
        status: Some("tool result".to_string()),
        tool_invocation: Some(invocation),
        tool_result: Some(result),
        ..ChatMessage::new(
            ChatRole::Assistant,
            "stale full tool content should not render in compact mode\n\nInput\nOutput"
                .to_string(),
        )
    });

    let text = build_chat_lines(&mut app, 80)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("List /tmp/project"));
    assert!(!text.contains("Input"));
    assert!(!text.contains("Output"));
    assert!(!text.contains("stale full tool content"));
}

#[test]
fn compact_tool_render_keeps_consecutive_tools_progressive() {
    let mut app = test_app("");
    for index in 0..8 {
        let id = format!("call-{index}");
        let invocation = ToolInvocation {
            id: id.clone(),
            tool_name: "grep".to_string(),
            input: serde_json::json!({ "pattern": format!("needle-{index}"), "path": "src" }),
        };
        let result = ToolResult {
            invocation_id: id,
            ok: true,
            output: serde_json::json!({ "matches": [] }),
        };
        app.messages.push(ChatMessage {
            status: Some("tool result".to_string()),
            tool_invocation: Some(invocation),
            tool_result: Some(result),
            ..ChatMessage::new(ChatRole::Assistant, String::new())
        });
    }

    let text = build_chat_lines(&mut app, 80)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!text.contains("earlier tool calls"));
    assert!(text.contains("needle-0"));
    assert!(text.contains("needle-2"));
    assert!(text.contains("needle-7"));
    assert_eq!(text.matches("Search \"").count(), 8);
}

#[test]
fn compact_tool_render_keeps_older_tool_runs_progressive() {
    let mut app = test_app("");
    for index in 0..6 {
        let id = format!("old-call-{index}");
        app.messages.push(ChatMessage {
            status: Some("tool result".to_string()),
            tool_invocation: Some(ToolInvocation {
                id: id.clone(),
                tool_name: "grep".to_string(),
                input: serde_json::json!({ "pattern": format!("old-needle-{index}") }),
            }),
            tool_result: Some(ToolResult {
                invocation_id: id,
                ok: true,
                output: serde_json::json!({ "matches": [] }),
            }),
            ..ChatMessage::new(ChatRole::Assistant, String::new())
        });
    }
    app.messages.push(ChatMessage::new(
        ChatRole::Assistant,
        "intermediate thought".to_string(),
    ));
    app.messages.push(ChatMessage {
        status: Some("tool result".to_string()),
        tool_invocation: Some(ToolInvocation {
            id: "latest-call".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({ "path": "README.md" }),
        }),
        tool_result: Some(ToolResult {
            invocation_id: "latest-call".to_string(),
            ok: true,
            output: serde_json::json!({ "path": "README.md" }),
        }),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });

    let text = build_chat_lines(&mut app, 80)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!text.contains("6 tools · Search x6"));
    assert!(text.contains("old-needle-0"));
    assert!(text.contains("old-needle-5"));
    assert!(text.contains("intermediate thought"));
    assert!(text.contains("Read README.md"));
}

#[test]
fn compact_tool_render_does_not_group_across_thinking_placeholder() {
    let mut app = test_app("");
    for path in ["src/a.rs", "src/b.rs"] {
        app.messages.push(ChatMessage {
            status: Some("tool result".to_string()),
            tool_invocation: Some(ToolInvocation {
                id: path.to_string(),
                tool_name: "read_file".to_string(),
                input: serde_json::json!({ "path": path }),
            }),
            tool_result: Some(ToolResult {
                invocation_id: path.to_string(),
                ok: true,
                output: serde_json::json!({ "path": path }),
            }),
            ..ChatMessage::new(ChatRole::Assistant, String::new())
        });
    }
    app.messages.push(ChatMessage {
        status: Some("thinking".to_string()),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });
    app.messages.push(ChatMessage {
        status: Some("tool result".to_string()),
        tool_invocation: Some(ToolInvocation {
            id: "src/c.rs".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({ "path": "src/c.rs" }),
        }),
        tool_result: Some(ToolResult {
            invocation_id: "src/c.rs".to_string(),
            ok: true,
            output: serde_json::json!({ "path": "src/c.rs" }),
        }),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });

    let text = build_chat_lines(&mut app, 80)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Read src/a.rs"));
    assert!(text.contains("Read src/b.rs"));
    assert!(text.contains("Read src/c.rs"));
    assert!(!text.contains("tools · Read"));
}

#[test]
fn full_tool_render_generates_sanitized_metadata_view() {
    let mut app = test_app("");
    app.full_tool_view = true;
    let invocation = ToolInvocation {
        id: "call-1".to_string(),
        tool_name: "grep".to_string(),
        input: serde_json::json!({ "pattern": "NAVI" }),
    };
    let result = ToolResult {
        invocation_id: "call-1".to_string(),
        ok: false,
        output: serde_json::json!({ "error": "denied" }),
    };
    app.messages.push(ChatMessage {
        status: Some("tool result".to_string()),
        tool_invocation: Some(invocation),
        tool_result: Some(result),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });

    let text = build_chat_lines(&mut app, 80)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Search \"NAVI\""));
    assert!(text.contains("error: denied"));
    assert!(text.contains("denied"));
    assert!(!text.contains("Input"));
    assert!(!text.contains("Output"));
    assert!(!text.contains("```json"));
}

#[test]
fn running_subagent_renders_active_task_block() {
    let mut app = test_app("");
    record_tool_requested(
        &mut app,
        ToolInvocation {
            id: "subagent-1".to_string(),
            tool_name: "subagent".to_string(),
            input: serde_json::json!({
                "description": "Analyze repository structure",
                "prompt": "Read justfile and summarize the main crates",
                "profile": "repo_search",
            }),
        },
    );

    let text = build_chat_lines(&mut app, 80)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Subagent Task"));
    assert!(text.contains("Analyze repository structure"));
    assert!(text.contains("Read justfile and summarize the main crates"));
}

#[test]
fn subagent_activity_replaces_single_status_without_context_leak() {
    let mut app = test_app("");
    record_tool_requested(
        &mut app,
        ToolInvocation {
            id: "subagent-1".to_string(),
            tool_name: "subagent".to_string(),
            input: serde_json::json!({
                "description": "Analyze repository structure",
                "prompt": "Inspect project files",
            }),
        },
    );
    let history_len = app.conversation_history.len();
    let event_len = app.events.len();

    handle_async_event(
        &mut app,
        AsyncEvent::Agent(AgentEvent::SubagentActivity {
            invocation_id: "subagent-1".to_string(),
            message: "Read justfile".to_string(),
        }),
    );
    handle_async_event(
        &mut app,
        AsyncEvent::Agent(AgentEvent::SubagentActivity {
            invocation_id: "subagent-1".to_string(),
            message: "Search \"SubagentTool\"".to_string(),
        }),
    );

    let text = build_chat_lines(&mut app, 80)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Subagent Task"));
    assert!(text.contains("Search \"SubagentTool\""));
    assert!(!text.contains("Read justfile"));
    assert_eq!(app.conversation_history.len(), history_len);
    assert_eq!(app.events.len(), event_len);
}

#[test]
fn subagent_view_renders_transcript_and_footer() {
    let mut app = test_app("");
    handle_async_event(
        &mut app,
        AsyncEvent::Agent(AgentEvent::ToolRequested(ToolInvocation {
            id: "subagent-1".to_string(),
            tool_name: "subagent".to_string(),
            input: serde_json::json!({
                "description": "Analyze repository structure",
                "prompt": "Inspect project files",
            }),
        })),
    );
    handle_async_event(
        &mut app,
        AsyncEvent::Agent(AgentEvent::SubagentTranscript {
            invocation_id: "subagent-1".to_string(),
            item: SubagentTranscriptItem {
                kind: SubagentTranscriptKind::ToolRequested,
                title: "Read justfile".to_string(),
                detail: None,
                ok: None,
            },
        }),
    );
    handle_async_event(
        &mut app,
        AsyncEvent::Agent(AgentEvent::SubagentTranscript {
            invocation_id: "subagent-1".to_string(),
            item: SubagentTranscriptItem {
                kind: SubagentTranscriptKind::Text,
                title: "Final response".to_string(),
                detail: Some("Repository summary ready".to_string()),
                ok: Some(true),
            },
        }),
    );
    app.open_subagent_view("subagent-1");

    let backend = TestBackend::new(96, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| crate::view::render(frame, &mut app))
        .expect("draw");
    let text = terminal_buffer_text(&terminal);

    assert!(text.contains("Analyze repository structure"));
    assert!(text.contains("Read justfile"));
    assert!(text.contains("Final response"));
    assert!(text.contains("Parent up"));
    assert!(text.contains("Prev left"));
    assert!(text.contains("Next right"));
}

#[test]
fn subagent_view_keyboard_navigation_and_context_isolation() {
    let mut app = test_app("");
    for id in ["subagent-1", "subagent-2"] {
        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::ToolRequested(ToolInvocation {
                id: id.to_string(),
                tool_name: "subagent".to_string(),
                input: serde_json::json!({
                    "description": format!("Task {id}"),
                    "prompt": "Inspect project files",
                }),
            })),
        );
    }
    let history_len = app.conversation_history.len();
    let event_len = app.events.len();

    app.open_subagent_view("subagent-1");
    assert!(matches!(
        app.chat_view,
        crate::state::ChatView::Subagent { ref invocation_id } if invocation_id == "subagent-1"
    ));

    handle_normal_key(&mut app, KeyCode::Right, KeyModifiers::NONE);
    assert!(matches!(
        app.chat_view,
        crate::state::ChatView::Subagent { ref invocation_id } if invocation_id == "subagent-2"
    ));

    handle_normal_key(&mut app, KeyCode::Left, KeyModifiers::NONE);
    assert!(matches!(
        app.chat_view,
        crate::state::ChatView::Subagent { ref invocation_id } if invocation_id == "subagent-1"
    ));

    handle_normal_key(&mut app, KeyCode::Up, KeyModifiers::NONE);
    assert!(matches!(app.chat_view, crate::state::ChatView::Parent));
    assert_eq!(app.conversation_history.len(), history_len);
    assert_eq!(app.events.len(), event_len);
}

#[test]
fn compact_tool_render_expands_only_clicked_tool_result() {
    let mut app = test_app("");
    let invocation = ToolInvocation {
        id: "call-1".to_string(),
        tool_name: "read_file".to_string(),
        input: serde_json::json!({ "path": "README.md" }),
    };
    let result = ToolResult {
        invocation_id: "call-1".to_string(),
        ok: true,
        output: serde_json::json!({
            "path": "README.md",
            "content": "expanded-only-content"
        }),
    };
    app.messages.push(ChatMessage {
        status: Some("tool result".to_string()),
        tool_invocation: Some(invocation),
        tool_result: Some(result),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });

    let collapsed = build_chat_lines(&mut app, 80)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!collapsed.contains("expanded-only-content"));

    app.expanded_tool_results.insert("call-1".to_string());
    let expanded = build_chat_lines(&mut app, 80)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(expanded.contains("Click to collapse"));
    assert!(expanded.contains("expanded-only-content"));
    assert!(!app.full_tool_view);
}

#[test]
fn compact_tool_render_hides_bash_output_until_expanded() {
    let mut app = test_app("");
    let invocation = ToolInvocation {
        id: "call-1".to_string(),
        tool_name: "bash".to_string(),
        input: serde_json::json!({ "command": "just test-crate navi-tui" }),
    };
    let result = ToolResult {
        invocation_id: "call-1".to_string(),
        ok: true,
        output: serde_json::json!({
            "stdout": "hidden stdout",
            "stderr": "hidden stderr",
            "exit_code": 0
        }),
    };
    app.messages.push(ChatMessage {
        status: Some("tool result".to_string()),
        tool_invocation: Some(invocation),
        tool_result: Some(result),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });

    let collapsed = build_chat_lines(&mut app, 80)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(collapsed.contains("Run just test-crate navi-tui"));
    assert!(!collapsed.contains("hidden stdout"));
    assert!(!collapsed.contains("hidden stderr"));

    app.expanded_tool_results.insert("call-1".to_string());
    let expanded = build_chat_lines(&mut app, 80)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(expanded.contains("hidden stdout"));
    assert!(expanded.contains("hidden stderr"));
}

#[test]
fn apply_patch_tool_full_content_uses_edit_summary() {
    let invocation = ToolInvocation {
        id: "call-1".to_string(),
        tool_name: "apply_patch".to_string(),
        input: serde_json::json!({
            "patch": "*** Begin Patch\n*** Update File: crates/navi-tui/src/lib.rs\n@@\n-    old\n+    new\n+    added\n*** End Patch\n"
        }),
    };
    let result = ToolResult {
        invocation_id: "call-1".to_string(),
        ok: true,
        output: serde_json::json!({ "status": 0 }),
    };

    let content = tool_full_content(&invocation, &result);

    assert!(content.contains("Edited crates/navi-tui/src/lib.rs (+2 -1)"));
    assert!(!content.contains("*** Begin Patch"));
    assert!(!content.contains("Input"));
    assert!(!content.contains("Output"));
}

#[tokio::test(flavor = "multi_thread")]
async fn command_palette_sync_models_starts_sync() {
    let mut app = test_app("");
    app.command_filter = "sync".to_string();
    app.selected_command = 0;

    let commands = filtered_commands(&app);
    assert!(
        commands
            .iter()
            .any(|c| matches!(c.action, CommandAction::SyncModels))
    );

    sync_models_tui(&mut app);

    assert!(app.is_loading);
    assert!(app.loading_start.is_some());
    assert_eq!(app.messages.len(), 1);
    assert_eq!(
        app.messages[0].content,
        "Syncing registry and models from providers..."
    );
    assert_eq!(app.messages[0].status, Some("syncing".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn model_picker_tab_triggers_per_provider_sync() {
    let mut app = test_app("");
    app.mode = Mode::Models;

    let provider_id = app.models[app.selected_model].provider_id.clone();

    // Press Tab to trigger per-provider sync
    handle_model_key(&mut app, KeyCode::Tab, KeyModifiers::NONE);

    assert!(app.is_loading);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.messages.len(), 1);
    assert!(
        app.messages[0].content.contains(&provider_id),
        "Tab sync message should mention the provider: got '{}'",
        app.messages[0].content
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn model_picker_ctrl_r_triggers_all_provider_sync() {
    let mut app = test_app("");
    app.mode = Mode::Models;

    // Press Ctrl+r to trigger all-provider sync
    handle_model_key(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);

    assert!(app.is_loading);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.messages.len(), 1);
    assert_eq!(
        app.messages[0].content,
        "Syncing registry and models from providers..."
    );
}

#[test]
fn model_picker_ctrl_e_opens_provider_setup() {
    let mut app = test_app("");
    app.mode = Mode::Models;
    // Set a dummy API key so the default provider's models are visible
    let provider_id = app.models[app.selected_model].provider_id.clone();
    let _ = app
        .credential_store()
        .set_api_key(&provider_id, "dummy-key");
    app.refresh_authenticated_providers();

    let selected = app.selected_model;

    handle_model_key(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);

    assert_eq!(app.mode, Mode::ApiKeyEntry);
    assert_eq!(app.pending_model_selection, Some(selected));
    assert!(app.api_key_input.is_empty());
    assert_eq!(app.api_key_cursor, 0);
}

#[test]
fn model_error_is_rendered_as_separate_message() {
    let mut app = test_app("");
    app.messages.push(ChatMessage {
        status: Some("tool result".to_string()),
        ..ChatMessage::new(
            ChatRole::Assistant,
            "✓ write_file called · success".to_string(),
        )
    });
    app.messages.push(ChatMessage {
        status: Some("thinking".to_string()),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });
    app.is_loading = true;
    app.skip_next_model_done = true;

    handle_model_error(
        &mut app,
        "provider request failed with 400 Bad Request".to_string(),
    );

    assert_eq!(app.messages[0].status.as_deref(), Some("tool result"));
    assert_eq!(app.messages[2].status.as_deref(), Some("error"));
    assert!(app.messages[2].content.contains("400"));
    assert!(!app.is_loading);
    assert!(!app.skip_next_model_done);
}

#[tokio::test(flavor = "multi_thread")]
async fn transient_model_error_retries_without_final_error() {
    let mut app = test_app("");
    app.provider_configured = false;
    app.messages.push(ChatMessage {
        status: Some("thinking".to_string()),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });
    app.is_loading = true;

    handle_model_error(
        &mut app,
        "failed to read chat completions stream: unexpected EOF during chunk size line".to_string(),
    );

    assert_eq!(app.model_retry_attempts, 1);
    assert!(app.is_loading);
    assert!(app.has_stream_task());
    assert!(
        app.messages
            .iter()
            .any(|message| message.status.as_deref() == Some("retrying"))
    );
    assert!(
        app.messages
            .iter()
            .all(|message| message.status.as_deref() != Some("thinking"))
    );
}

#[test]
fn free_usage_limit_error_does_not_schedule_retry() {
    let mut app = test_app("");
    app.messages.push(ChatMessage {
        status: Some("thinking".to_string()),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });
    app.is_loading = true;

    handle_model_error(
            &mut app,
            "API error 429 Too Many Requests: {\"type\":\"error\",\"error\":{\"type\":\"FreeUsageLimitError\",\"message\":\"Rate limit exceeded.\"}} (requested delay: Some(64649s))".to_string(),
        );

    assert_eq!(app.model_retry_attempts, 0);
    assert!(!app.is_loading);
    assert!(!app.has_stream_task());
    assert!(
        app.messages
            .last()
            .unwrap()
            .content
            .contains("Usage limit reached for")
    );
    assert!(
        app.messages
            .last()
            .unwrap()
            .content
            .contains("select a non-free model")
    );
}

#[test]
fn opencode_zen_model_names_are_canonicalized_for_api_requests() {
    assert_eq!(
        navi_sdk::provider_request_model_name("opencode", "DeepSeek V4 Flash Free"),
        "deepseek-v4-flash-free"
    );
    assert_eq!(
        navi_sdk::provider_request_model_name("opencode-zen", "opencode/Nemotron 3 Super Free"),
        "nemotron-3-super-free"
    );
    assert_eq!(
        navi_sdk::provider_request_model_name("openrouter", "DeepSeek V4 Flash Free"),
        "DeepSeek V4 Flash Free"
    );
}

#[test]
fn opencode_free_models_can_use_public_access_without_key() {
    let app = test_app("");
    let model = ModelOption {
        name: "deepseek-v4-flash-free".to_string(),
        provider_id: "opencode".to_string(),
        provider_label: "OpenCode Zen".to_string(),
        provider_description: "Recommended".to_string(),
        task_size: navi_sdk::ModelTaskSize::Small,
        context_window_tokens: None,
    };

    assert!(model_is_available_for_selection(&app, &model));
    assert_eq!(
        navi_sdk::provider_request_model_name("opencode", "deepseek-v4-flash-free"),
        "deepseek-v4-flash-free"
    );
}

#[test]
fn escape_double_press_cancels_active_tool_task_state() {
    let mut app = test_app("");
    app.is_loading = true;
    app.skip_next_model_done = true;
    app.messages.push(ChatMessage {
        status: Some("tool: bash".to_string()),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    });

    // First Esc shows a confirmation notification but keeps the operation running.
    let should_quit = handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(!should_quit);
    assert!(app.is_loading);
    assert!(app.cancel_esc_pressed);

    // Second Esc actually cancels the operation.
    let should_quit = handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(!should_quit);
    assert!(!app.is_loading);
    assert!(!app.skip_next_model_done);
    assert_eq!(
        active_assistant_message(&mut app).and_then(|message| message.status.clone()),
        Some("cancelled".to_string())
    );
}

// ─── recent provider / model lists ────────────────────────────────────────

#[test]
fn push_recent_provider_dedupes_and_caps() {
    let mut app = test_app("");
    app.loaded_config.config.tui.recent_provider_ids.clear();

    for _ in 0..20 {
        crate::providers::push_recent_provider(&mut app, "openai");
    }
    assert_eq!(
        app.loaded_config.config.tui.recent_provider_ids,
        vec!["openai".to_string()]
    );

    crate::providers::push_recent_provider(&mut app, "anthropic");
    crate::providers::push_recent_provider(&mut app, "google-gemini");
    crate::providers::push_recent_provider(&mut app, "openai");
    assert_eq!(
        app.loaded_config.config.tui.recent_provider_ids,
        vec![
            "openai".to_string(),
            "google-gemini".to_string(),
            "anthropic".to_string(),
        ]
    );

    // Fill past the cap and confirm the oldest entries fall off.
    for id in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"] {
        crate::providers::push_recent_provider(&mut app, id);
    }
    assert_eq!(
        app.loaded_config.config.tui.recent_provider_ids.len(),
        crate::providers::RECENTS_LIMIT
    );
    assert_eq!(
        app.loaded_config
            .config
            .tui
            .recent_provider_ids
            .first()
            .map(String::as_str),
        Some("j"),
    );
}

#[test]
fn push_recent_model_uses_provider_model_key() {
    let mut app = test_app("");
    app.loaded_config.config.tui.recent_model_ids.clear();

    crate::providers::push_recent_model(&mut app, "openai", "gpt-5.5");
    crate::providers::push_recent_model(&mut app, "anthropic", "claude-sonnet-4-20250514");
    crate::providers::push_recent_model(&mut app, "openai", "gpt-5.5");

    assert_eq!(
        app.loaded_config.config.tui.recent_model_ids,
        vec![
            "openai:gpt-5.5".to_string(),
            "anthropic:claude-sonnet-4-20250514".to_string(),
        ]
    );
}

#[test]
fn push_recent_provider_canonicalizes_aliases() {
    let mut app = test_app("");
    app.loaded_config.config.tui.recent_provider_ids.clear();

    crate::providers::push_recent_provider(&mut app, "opencode-zen");
    crate::providers::push_recent_provider(&mut app, "opencode");

    // Both should collapse to the canonical `opencode`.
    assert_eq!(
        app.loaded_config.config.tui.recent_provider_ids,
        vec!["opencode".to_string()],
    );
}

fn provider_rows_labels(app: &TuiApp) -> Vec<String> {
    app.filtered_providers()
        .into_iter()
        .map(|row| match row {
            crate::providers::ProviderListRow::Header { label } => {
                format!("section: {label}")
            }
            crate::providers::ProviderListRow::Provider { index } => {
                let catalog = navi_sdk::provider_catalog(&app.loaded_config.config);
                catalog
                    .get(index)
                    .map(|p| p.id.clone())
                    .unwrap_or_else(|| format!("#{index}"))
            }
            crate::providers::ProviderListRow::Account {
                label, selected, ..
            } => {
                let indicator = if selected { "●" } else { "○" };
                format!("  {indicator} {label}")
            }
        })
        .collect()
}

#[test]
fn filtered_providers_orders_recent_connected_other() {
    let mut app = test_app("");
    // Mark two arbitrary providers as recent.
    app.loaded_config.config.tui.recent_provider_ids.clear();
    app.loaded_config.config.tui.recent_provider_ids =
        vec!["anthropic".to_string(), "openai".to_string()];
    // Mark a different provider as connected.
    app.authenticated_providers.clear();
    app.authenticated_providers
        .insert("google-gemini".to_string());

    let rows = provider_rows_labels(&app);

    // Recent header first, then the two recents (in recents order).
    assert!(rows[0].starts_with("section:"));
    assert!(rows[0].contains("Recent"));
    assert!(rows[1].ends_with("anthropic"));
    assert!(rows[2].ends_with("openai"));
    // Connected section next.
    let connected_idx = rows
        .iter()
        .position(|r| r.contains("Connected"))
        .expect("connected header present");
    assert!(rows[connected_idx + 1].ends_with("google-gemini"));
    // The trailing "Other" section appears last.
    assert!(rows.iter().any(|r| r.contains("Other providers")));
}

#[test]
fn filtered_providers_skips_sections_when_filtering() {
    let mut app = test_app("");
    app.loaded_config.config.tui.recent_provider_ids =
        vec!["anthropic".to_string(), "openai".to_string()];
    app.authenticated_providers.clear();
    app.authenticated_providers
        .insert("google-gemini".to_string());
    app.provider_filter = "ant".to_string();

    let rows = provider_rows_labels(&app);

    // No section headers when filtering, and only matches show up.
    assert!(rows.iter().all(|r| !r.starts_with("section:")));
    assert!(rows.iter().any(|r| r.contains("anthropic")));
}

#[test]
fn provider_modal_arrow_keys_skip_section_headers() {
    let mut app = test_app("");
    app.loaded_config.config.tui.recent_provider_ids =
        vec!["anthropic".to_string(), "openai".to_string()];
    app.authenticated_providers.clear();
    open_modal(&mut app, ModalKind::Providers);

    let rows = app.filtered_providers();
    let recent_openai = rows
        .iter()
        .position(|row| {
            matches!(
                row,
                crate::providers::ProviderListRow::Provider { index }
                    if navi_sdk::provider_catalog(&app.loaded_config.config)
                        .get(*index)
                        .is_some_and(|provider| provider.id == "openai")
            )
        })
        .expect("recent openai row");
    let next_header = rows
        .iter()
        .enumerate()
        .skip(recent_openai + 1)
        .find_map(|(index, row)| {
            matches!(row, crate::providers::ProviderListRow::Header { .. }).then_some(index)
        })
        .expect("section header after recent providers");
    let last_selectable_before_header = next_header - 1;
    let next_provider_after_header = rows
        .iter()
        .enumerate()
        .skip(next_header + 1)
        .find_map(|(index, row)| {
            matches!(row, crate::providers::ProviderListRow::Provider { .. }).then_some(index)
        })
        .expect("provider after next section header");

    app.selected_provider_setting = last_selectable_before_header;
    handle_key(&mut app, KeyCode::Down, KeyModifiers::empty());
    assert_eq!(app.selected_provider_setting, next_provider_after_header);

    handle_key(&mut app, KeyCode::Up, KeyModifiers::empty());
    assert_eq!(app.selected_provider_setting, last_selectable_before_header);
}

#[test]
fn open_model_picker_keeps_current_model_selected() {
    let mut app = test_app("");
    if app.models.len() >= 2 {
        app.selected_model = 1;
        let current_index = app.selected_model;
        let current_name = app.models[current_index].name.clone();

        open_model_picker(&mut app);

        assert_eq!(app.selected_model, current_index);
        // The model is still the one we picked.
        assert_eq!(app.models[app.selected_model].name, current_name);
    }
}

#[test]
fn build_model_rows_prepends_recent_section() {
    let mut app = test_app("");
    let Some((recent_index, recent_model)) = app
        .models
        .iter()
        .enumerate()
        .find(|(_, model)| model_is_available_for_selection(&app, model))
    else {
        return;
    };
    // Force the first model to be a "recent" so it appears at the top.
    app.loaded_config.config.tui.recent_model_ids.clear();
    app.loaded_config.config.tui.recent_model_ids = vec![format!(
        "{}:{}",
        recent_model.provider_id, recent_model.name
    )];

    let rows = build_model_rows(&app);

    // The first non-header row should be the recent model.
    let first_model = rows
        .iter()
        .find_map(|r| match r {
            ListRow::Model { index } => Some(*index),
            ListRow::Header { .. } => None,
        })
        .expect("at least one model row");
    assert_eq!(first_model, recent_index);
    // And there must be a recent header above it.
    let has_recent_header = rows.iter().any(|r| match r {
        ListRow::Header { label, .. } => label.contains("Recent"),
        _ => false,
    });
    assert!(
        has_recent_header,
        "expected a Recent header in the rendered model rows"
    );
}
