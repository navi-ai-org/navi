//! Provider credential helpers for embedders (TUI, Tutor, CLI).
//!
//! Prefer these APIs over provider-specific OAuth entry points.

use anyhow::{Result, bail};
use navi_core::{
    CredentialStore, ProviderConfig, canonical_provider_id, resolve_provider_api_key,
    resolve_provider_credential_status,
};

pub use navi_core::{CredentialSource, CredentialStatus};
pub use navi_providers::DeviceOAuthStarted;

/// Whether the provider supports device-code OAuth through the SDK.
pub fn provider_supports_device_oauth(provider_id: &str) -> bool {
    matches!(canonical_provider_id(provider_id), "github-copilot")
}

/// Start device-code OAuth for a supported provider.
///
/// Currently supported: `github-copilot`.
pub async fn start_provider_device_oauth<F>(
    credential_store: &CredentialStore,
    provider_id: &str,
    on_started: F,
) -> Result<()>
where
    F: FnMut(DeviceOAuthStarted) + Send,
{
    match canonical_provider_id(provider_id) {
        "github-copilot" => navi_providers::github_copilot_device_oauth(
            credential_store.clone(),
            provider_id,
            on_started,
        )
        .await
        .map_err(|err| anyhow::anyhow!(err)),
        other => bail!("device OAuth is not supported for provider '{other}'"),
    }
}

/// Resolve whether credentials are configured for a provider/model pair.
pub fn provider_credential_status(
    credential_store: &CredentialStore,
    provider_config: &ProviderConfig,
    requested_provider_id: &str,
    model: Option<&str>,
) -> CredentialStatus {
    resolve_provider_credential_status(
        credential_store,
        provider_config,
        requested_provider_id,
        model,
    )
}

/// Resolve an API key for a provider (env, external auth, or credential store).
pub fn provider_api_key(
    credential_store: &CredentialStore,
    provider_config: &ProviderConfig,
    requested_provider_id: &str,
) -> Option<String> {
    resolve_provider_api_key(credential_store, provider_config, requested_provider_id)
}
