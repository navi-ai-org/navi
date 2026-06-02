use std::time::Instant;

use navi_sdk::{
    ModelOption, NaviConfigSaveTarget, NaviModelSelectionRequest, NaviProviderCredentialStatus,
    ProviderConfig, canonical_provider_id, is_free_model_name, model_can_run_publicly,
    resolve_provider_config, start_provider_device_oauth,
};

use crate::chat::refresh_system_context;
use crate::dispatch::AsyncEvent;
use crate::keybindings::{close_active_modal, close_all_modals};
use crate::runtime::{build_engine, provider_supports_oauth, selected_model_runtime_available};
use crate::ui::list::SelectListState;
use crate::{
    TuiApp,
    notifications::{push_diagnostic, show_notification},
};

pub(crate) fn rebuild_provider(app: &mut TuiApp) {
    match build_engine(&app.loaded_config, app.project_dir.clone()) {
        Ok(engine) => app.set_engine(engine),
        Err(err) => push_diagnostic(app, format!("Failed to rebuild runtime engine: {err:#}")),
    }
    app.provider_configured =
        selected_model_runtime_available(&app.loaded_config, app.credential_store());
    app.refresh_harness_policy();
    app.compact_state.context_window =
        navi_sdk::effective_context_window(&app.loaded_config.config);
    refresh_system_context(app);
    tracing::info!(
        provider = %app.loaded_config.config.model.provider,
        model = %app.loaded_config.config.model.name,
        "provider rebuilt"
    );
}

pub(crate) fn provider_has_api_key(app: &TuiApp, provider_id: &str) -> bool {
    app.engine()
        .credential_status(provider_id)
        .map(|status| status.configured)
        .unwrap_or(false)
}

pub(crate) fn model_is_available_for_selection(app: &TuiApp, model: &ModelOption) -> bool {
    provider_has_api_key(app, &model.provider_id)
        || model_can_run_publicly(&model.provider_id, &model.name)
}

pub(crate) fn apply_model_selection(app: &mut TuiApp, model_index: usize) {
    let Some(model) = app.models.get(model_index).cloned() else {
        return;
    };

    let result = app.engine().select_model(NaviModelSelectionRequest {
        provider_id: model.provider_id.clone(),
        model: model.name.clone(),
        save_target: NaviConfigSaveTarget::Auto,
    });

    match result {
        Ok(selection) => {
            app.loaded_config = selection.loaded_config;
            app.provider_configured = selection.provider_configured;
            app.selected_model = model_index;
            app.model_scroll = 0;
            if navi_sdk::ProviderId::from_config_id(&model.provider_id).is_opencode_family()
                && is_free_model_name(&model.name)
            {
                show_notification(
                    app,
                    "OpenCode Zen",
                    "Free model selected. NAVI will use your Zen key when configured.",
                );
            }
            rebuild_provider(app);
        }
        Err(err) => show_notification(app, "Model", format!("Failed to select model: {err:#}")),
    }
}

pub(crate) fn selected_or_pending_provider_id(app: &TuiApp) -> String {
    app.pending_provider_setup.clone().unwrap_or_else(|| {
        app.pending_model_selection
            .and_then(|index| app.models.get(index))
            .map(|model| model.provider_id.clone())
            .unwrap_or_else(|| app.loaded_config.config.model.provider.clone())
    })
}

pub(crate) fn selected_or_pending_provider_label(app: &TuiApp) -> String {
    if let Some(provider_id) = &app.pending_provider_setup {
        return resolve_provider_config(&app.loaded_config.config, provider_id)
            .map(|provider| provider.label)
            .unwrap_or_else(|| provider_id.clone());
    }

    app.pending_model_selection
        .and_then(|index| app.models.get(index))
        .map(|model| model.provider_label.clone())
        .unwrap_or_else(|| selected_provider_label(app).to_string())
}

pub(crate) fn save_api_key_and_rebuild(app: &mut TuiApp) {
    let key = app.api_key_input.trim().to_string();
    if key.is_empty() {
        return;
    }

    let provider_id = selected_or_pending_provider_id(app);
    if let Err(err) = app.engine().set_provider_api_key(&provider_id, &key) {
        show_notification(app, "Credentials", format!("Failed to save key: {err:#}"));
    } else {
        show_notification(
            app,
            "Credentials",
            format!("API key saved for provider \"{provider_id}\"."),
        );
    }

    let return_to_providers = app.pending_provider_setup.take().is_some();
    if let Some(model_index) = app.pending_model_selection.take() {
        apply_model_selection(app, model_index);
    } else {
        rebuild_provider(app);
    }
    app.api_key_input.clear();
    app.api_key_cursor = 0;
    if return_to_providers {
        close_active_modal(app);
    } else {
        close_all_modals(app);
    }
}

pub(crate) fn current_provider_env_var(app: &TuiApp) -> String {
    let provider_id = selected_or_pending_provider_id(app);
    resolve_provider_config(&app.loaded_config.config, &provider_id)
        .map(|p| p.api_key_env.clone())
        .unwrap_or_else(|| "API_KEY".to_string())
}

pub(crate) fn current_provider_credential_status(app: &TuiApp) -> String {
    let provider_id = selected_or_pending_provider_id(app);
    match app.engine().credential_status(&provider_id) {
        Ok(status) => status.detail.unwrap_or(status.label),
        Err(_) => "unknown provider".to_string(),
    }
}

pub(crate) struct ProviderAuthStatus {
    pub(crate) configured: bool,
    pub(crate) label: String,
}

pub(crate) fn provider_auth_status(
    app: &TuiApp,
    provider_config: &ProviderConfig,
) -> ProviderAuthStatus {
    let status = app
        .engine()
        .credential_status(&provider_config.id)
        .unwrap_or(NaviProviderCredentialStatus {
            provider_id: provider_config.id.clone(),
            configured: false,
            source: None,
            label: "not configured".to_string(),
            detail: None,
            env_var: provider_config.api_key_env.clone(),
            credential_store_path: app.credential_store().path().to_path_buf(),
        });
    ProviderAuthStatus {
        configured: status.configured,
        label: status.label,
    }
}

pub(crate) fn start_provider_oauth(app: &mut TuiApp, provider: &ProviderConfig) {
    if !provider_supports_oauth(&provider.id) {
        show_notification(
            app,
            "OAuth",
            format!("{} uses API key setup.", provider.label),
        );
        return;
    }
    if app.is_loading {
        return;
    }

    app.is_loading = true;
    app.loading_start = Some(Instant::now());
    let tx = app.async_sender();
    let credential_store = app.credential_store_clone();
    let provider_id = provider.id.clone();
    app.set_stream_task(tokio::spawn(async move {
        let result = start_provider_device_oauth(&credential_store, &provider_id, |started| {
            let _ = tx.send(AsyncEvent::OAuthDeviceStarted {
                provider_id: provider_id.clone(),
                verification_uri: started.verification_uri,
                user_code: started.user_code,
            });
        })
        .await
        .map_err(|err| err.to_string());
        let _ = tx.send(AsyncEvent::OAuthCompleted {
            provider_id,
            result,
        });
    }));
}

pub(crate) fn selected_provider_label(app: &TuiApp) -> &str {
    let current_provider = canonical_provider_id(&app.loaded_config.config.model.provider);
    app.models
        .iter()
        .find(|model| canonical_provider_id(&model.provider_id) == current_provider)
        .map(|model| model.provider_label.as_str())
        .unwrap_or(app.loaded_config.config.model.provider.as_str())
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // `description` and `provider_id` are used in tests only
pub(crate) enum ListRow {
    Header {
        label: String,
        description: String,
        provider_id: String,
    },
    Model {
        index: usize,
    },
}

pub(crate) fn build_model_rows(app: &TuiApp) -> Vec<ListRow> {
    let filter = app.model_filter.trim().to_lowercase();

    let mut rows = Vec::new();
    let mut current_provider: Option<&str> = None;

    for (index, model) in app.models.iter().enumerate() {
        if !filter.is_empty()
            && !model.name.to_lowercase().contains(&filter)
            && !model.provider_id.to_lowercase().contains(&filter)
            && !model.provider_label.to_lowercase().contains(&filter)
            && !model.provider_description.to_lowercase().contains(&filter)
        {
            continue;
        }
        if current_provider != Some(model.provider_label.as_str()) {
            current_provider = Some(model.provider_label.as_str());
            rows.push(ListRow::Header {
                label: model.provider_label.clone(),
                description: model.provider_description.clone(),
                provider_id: model.provider_id.clone(),
            });
        }
        rows.push(ListRow::Model { index });
    }

    rows
}

pub(crate) fn first_model_index(rows: &[ListRow]) -> Option<usize> {
    rows.iter().find_map(|row| match row {
        ListRow::Model { index } => Some(*index),
        ListRow::Header { .. } => None,
    })
}

pub(crate) fn selected_model_in_rows(rows: &[ListRow], selected_model: usize) -> Option<usize> {
    rows.iter().position(|row| match row {
        ListRow::Model { index } => *index == selected_model,
        ListRow::Header { .. } => false,
    })
}

pub(crate) fn next_model_index(app: &TuiApp, rows: &[ListRow]) -> usize {
    let Some(current) = selected_model_in_rows(rows, app.selected_model) else {
        return rows
            .iter()
            .find_map(|row| match row {
                ListRow::Model { index } => Some(*index),
                _ => None,
            })
            .unwrap_or(app.selected_model);
    };

    rows.iter()
        .skip(current + 1)
        .find_map(|row| match row {
            ListRow::Model { index } => Some(*index),
            _ => None,
        })
        .unwrap_or(app.selected_model)
}

pub(crate) fn previous_model_index(app: &TuiApp, rows: &[ListRow]) -> usize {
    let Some(current) = selected_model_in_rows(rows, app.selected_model) else {
        return rows
            .iter()
            .find_map(|row| match row {
                ListRow::Model { index } => Some(*index),
                _ => None,
            })
            .unwrap_or(app.selected_model);
    };

    rows.iter()
        .take(current)
        .rev()
        .find_map(|row| match row {
            ListRow::Model { index } => Some(*index),
            _ => None,
        })
        .unwrap_or(app.selected_model)
}

pub(crate) fn sync_scroll_to_selection(app: &mut TuiApp, rows: &[ListRow], visible_rows: u16) {
    let Some(selected_row) = selected_model_in_rows(rows, app.selected_model) else {
        return;
    };

    let visible_rows = usize::from(visible_rows).max(1);
    let mut state = SelectListState::new(selected_row, app.model_scroll);
    state.sync_scroll_with_context(visible_rows, 4);
    state.clamp_scroll(rows.len(), visible_rows);
    app.model_scroll = state.scroll();
}
