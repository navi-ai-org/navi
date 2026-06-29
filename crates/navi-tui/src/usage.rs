use crate::TuiApp;
use crate::dispatch::AsyncEvent;
use crate::runtime::spawn_runtime_task;
use crate::state::ModalKind;

pub(crate) fn open_usage_modal(app: &mut TuiApp) {
    crate::keybindings::replace_modal(app, ModalKind::Usage);
    refresh_usage(app);
}

pub(crate) fn refresh_usage(app: &mut TuiApp) {
    app.usage_state.loading = true;
    app.usage_state.error = None;
    let engine = app.engine();
    let tx = app.async_sender();
    spawn_runtime_task(async move {
        let result = engine.usage_report().await.map_err(|err| err.to_string());
        let _ = tx.send(AsyncEvent::UsageLoaded { result });
    });
}
