use std::future::Future;
use std::path::PathBuf;

use anyhow::Result;
use tokio::sync::mpsc;

use navi_core::{
    CredentialStore, LoadedConfig, RuntimeEvent, canonical_provider_id, resolve_provider_config,
    resolve_provider_credential_status,
};
use navi_sdk::{NaviEngine, NaviEngineBuilder};

use crate::dispatch::AsyncEvent;

pub(crate) fn forward_runtime_event_to_tui(
    event: RuntimeEvent,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
) {
    if let Some(event) = event.into_agent_event() {
        let _ = tx.send(AsyncEvent::Agent(event));
    }
}

pub(crate) fn spawn_runtime_task<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(future);
    }
}

pub(crate) fn build_engine(
    loaded_config: &LoadedConfig,
    project_dir: PathBuf,
) -> Result<NaviEngine> {
    NaviEngineBuilder::from_project(project_dir)
        .loaded_config(loaded_config.clone())
        .build()
}

pub(crate) fn selected_model_runtime_available(
    loaded_config: &LoadedConfig,
    credential_store: &CredentialStore,
) -> bool {
    let Some(provider_config) =
        resolve_provider_config(&loaded_config.config, &loaded_config.config.model.provider)
    else {
        return false;
    };
    resolve_provider_credential_status(
        credential_store,
        &provider_config,
        &loaded_config.config.model.provider,
        Some(&loaded_config.config.model.name),
    )
    .configured
}

pub(crate) fn provider_supports_oauth(provider_id: &str) -> bool {
    canonical_provider_id(provider_id) == "github-copilot"
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use navi_core::{AgentEvent, RuntimeEvent, RuntimeEventKind};

    use super::*;

    #[test]
    fn forward_runtime_event_maps_deltas_to_agent_events() {
        let (async_tx, mut async_rx) = mpsc::unbounded_channel();

        forward_runtime_event_to_tui(
            RuntimeEvent::new(RuntimeEventKind::AssistantDelta {
                text: "final answer".to_string(),
            }),
            &async_tx,
        );

        let first = async_rx.try_recv().ok();
        assert!(matches!(
            first,
            Some(AsyncEvent::Agent(AgentEvent::ModelDelta { text })) if text == "final answer"
        ));
    }
}
