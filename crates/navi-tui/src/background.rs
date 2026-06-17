use std::time::Duration;

use navi_sdk::BackgroundCommandSnapshot;

use crate::app::TuiApp;
use crate::dispatch::AsyncEvent;

const BG_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Starts a background poller task that periodically checks running background
/// commands and sends updates via `AsyncEvent::BackgroundCommandsUpdated`.
///
/// If a poller is already running, this is a no-op.
pub(crate) fn start_background_poller(app: &mut TuiApp) {
    if app.bg_poll_task.is_some() {
        return;
    }

    let tx = app.async_sender();
    let engine = app.engine();
    let session_id = app.session_id.as_str().to_string();

    let handle = spawn_runtime_task_with_handle(async move {
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
    });

    app.bg_poll_task = Some(handle);
}

/// Stops the background poller task if running.
#[allow(dead_code)]
pub(crate) fn stop_background_poller(app: &mut TuiApp) {
    if let Some(task) = app.bg_poll_task.take() {
        task.abort();
    }
}

/// Returns the elapsed time string for a background command.
pub(crate) fn format_bg_elapsed(snapshot: &BackgroundCommandSnapshot) -> String {
    format_duration_ms(snapshot.elapsed_ms)
}

/// Returns a status label for a background command.
pub(crate) fn bg_status_label(snapshot: &BackgroundCommandSnapshot) -> &'static str {
    match snapshot.status {
        navi_sdk::BackgroundTaskStatus::Running => "running",
        navi_sdk::BackgroundTaskStatus::Completed => "completed",
        navi_sdk::BackgroundTaskStatus::Failed => "failed",
        navi_sdk::BackgroundTaskStatus::TimedOut => "timed_out",
        navi_sdk::BackgroundTaskStatus::Cancelled => "cancelled",
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
fn spawn_runtime_task_with_handle<F>(future: F) -> tokio::task::JoinHandle<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(future)
    } else {
        // Fallback: should not happen in TUI context
        tokio::task::spawn(future)
    }
}
