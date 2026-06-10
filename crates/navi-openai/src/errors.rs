use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("{}", format_api_error(.status, .body, .requested_delay))]
    Api {
        status: reqwest::StatusCode,
        body: String,
        requested_delay: Option<Duration>,
        /// Reason the response body could not be read, if any. Helps debugging
        /// transport-level failures (encoding, premature close, etc.) where the
        /// body itself is unavailable.
        body_read_error: Option<String>,
    },
    #[error("Connection failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("Response timeout: no data received for {0:?}")]
    StreamIdleTimeout(Duration),
    #[error("Invalid header value: {0}")]
    InvalidHeader(#[from] reqwest::header::InvalidHeaderValue),
    #[error("{0}")]
    Other(String),
}

fn format_api_error(
    status: &reqwest::StatusCode,
    body: &str,
    requested_delay: &Option<Duration>,
) -> String {
    let status_code = status.as_u16();
    let user_message = match status_code {
        400 => {
            if body.contains("invalid_api_key") || body.contains("Invalid API key") {
                "Invalid API key format. Please check your API key configuration.".to_string()
            } else if body.contains("model") && (body.contains("not found") || body.contains("does not exist")) {
                "The selected model is not available for this provider. Use ctrl+m to select a different model.".to_string()
            } else {
                "The request was invalid. This may be a configuration issue.".to_string()
            }
        }
        401 => "Authentication failed. Please verify your API key is correct and active.".to_string(),
        403 => "Access denied. Your API key may not have permission for this resource, or the account may be suspended.".to_string(),
        404 => "The API endpoint was not found. This usually means the provider base URL is misconfigured.".to_string(),
        408 | 504 => "The request timed out. The provider may be experiencing high load. Try again in a moment.".to_string(),
        422 => "The request contained invalid parameters. This may be a model compatibility issue.".to_string(),
        429 => {
            if let Some(delay) = requested_delay {
                format!("Rate limited. The provider asks to wait {} before retrying.", format_delay(delay))
            } else {
                "Rate limited. Too many requests sent to the provider. Please wait a moment before trying again.".to_string()
            }
        }
        500 => "The provider encountered an internal error. This is a temporary issue on their side.".to_string(),
        502 => "Bad gateway. The provider is temporarily unavailable.".to_string(),
        503 => "The provider service is temporarily unavailable. They may be undergoing maintenance.".to_string(),
        _ if status_code >= 500 => format!("The provider returned an unexpected server error ({}). This is usually temporary.", status_code),
        _ if status_code >= 400 => format!("The provider rejected the request ({}).", status_code),
        _ => format!("Unexpected response status: {}", status_code),
    };

    user_message
}

fn format_delay(delay: &Duration) -> String {
    if delay.as_secs() >= 60 {
        format!("{} minutes", delay.as_secs() / 60)
    } else if delay.as_secs() > 0 {
        format!("{} seconds", delay.as_secs())
    } else {
        format!("{}ms", delay.as_millis())
    }
}
