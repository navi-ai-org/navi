//! Provider credential helpers for embedders (TUI, Tutor, CLI).
//!
//! Prefer these APIs over provider-specific OAuth entry points.

use anyhow::{Result, bail};
use navi_core::{
    CredentialStore, ProviderConfig, ProviderId, canonical_provider_id, resolve_provider_api_key,
    resolve_provider_credential_status,
};
use std::path::Path;

pub use navi_core::{CredentialAccountInfo, CredentialSource, CredentialStatus};
pub use navi_providers::CommandCodeUsageData;
pub use navi_providers::DeviceOAuthStarted;

/// Whether the provider supports browser/device OAuth through the SDK.
pub fn provider_supports_device_oauth(provider_id: &str) -> bool {
    matches!(
        canonical_provider_id(provider_id),
        "commandcode" | "github-copilot"
    )
}

/// Start browser/device OAuth for a supported provider.
///
/// Currently supported: `commandcode`, `github-copilot`.
pub async fn start_provider_device_oauth<F>(
    credential_store: &CredentialStore,
    provider_id: &str,
    on_started: F,
) -> Result<Option<String>>
where
    F: FnMut(DeviceOAuthStarted) + Send,
{
    match canonical_provider_id(provider_id) {
        "commandcode" => navi_providers::commandcode_browser_oauth(
            credential_store.clone(),
            provider_id,
            on_started,
        )
        .await
        .map(Some)
        .map_err(|err| anyhow::anyhow!(err)),
        "github-copilot" => navi_providers::github_copilot_device_oauth(
            credential_store.clone(),
            provider_id,
            on_started,
        )
        .await
        .map(|_| None)
        .map_err(|err| anyhow::anyhow!(err)),
        other => bail!("device OAuth is not supported for provider '{other}'"),
    }
}

pub fn provider_credential_accounts(
    credential_store: &CredentialStore,
    provider_id: &str,
    project_dir: Option<&Path>,
) -> Result<Vec<CredentialAccountInfo>> {
    credential_store.list_credential_accounts(provider_id, project_dir)
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

pub async fn commandcode_usage_data(
    credential_store: &CredentialStore,
) -> Result<CommandCodeUsageData> {
    let api_key = credential_store
        .get_api_key(ProviderId::COMMANDCODE)
        .ok_or_else(|| anyhow::anyhow!("missing stored Command Code credential"))?;
    navi_providers::commandcode_fetch_usage_data(&api_key)
        .await
        .map_err(|err| anyhow::anyhow!(err))
}

pub async fn commandcode_remote_models(credential_store: &CredentialStore) -> Result<Vec<String>> {
    let api_key = credential_store
        .get_api_key(ProviderId::COMMANDCODE)
        .ok_or_else(|| anyhow::anyhow!("missing stored Command Code credential"))?;
    navi_providers::commandcode_list_models(&api_key)
        .await
        .map_err(|err| anyhow::anyhow!(err))
}
