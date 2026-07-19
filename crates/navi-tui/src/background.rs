use std::time::Duration;

use navi_sdk::BackgroundCommandSnapshot;

use crate::app::TuiApp;
use crate::dispatch::AsyncEvent;
use crate::state::ModalKind;

const BG_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Starts a background poller task that periodically checks running background
/// commands and sends updates via `AsyncEvent::BackgroundCommandsUpdated`.
///
/// If a poller is already running, this is a no-op.
pub(crate) fn start_background_poller(app: &mut TuiApp) {
    if let Some(task) = &app.bg_poll_task {
        if !task.is_finished() {
            return;
        }
        app.bg_poll_task = None;
    }

    let tx = app.async_sender();
    let engine = app.engine();
    let session_id = app.session_id.as_str().to_string();

    let Some(handle) = spawn_runtime_task_with_handle(async move {
        loop {
            tokio::time::sleep(BG_POLL_INTERVAL).await;

            match engine.list_background_commands(&session_id).await {
                Ok(commands) => {
                    let has_running = commands.iter().any(|c| c.is_running());
                    let _ = tx.send(AsyncEvent::BackgroundCommandsUpdated(commands));
                    if !has_running {
                        break;
                    }
                }
                Err(_) => {
                    // Session may have been closed; stop polling
                    break;
                }
            }
        }
    }) else {
        return;
    };

    app.bg_poll_task = Some(handle);
}

/// Refreshes the TUI's background command list through the SDK.
pub(crate) fn refresh_background_commands(app: &mut TuiApp) {
    let engine = app.engine();
    let session_id = app.session_id.as_str().to_string();
    let tx = app.async_sender();
    crate::runtime::spawn_runtime_task(async move {
        if let Ok(commands) = engine.list_background_commands(&session_id).await {
            let _ = tx.send(AsyncEvent::BackgroundCommandsUpdated(commands));
        }
    });
}

/// Upserts a single background command snapshot while keeping deterministic
/// ordering and the current selection stable when possible.
pub(crate) fn upsert_background_command(app: &mut TuiApp, command: BackgroundCommandSnapshot) {
    let selected_task_id = app
        .background_commands
        .get(app.bg_command_selected)
        .map(|cmd| cmd.task_id.clone());

    if let Some(existing) = app
        .background_commands
        .iter_mut()
        .find(|cmd| cmd.task_id == command.task_id)
    {
        *existing = command;
    } else {
        app.background_commands.push(command);
    }

    app.background_commands
        .sort_by(|left, right| left.task_id.cmp(&right.task_id));

    if let Some(task_id) = selected_task_id {
        if let Some(index) = app
            .background_commands
            .iter()
            .position(|cmd| cmd.task_id == task_id)
        {
            app.bg_command_selected = index;
        }
    }
    clamp_background_selection(app);
}

pub(crate) fn replace_background_commands(
    app: &mut TuiApp,
    commands: Vec<BackgroundCommandSnapshot>,
) {
    let selected_task_id = app
        .background_commands
        .get(app.bg_command_selected)
        .map(|cmd| cmd.task_id.clone());
    app.background_commands = commands;
    app.background_commands
        .sort_by(|left, right| left.task_id.cmp(&right.task_id));
    if let Some(task_id) = selected_task_id {
        if let Some(index) = app
            .background_commands
            .iter()
            .position(|cmd| cmd.task_id == task_id)
        {
            app.bg_command_selected = index;
        }
    }
    clamp_background_selection(app);
    if app.background_commands.iter().any(|cmd| cmd.is_running()) {
        start_background_poller(app);
    }
}

pub(crate) fn open_background_command_output(app: &mut TuiApp, index: usize) {
    if index >= app.background_commands.len() {
        return;
    }
    app.bg_command_selected = index;
    app.bg_command_output_scroll = 0;
    app.bg_command_output_follow = true;
    crate::keybindings::replace_modal(app, ModalKind::BackgroundCommandOutput);
    if app
        .background_commands
        .get(index)
        .is_some_and(|cmd| cmd.is_running())
    {
        start_background_poller(app);
    }
}

/// Cancel a running background command by list index and refresh the list.
pub(crate) fn cancel_background_command_at(app: &mut TuiApp, index: usize) {
    let Some(cmd) = app.background_commands.get(index) else {
        return;
    };
    if !cmd.is_running() {
        return;
    }
    app.bg_command_selected = index;
    let task_id = cmd.task_id.clone();
    let engine = app.engine();
    let session_id = app.session_id.as_str().to_string();
    let tx = app.async_sender();
    crate::runtime::spawn_runtime_task(async move {
        let _ = engine
            .cancel_background_command(&session_id, &task_id)
            .await;
        if let Ok(commands) = engine.list_background_commands(&session_id).await {
            let _ = tx.send(AsyncEvent::BackgroundCommandsUpdated(commands));
        }
    });
}

pub(crate) fn clamp_background_selection(app: &mut TuiApp) {
    if app.background_commands.is_empty() {
        app.bg_command_selected = 0;
        app.bg_command_scroll = 0;
        return;
    }
    app.bg_command_selected = app
        .bg_command_selected
        .min(app.background_commands.len().saturating_sub(1));
    // Cards are multi-line; keep the selection inside the currently visible window.
    // `bg_command_visible_cards` is measured during render — fall back to 1 so a
    // short terminal never leaves the selected card off-screen after ↓/↑.
    let visible_cards = app.bg_command_visible_cards.max(1);
    if app.bg_command_selected < app.bg_command_scroll {
        app.bg_command_scroll = app.bg_command_selected;
    } else if app.bg_command_selected >= app.bg_command_scroll + visible_cards {
        app.bg_command_scroll = app.bg_command_selected.saturating_sub(visible_cards - 1);
    }
    app.bg_command_scroll = app
        .bg_command_scroll
        .min(app.background_commands.len().saturating_sub(visible_cards));
}

/// Stops the background poller task if running.
///
/// Kept as a shared teardown helper for session close / future UI controls;
/// callers may also abort `bg_poll_task` inline (e.g. new session).
#[allow(dead_code)] // public helper reserved for explicit poller teardown
pub(crate) fn stop_background_poller(app: &mut TuiApp) {
    if let Some(task) = app.bg_poll_task.take() {
        task.abort();
    }
}

/// Returns the elapsed time string for a background command.
pub(crate) fn format_bg_elapsed(snapshot: &BackgroundCommandSnapshot) -> String {
    format_duration_ms(snapshot.elapsed_ms)
}

/// Returns a human status label for a background command.
pub(crate) fn bg_status_label(snapshot: &BackgroundCommandSnapshot) -> &'static str {
    match snapshot.status {
        navi_sdk::BackgroundTaskStatus::Running => "Running",
        navi_sdk::BackgroundTaskStatus::Completed => "Done",
        navi_sdk::BackgroundTaskStatus::Failed => "Failed",
        navi_sdk::BackgroundTaskStatus::TimedOut => "Timed out",
        navi_sdk::BackgroundTaskStatus::Cancelled => "Cancelled",
    }
}

pub(crate) fn format_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{}s", ms / 1000)
    } else if ms < 3_600_000 {
        let mins = ms / 60_000;
        let secs = (ms % 60_000) / 1000;
        format!("{mins}m{secs}s")
    } else {
        let hours = ms / 3_600_000;
        let mins = (ms % 3_600_000) / 60_000;
        format!("{hours}h{mins}m")
    }
}

/// Like `spawn_runtime_task` but returns the JoinHandle.
fn spawn_runtime_task_with_handle<F>(future: F) -> Option<tokio::task::JoinHandle<()>>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        Some(handle.spawn(future))
    } else {
        None
    }
}
