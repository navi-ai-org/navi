use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("API error {status}: {body} (requested delay: {requested_delay:?})")]
    Api {
        status: reqwest::StatusCode,
        body: String,
        requested_delay: Option<Duration>,
        /// Reason the response body could not be read, if any. Helps debugging
        /// transport-level failures (encoding, premature close, etc.) where the
        /// body itself is unavailable.
        body_read_error: Option<String>,
    },
    #[error("Transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("Stream idle timeout: stream was idle for more than {0:?}")]
    StreamIdleTimeout(Duration),
    #[error("Invalid header value: {0}")]
    InvalidHeader(#[from] reqwest::header::InvalidHeaderValue),
    #[error("Other error: {0}")]
    Other(String),
}
