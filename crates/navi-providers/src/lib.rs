//! Provider facade for NAVI.
//!
//! This crate re-exports the public API from the underlying provider
//! implementation (`navi-openai`). Downstream crates that need provider
//! types should depend on `navi-providers` rather than `navi-openai`
//! directly, so the implementation crate can be swapped or split later
//! without widespread churn.

pub use navi_openai::errors;
pub use navi_openai::oauth;
pub use navi_openai::types;

pub use navi_openai::CommandCodeUsageData;
pub use navi_openai::DeviceOAuthStarted;
pub use navi_openai::OpenAiApiKind;
pub use navi_openai::OpenAiProvider;
pub use navi_openai::ProviderError;
pub use navi_openai::commandcode_browser_oauth;
pub use navi_openai::commandcode_fetch_usage_data;
pub use navi_openai::commandcode_list_models;
pub use navi_openai::github_copilot_device_oauth;

// Convenience re-export matching navi-openai's public surface.
pub use navi_core::ProviderId;
