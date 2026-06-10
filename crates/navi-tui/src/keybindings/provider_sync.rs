use navi_sdk::{NaviConfigSaveTarget, NaviProviderSyncReport};

use crate::app::TuiApp;
use crate::dispatch::AsyncEvent;
use crate::state::{ChatMessage, ChatRole};

fn sync_summary(report: &NaviProviderSyncReport) -> String {
    let mut parts = Vec::new();
    if !report.updated.is_empty() {
        let models = report
            .updated
            .iter()
            .map(|provider| format!("{} ({} models)", provider.provider_id, provider.model_count))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("synced {models}"));
    }
    if !report.skipped.is_empty() {
        parts.push(format!("{} skipped", report.skipped.len()));
    }
    if !report.failed.is_empty() {
        let failures = report
            .failed
            .iter()
            .map(|failure| format!("{}: {}", failure.provider_id, failure.message))
            .collect::<Vec<_>>()
            .join("; ");
        parts.push(format!("failed {failures}"));
    }
    if parts.is_empty() {
        "No provider models were synced".to_string()
    } else {
        format!("Model sync complete: {}", parts.join("; "))
    }
}

pub fn sync_models_tui(app: &mut TuiApp) {
    app.is_loading = true;
    let sender = app.async_sender();
    let engine = app.engine();
    tokio::spawn(async move {
        // First sync the remote registry (fetches latest provider/model definitions).
        let registry_msg = match engine.sync_registry(false).await {
            Ok(true) => Some("Registry updated from remote. "),
            Ok(false) => None,
            Err(err) => {
                tracing::warn!(error = %err, "registry sync failed, continuing with model sync");
                None
            }
        };

        // Then sync provider models (queries each provider's API).
        let result = engine.sync_models(NaviConfigSaveTarget::Auto).await;
        let event = match result {
            Ok(report) => {
                let summary = sync_summary(&report);
                let message = match registry_msg {
                    Some(prefix) => format!("{prefix}{summary}"),
                    None => summary,
                };
                AsyncEvent::SyncCompleted {
                    message,
                    loaded_config: report.loaded_config,
                }
            }
            Err(err) => AsyncEvent::SyncCompleted {
                message: format!("Model sync failed: {err}"),
                loaded_config: engine.loaded_config(),
            },
        };
        let _ = sender.send(event);
    });
    app.loading_start = Some(std::time::Instant::now());
    app.messages.push(ChatMessage {
        status: Some("syncing".to_string()),
        ..ChatMessage::new(
            ChatRole::Assistant,
            "Syncing registry and models from providers...".to_string(),
        )
    });
}

pub fn sync_provider_tui(app: &mut TuiApp, provider_id: &str) {
    app.is_loading = true;
    let sender = app.async_sender();
    let engine = app.engine();
    let provider_id = provider_id.to_string();
    let provider_label = provider_id.clone();
    tokio::spawn(async move {
        let result = engine
            .sync_provider_models(&provider_id, NaviConfigSaveTarget::Auto)
            .await;
        let event = match result {
            Ok(report) => AsyncEvent::SyncCompleted {
                message: sync_summary(&report),
                loaded_config: report.loaded_config,
            },
            Err(err) => AsyncEvent::SyncCompleted {
                message: format!("Model sync failed: {err}"),
                loaded_config: engine.loaded_config(),
            },
        };
        let _ = sender.send(event);
    });
    app.loading_start = Some(std::time::Instant::now());
    app.messages.push(ChatMessage {
        status: Some("syncing".to_string()),
        ..ChatMessage::new(
            ChatRole::Assistant,
            format!("Syncing models for {provider_label}..."),
        )
    });
}
