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
    let provider_error = ProviderErrorBody::parse(body);
    let provider_code = provider_error.code.as_deref();
    let provider_detail = provider_error.detail();
    let user_message = match (status_code, provider_code) {
        (400, Some("unsupported_model")) => "The selected model is not in this provider catalog. Use ctrl+m to select a different model.".to_string(),
        (400, Some("invalid_request_error")) => "The provider rejected the request shape. This may mean the selected model is using the wrong endpoint or unsupported parameters.".to_string(),
        (400, _) => {
            if body.contains("invalid_api_key") || body.contains("Invalid API key") {
                "Invalid API key format. Please check your API key configuration.".to_string()
            } else if body.contains("model") && (body.contains("not found") || body.contains("does not exist")) {
                "The selected model is not available for this provider. Use ctrl+m to select a different model.".to_string()
            } else {
                "The request was invalid. This may be a configuration issue.".to_string()
            }
        }
        (401, Some("authentication_error")) | (401, _) => "Authentication failed. Please verify your API key is correct and active.".to_string(),
        (403, Some("upgrade_required")) => "Access denied. The provider says this account must upgrade to the Provider plan or higher before using this API.".to_string(),
        (403, _) => "Access denied. Your API key may not have permission for this resource, or the account may be suspended.".to_string(),
        (404, _) => "The API endpoint was not found. This usually means the provider base URL is misconfigured.".to_string(),
        (408 | 504, _) => "The request timed out. The provider may be experiencing high load. Try again in a moment.".to_string(),
        (422, _) => "The request contained invalid parameters. This may be a model compatibility issue.".to_string(),
        (429, Some("insufficient_quota")) => {
            "Quota exhausted for the credential actually used by this provider. Check the selected provider/model and make sure the stored credential belongs to an OpenAI Platform project with API billing/quota; ChatGPT/OAuth connector access is not the same as Platform API quota.".to_string()
        }
        (429, Some("rate_limit_error")) | (429, _) => {
            if let Some(delay) = requested_delay {
                format!("Rate limited. The provider asks to wait {} before retrying.", format_delay(delay))
            } else {
                "Rate limited. Too many requests sent to the provider. Please wait a moment before trying again.".to_string()
            }
        }
        (500, Some("server_error" | "api_error")) | (500, _) => "The provider encountered an upstream error. This is usually temporary.".to_string(),
        (502, _) => "Bad gateway. The provider is temporarily unavailable.".to_string(),
        (503, _) => "The provider service is temporarily unavailable. They may be undergoing maintenance.".to_string(),
        (_, Some("server_error" | "api_error")) if status_code >= 500 => format!("The provider returned an upstream server error ({}). This is usually temporary.", status_code),
        (_, _) if status_code >= 500 => format!("The provider returned an unexpected server error ({}). This is usually temporary.", status_code),
        (426, _) => {
            if body.contains("outdated") || body.contains("Grok CLI version") {
                "Grok/xAI rejected the request as an outdated CLI client (HTTP 426). NAVI should call api.x.ai directly — rebuild/update NAVI, or use an XAI_API_KEY from console.x.ai.".to_string()
            } else {
                "The provider requires a protocol or client upgrade (HTTP 426).".to_string()
            }
        }
        (_, _) if status_code >= 400 => format!("The provider rejected the request ({}).", status_code),
        (_, _) => format!("Unexpected response status: {}", status_code),
    };

    if let Some(detail) = provider_detail {
        format!("{user_message}\nProvider detail: {detail}")
    } else {
        user_message
    }
}

#[derive(Debug, Default)]
struct ProviderErrorBody {
    message: Option<String>,
    code: Option<String>,
}

impl ProviderErrorBody {
    fn parse(body: &str) -> Self {
        let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
            return Self::default();
        };
        let error = json.get("error").unwrap_or(&json);
        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .or_else(|| json.get("message").and_then(serde_json::Value::as_str))
            .filter(|message| !message.is_empty())
            .map(str::to_string);
        let code = error
            .get("code")
            .and_then(serde_json::Value::as_str)
            .or_else(|| error.get("type").and_then(serde_json::Value::as_str))
            .or_else(|| json.get("code").and_then(serde_json::Value::as_str))
            .or_else(|| json.get("type").and_then(serde_json::Value::as_str))
            .filter(|code| !code.is_empty())
            .map(str::to_string);

        Self { message, code }
    }

    fn detail(&self) -> Option<String> {
        match (self.message.as_deref(), self.code.as_deref()) {
            (Some(message), Some(code)) => Some(format!("{message} ({code})")),
            (Some(message), _) => Some(message.to_string()),
            (_, Some(code)) => Some(code.to_string()),
            _ => None,
        }
    }
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

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;

    use super::*;

    #[test]
    fn api_error_includes_provider_code_and_message() {
        let body = r#"{"error":{"message":"Upgrade to Provider plan or higher.","type":"permission_error","code":"upgrade_required"}}"#;

        let message = format_api_error(&StatusCode::FORBIDDEN, body, &None);

        assert!(message.contains("Provider plan or higher"));
        assert!(message.contains("Upgrade to Provider plan or higher."));
        assert!(message.contains("upgrade_required"));
    }

    #[test]
    fn api_error_maps_unsupported_model() {
        let body = r#"{"error":{"message":"Model is not in catalog.","code":"unsupported_model"}}"#;

        let message = format_api_error(&StatusCode::BAD_REQUEST, body, &None);

        assert!(message.contains("not in this provider catalog"));
        assert!(message.contains("unsupported_model"));
    }

    #[test]
    fn api_error_maps_wrong_endpoint_request_shape() {
        let body =
            r#"{"error":{"message":"Wrong endpoint for model.","type":"invalid_request_error"}}"#;

        let message = format_api_error(&StatusCode::BAD_REQUEST, body, &None);

        assert!(message.contains("wrong endpoint"));
        assert!(message.contains("invalid_request_error"));
    }

    #[test]
    fn anthropic_error_type_is_used_as_provider_code() {
        let body =
            r#"{"type":"error","error":{"type":"authentication_error","message":"Missing auth"}}"#;

        let message = format_api_error(&StatusCode::UNAUTHORIZED, body, &None);

        assert!(message.contains("Authentication failed"));
        assert!(message.contains("authentication_error"));
    }
}
