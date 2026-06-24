use std::time::Duration;

use navi_sdk::AgentEvent;

use crate::app::TuiApp;
use crate::chat::{remove_active_empty_generation_placeholder, remove_active_tool_placeholder};
use crate::dispatch::AsyncEvent;
use crate::notifications::push_diagnostic;
use crate::providers::selected_provider_label;
use crate::state::{ChatMessage, ChatRole};

pub(crate) fn handle_model_error(app: &mut TuiApp, message: String) {
    if should_retry_model_error(&message)
        && !is_usage_limit_error(&message)
        && app.model_retry_attempts < max_model_retries(app)
    {
        let next_attempt = app.model_retry_attempts + 1;
        let retry_delay = model_retry_delay(&message, next_attempt);
        tracing::warn!(
            error = %message,
            attempt = next_attempt,
            max = max_model_retries(app),
            retry_delay_ms = retry_delay.as_millis() as u64,
            "transient model error retrying"
        );
        push_diagnostic(app, format!("Retrying transient provider error: {message}"));
        app.model_retry_attempts = next_attempt;
        app.skip_next_model_done = false;
        app.is_loading = true;
        app.loading_start = None;
        remove_active_tool_placeholder(app);
        remove_active_empty_generation_placeholder(app);
        app.messages.push(ChatMessage {
            status: Some("retrying".to_string()),
            ..ChatMessage::new(
                ChatRole::Assistant,
                format!(
                    "Transient provider error: {message}\nRetrying agent step {}/{} in {}.",
                    app.model_retry_attempts,
                    max_model_retries(app),
                    human_duration(retry_delay),
                ),
            )
        });
        schedule_model_retry(app, retry_delay);
        return;
    }

    let formatted_message = format_model_error_message(app, &message);
    let is_duplicate_tail_error = app.messages.last().is_some_and(|last| {
        last.status.as_deref() == Some("error") && last.content == formatted_message
    });

    if !is_duplicate_tail_error {
        tracing::error!(error = %message, "model stream failed");
        push_diagnostic(app, format!("Model error: {message}"));
        app.messages.push(ChatMessage {
            status: Some("error".to_string()),
            ..ChatMessage::new(ChatRole::Assistant, formatted_message)
        });
        app.events.push(AgentEvent::Error { message });
    }
    app.skip_next_model_done = false;
    app.is_loading = false;
    app.loading_start = None;
    app.clear_stream_task();
}

fn schedule_model_retry(app: &mut TuiApp, delay: Duration) {
    let tx = app.async_sender();
    app.set_stream_task(tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        let _ = tx.send(AsyncEvent::RetryModel);
    }));
}

fn format_model_error_message(app: &TuiApp, message: &str) -> String {
    if is_usage_limit_error(message) {
        let model = app.loaded_config.config.model.name.as_str();
        let provider = selected_provider_label(app);
        let free_hint = if navi_sdk::is_free_model_name(model) {
            "This selected model is a free-tier model. Free-tier quota can be exhausted even when the provider account still has paid/regular capacity."
        } else {
            "The selected provider reported a usage-limit error for this request."
        };
        format!(
            "⚠ Usage limit reached for {model} via {provider}.\n\n{free_hint}\n\n{message}\n\nUse ctrl+m and select a non-free model, or wait for the provider limit window to reset."
        )
    } else {
        format!("⚠ Error: {message}")
    }
}

fn max_model_retries(_app: &TuiApp) -> usize {
    5
}

pub(crate) fn human_duration(duration: Duration) -> String {
    if duration.as_secs() > 0 {
        format!("{}s", duration.as_secs())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

pub(crate) fn should_retry_model_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    !message.contains("provider stream failed after")
        && (message.contains("429")
            || message.contains("too many requests")
            || message.contains("unexpected eof")
            || message.contains("connection")
            || message.contains("timeout")
            || message.contains("timed out"))
}

pub(crate) fn is_usage_limit_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("freeusagelimiterror")
        || message.contains("free usage limit")
        || message.contains("usage limit exceeded")
}

pub(crate) fn model_retry_delay(message: &str, attempt: usize) -> Duration {
    if let Some(delay) = parse_requested_retry_delay(message) {
        return delay.min(Duration::from_secs(60));
    }

    if message.to_ascii_lowercase().contains("429")
        || message.to_ascii_lowercase().contains("too many requests")
    {
        return Duration::from_secs((attempt as u64).saturating_mul(10).min(60));
    }

    Duration::from_secs(
        2_u64
            .saturating_pow(attempt.saturating_sub(1) as u32)
            .min(15),
    )
}

fn parse_requested_retry_delay(message: &str) -> Option<Duration> {
    let marker = "requested delay: Some(";
    let start = message.find(marker)? + marker.len();
    let end = message[start..].find(')')? + start;
    parse_duration_fragment(&message[start..end])
}

fn parse_duration_fragment(fragment: &str) -> Option<Duration> {
    let value = fragment.trim();
    if let Some(ms) = value.strip_suffix("ms") {
        return ms.trim().parse::<u64>().ok().map(Duration::from_millis);
    }
    if let Some(secs) = value.strip_suffix('s') {
        return secs.trim().parse::<f64>().ok().map(Duration::from_secs_f64);
    }
    None
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn exhausted_provider_reconnects_are_not_retried_by_the_tui() {
        assert!(!should_retry_model_error(
            "Provider stream failed after 5 attempts: Connection failed"
        ));
    }

    #[test]
    fn model_retry_delay_uses_rate_limit_backoff_without_requested_delay() {
        let delay = model_retry_delay(
            "API error 429 Too Many Requests: {\"status\":429} (requested delay: None)",
            2,
        );

        assert_eq!(delay, Duration::from_secs(20));
    }

    #[test]
    fn model_retry_delay_uses_requested_delay_when_present() {
        let delay = model_retry_delay(
            "API error 429 Too Many Requests: {} (requested delay: Some(1500ms))",
            1,
        );

        assert_eq!(delay, Duration::from_millis(1500));
    }

    #[test]
    fn model_retry_delay_caps_large_requested_delay() {
        let delay = model_retry_delay(
            "API error 429 Too Many Requests: {} (requested delay: Some(64649s))",
            1,
        );

        assert_eq!(delay, Duration::from_secs(60));
    }

    #[test]
    fn handle_model_error_deduplicates_identical_tail_error() {
        let mut app = crate::tests::test_app("");

        handle_model_error(&mut app, "access denied".to_string());
        handle_model_error(&mut app, "access denied".to_string());

        let error_messages = app
            .messages
            .iter()
            .filter(|message| message.status.as_deref() == Some("error"))
            .count();
        let error_events = app
            .events
            .iter()
            .filter(|event| matches!(event, AgentEvent::Error { .. }))
            .count();
        assert_eq!(error_messages, 1);
        assert_eq!(error_events, 1);
    }
}
