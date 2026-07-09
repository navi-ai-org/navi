//! Device/browser OAuth and registry listing on [`NaviEngine`].

use navi_core::{CredentialStore, provider_catalog};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::credentials::{
    DeviceOAuthStarted, provider_supports_device_oauth, start_provider_device_oauth,
};
use crate::engine::NaviEngine;
use crate::types::NaviError;

type Result<T> = std::result::Result<T, NaviError>;

/// Lightweight registry row for desktop UIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryProviderSummary {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub model_count: usize,
}

/// OAuth start payload returned when the provider shows a user code / URI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceOAuthStartedInfo {
    pub verification_uri: String,
    pub user_code: String,
}

impl NaviEngine {
    /// Whether device/browser OAuth is supported for this provider id.
    pub fn provider_supports_device_oauth(&self, provider_id: &str) -> bool {
        provider_supports_device_oauth(provider_id)
    }

    /// Run device/browser OAuth for a provider.
    ///
    /// `on_started` is invoked when the verification URI / user code is ready
    /// (desktop should open browser / show code). Blocks until the flow finishes.
    /// Returns optional secondary token/string some providers yield (e.g. CommandCode).
    pub async fn start_device_oauth<F>(
        &self,
        provider_id: &str,
        mut on_started: F,
    ) -> Result<Option<String>>
    where
        F: FnMut(DeviceOAuthStartedInfo) + Send,
    {
        let loaded = self.loaded_config();
        let store = CredentialStore::new(loaded.data_dir.clone());
        start_provider_device_oauth(&store, provider_id, |started: DeviceOAuthStarted| {
            on_started(DeviceOAuthStartedInfo {
                verification_uri: started.verification_uri,
                user_code: started.user_code,
            });
        })
        .await
        .map_err(|e| NaviError::Config(e.to_string()))
    }

    /// Convenience OAuth without progress callback (still blocks until complete).
    pub async fn start_device_oauth_simple(&self, provider_id: &str) -> Result<Option<String>> {
        self.start_device_oauth(provider_id, |_| {}).await
    }

    /// List providers/models from the current config catalog (post-registry).
    pub fn list_registry(&self) -> Result<serde_json::Value> {
        let loaded = self.loaded_config();
        let providers = provider_catalog(&loaded.config);
        let rows: Vec<RegistryProviderSummary> = providers
            .iter()
            .map(|p| RegistryProviderSummary {
                id: p.id.clone(),
                label: p.label.clone(),
                kind: format!("{:?}", p.kind).to_lowercase(),
                model_count: p.models.len(),
            })
            .collect();
        let total_models: usize = rows.iter().map(|r| r.model_count).sum();
        Ok(json!({
            "provider_count": rows.len(),
            "model_count": total_models,
            "providers": rows,
        }))
    }
}
