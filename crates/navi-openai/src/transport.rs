use crate::errors::ProviderError;
use reqwest::Response;
use serde_json::Value;
use std::time::Duration;

pub(crate) async fn ensure_success(
    response: Response,
) -> std::result::Result<Response, ProviderError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let mut requested_delay = None;
    if let Some(retry_after_header) = response.headers().get(reqwest::header::RETRY_AFTER)
        && let Ok(retry_after_str) = retry_after_header.to_str()
    {
        requested_delay = parse_retry_after(retry_after_str);
    }

    let (body, body_read_error) = match response.text().await {
        Ok(text) => (text, None),
        Err(err) => (
            "<failed to read error body>".to_string(),
            Some(err.to_string()),
        ),
    };

    if let Ok(json_body) = serde_json::from_str::<serde_json::Value>(&body)
        && let Some(delay) = extract_requested_delay_from_json(&json_body)
    {
        requested_delay = Some(delay);
    }

    if let Some(read_err) = &body_read_error {
        tracing::warn!(
            status = %status,
            ?requested_delay,
            body_read_error = %read_err,
            "provider request failed and body could not be read"
        );
    } else {
        tracing::warn!(status = %status, ?requested_delay, "provider request failed");
    }
    Err(ProviderError::Api {
        status,
        body,
        requested_delay,
        body_read_error,
    })
}

pub(crate) fn should_retry_status(status: reqwest::StatusCode, retry_429: bool) -> bool {
    status.is_server_error() || (retry_429 && status == reqwest::StatusCode::TOO_MANY_REQUESTS)
}

pub(crate) fn should_retry_error(err: &ProviderError, retry_429: bool) -> bool {
    match err {
        ProviderError::Transport(_) => true,
        ProviderError::Api { status, body, .. } => {
            should_retry_status(*status, retry_429) && !is_usage_limit_error(body)
        }
        ProviderError::StreamIdleTimeout(_) => true,
        ProviderError::InvalidHeader(_) | ProviderError::Other(_) => false,
    }
}

pub(crate) fn retry_delay_for_error(err: &ProviderError, attempt: u32) -> Duration {
    const MAX_REQUESTED_RETRY_DELAY: Duration = Duration::from_secs(60);

    if let ProviderError::Api {
        requested_delay: Some(delay),
        ..
    } = err
    {
        return (*delay).min(MAX_REQUESTED_RETRY_DELAY);
    }

    get_backoff_delay(attempt)
}

fn is_usage_limit_error(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    body.contains("freeusagelimiterror")
        || body.contains("free usage limit")
        || body.contains("usage limit exceeded")
}

fn get_jitter() -> f64 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(123456789);
    let a: u64 = 6364136223846793005;
    let c: u64 = 1442695040888963407;
    let seed = nanos as u64;
    let rand_val = seed.wrapping_mul(a).wrapping_add(c);
    let normalized = (rand_val as f64) / (u64::MAX as f64);
    (normalized * 0.20) - 0.10
}

pub(crate) fn get_backoff_delay(attempt: u32) -> Duration {
    let exponent = (attempt.saturating_sub(1)).min(10);
    let base_ms = 200 * (1 << exponent);

    let jitter_pct = get_jitter();
    let jitter_ms = (base_ms as f64 * jitter_pct) as i64;
    let final_ms = (base_ms as i64 + jitter_ms).max(0) as u64;

    Duration::from_millis(final_ms)
}

fn parse_retry_after(header_val: &str) -> Option<Duration> {
    if let Ok(seconds) = header_val.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    None
}

fn value_to_duration_seconds(val: &Value) -> Option<Duration> {
    if let Some(ms) = val.as_u64() {
        Some(Duration::from_secs(ms))
    } else {
        val.as_f64().map(Duration::from_secs_f64)
    }
}

pub(crate) fn extract_requested_delay_from_json(json: &Value) -> Option<Duration> {
    if let Some(val) = json.get("requested_delay_ms").and_then(Value::as_u64) {
        return Some(Duration::from_millis(val));
    }
    if let Some(error) = json.get("error")
        && let Some(val) = error.get("requested_delay_ms").and_then(Value::as_u64)
    {
        return Some(Duration::from_millis(val));
    }

    if let Some(val) = json.get("requested_delay")
        && let Some(dur) = value_to_duration_seconds(val)
    {
        return Some(dur);
    }
    if let Some(error) = json.get("error")
        && let Some(val) = error.get("requested_delay")
        && let Some(dur) = value_to_duration_seconds(val)
    {
        return Some(dur);
    }

    None
}
