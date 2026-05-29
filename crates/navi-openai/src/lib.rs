pub mod types;

pub mod errors;
mod mapping;
pub mod oauth;
mod provider;
mod providers;
mod sse;
mod transport;

pub use errors::ProviderError;
pub use oauth::{DeviceOAuthStarted, github_copilot_device_oauth};
pub use provider::OpenAiProvider;
pub use types::OpenAiApiKind;
pub use navi_core::ProviderId;

#[cfg(test)]
fn extract_output_text(value: &serde_json::Value) -> String {
    if let Some(text) = value.get("output_text").and_then(|v| v.as_str()) {
        return text.to_string();
    }

    value
        .get("output")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .flat_map(|item| {
            item.get("content")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
        })
        .filter_map(|content| content.get("text").and_then(|v| v.as_str()))
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
fn extract_chat_completion_text(value: &serde_json::Value) -> String {
    value
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests;
