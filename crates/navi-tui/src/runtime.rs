use std::future::Future;
use std::path::PathBuf;

use anyhow::Result;
use tokio::sync::mpsc;

use navi_sdk::{
    CredentialStore, LoadedConfig, NaviEngine, NaviEngineBuilder, RuntimeEvent,
    provider_supports_device_oauth, resolve_provider_api_key_for_project, resolve_provider_config,
    resolve_provider_credential_status,
};

use crate::dispatch::AsyncEvent;

#[cfg(test)]
pub(crate) fn forward_runtime_event_to_tui(
    event: RuntimeEvent,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
) {
    forward_runtime_event_to_tui_inner(event, tx, None);
}

pub(crate) fn forward_runtime_event_to_tui_for_session(
    event: RuntimeEvent,
    session_id: &str,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
) {
    forward_runtime_event_to_tui_inner(event, tx, Some(session_id));
}

fn forward_runtime_event_to_tui_inner(
    event: RuntimeEvent,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
    session_id: Option<&str>,
) {
    if let Some(event) = event.into_agent_event() {
        let async_event = match session_id {
            Some(session_id) => AsyncEvent::AgentForSession {
                session_id: session_id.to_string(),
                event,
            },
            None => AsyncEvent::Agent(event),
        };
        let _ = tx.send(async_event);
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
    Ok(NaviEngineBuilder::from_project(project_dir)
        .loaded_config(loaded_config.clone())
        .build()?)
}

pub(crate) fn selected_model_runtime_available(
    loaded_config: &LoadedConfig,
    credential_store: &CredentialStore,
    project_dir: &std::path::Path,
) -> bool {
    let Some(provider_config) =
        resolve_provider_config(&loaded_config.config, &loaded_config.config.model.provider)
    else {
        return false;
    };
    if resolve_provider_api_key_for_project(
        credential_store,
        &provider_config,
        &loaded_config.config.model.provider,
        project_dir,
    )
    .is_some()
    {
        return true;
    }

    resolve_provider_credential_status(
        credential_store,
        &provider_config,
        &loaded_config.config.model.provider,
        Some(&loaded_config.config.model.name),
    )
    .configured
}

pub(crate) fn provider_supports_oauth(provider_id: &str) -> bool {
    provider_supports_device_oauth(provider_id)
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use navi_sdk::{AgentEvent, RuntimeEvent, RuntimeEventKind};

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
