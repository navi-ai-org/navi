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

pub use navi_openai::CharmHyperCreditsReport;
pub use navi_openai::CommandCodeUsageData;
pub use navi_openai::DeviceOAuthStarted;
pub use navi_openai::HYPERCREDIT_USD;
pub use navi_openai::OpenAiApiKind;
pub use navi_openai::OpenAiProvider;
pub use navi_openai::OpenAiUsageLimitSnapshot;
pub use navi_openai::OpenAiUsageReport;
pub use navi_openai::OpenAiUsageWindow;
pub use navi_openai::OpenRouterUsageReport;
pub use navi_openai::ProviderError;
pub use navi_openai::{XAI_GROK_CLI_BASE_URL, XAI_GROK_CLI_CLIENT_VERSION};
pub use navi_openai::XaiProductUsage;
pub use navi_openai::XaiUsageReport;
pub use navi_openai::charm_hyper_credits_report;
pub use navi_openai::commandcode_browser_oauth;
pub use navi_openai::commandcode_fetch_usage_data;
pub use navi_openai::commandcode_list_models;
pub use navi_openai::ensure_xai_access_token;
pub use navi_openai::extract_hypercredit_balance_from_usage;
pub use navi_openai::format_hypercredits;
pub use navi_openai::github_copilot_device_oauth;
pub use navi_openai::hyper_base_url;
pub use navi_openai::hypercredits_to_usd;
pub use navi_openai::is_xai_oauth_access_token;
pub use navi_openai::openai_browser_oauth;
pub use navi_openai::openai_usage_report;
pub use navi_openai::openrouter_usage_report;
pub use navi_openai::peek_hypercredit_balance;
pub use navi_openai::set_hypercredit_balance;
pub use navi_openai::set_hypercredit_balance_authoritative;
pub use navi_openai::take_hypercredit_balance;
pub use navi_openai::usd_to_hypercredits;
pub use navi_openai::xai_browser_oauth;
pub use navi_openai::xai_device_oauth;
pub use navi_openai::xai_oauth;
pub use navi_openai::xai_usage_report;

// Convenience re-export matching navi-openai's public surface.
pub use navi_core::ProviderId;
