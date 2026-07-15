pub mod types;

pub mod errors;
mod mapping;
pub mod oauth;
mod provider;
mod providers;
mod sse;
mod transport;

pub use errors::ProviderError;
pub use navi_core::ProviderId;
pub use oauth::{
    CharmHyperCreditsReport, CommandCodeUsageData, DeviceOAuthStarted, HYPERCREDIT_USD,
    OpenAiUsageLimitSnapshot, OpenAiUsageReport, OpenAiUsageWindow, OpenRouterUsageReport,
    XAI_GROK_CLI_BASE_URL, XAI_GROK_CLI_CLIENT_MODE, XAI_GROK_CLI_CLIENT_SURFACE,
    XAI_GROK_CLI_CLIENT_VERSION, XaiProductUsage, XaiUsageReport, charm_hyper_credits_report,
    commandcode_browser_oauth, commandcode_fetch_usage_data, commandcode_list_models,
    ensure_xai_access_token, extract_hypercredit_balance_from_usage, format_hypercredits,
    github_copilot_device_oauth, hyper_base_url, hypercredits_to_usd, is_xai_oauth_access_token,
    openai_browser_oauth, openai_usage_report, openrouter_usage_report, peek_hypercredit_balance,
    set_hypercredit_balance, set_hypercredit_balance_authoritative, take_hypercredit_balance,
    usd_to_hypercredits, xai_agent_id, xai_browser_oauth, xai_client_identifier, xai_device_oauth,
    xai_grok_cli_client_version, xai_oauth, xai_usage_report,
};
pub use provider::OpenAiProvider;
pub use types::OpenAiApiKind;

#[cfg(test)]
mod tests;
