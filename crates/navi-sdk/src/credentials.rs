//! Provider credential helpers for embedders (TUI, Tutor, CLI).
//!
//! Prefer these APIs over provider-specific OAuth entry points.

use anyhow::{Result, bail};
use navi_core::{
    CredentialStore, ProviderConfig, canonical_provider_id, resolve_provider_api_key,
    resolve_provider_credential_status,
};
use std::path::Path;

pub use navi_core::{CredentialAccountInfo, CredentialSource, CredentialStatus};
pub use navi_providers::DeviceOAuthStarted;

/// Whether the provider supports browser/device OAuth through the SDK.
pub fn provider_supports_device_oauth(provider_id: &str) -> bool {
    matches!(
        canonical_provider_id(provider_id),
        "openai" | "github-copilot" | "xai"
    )
}

/// Start browser/device OAuth for a supported provider.
///
/// Currently supported: `openai`, `github-copilot`, `xai`.
///
/// For `xai`, the default is browser OIDC (PKCE + loopback). Set
/// `NAVI_XAI_OAUTH_DEVICE=1` to force the device-code flow.
pub async fn start_provider_device_oauth<F>(
    credential_store: &CredentialStore,
    provider_id: &str,
    on_started: F,
) -> Result<Option<String>>
where
    F: FnMut(DeviceOAuthStarted) + Send,
{
    match canonical_provider_id(provider_id) {
        "openai" => {
            navi_providers::openai_browser_oauth(credential_store.clone(), provider_id, on_started)
                .await
                .map(|_| None)
                .map_err(|err| anyhow::anyhow!("openai browser OAuth failed: {err}"))
        }
        "github-copilot" => navi_providers::github_copilot_device_oauth(
            credential_store.clone(),
            provider_id,
            on_started,
        )
        .await
        .map(|_| None)
        .map_err(|err| anyhow::anyhow!("github-copilot device OAuth failed: {err}")),
        "xai" => navi_providers::xai_oauth(credential_store.clone(), provider_id, on_started)
            .await
            .map(|_| None)
            .map_err(|err| anyhow::anyhow!("xai OAuth failed: {err}")),
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
