//! Live MCP status probe for the TUI modal (same path as `navi mcp list`).

use navi_core::{McpConfig, McpServerConfig};

use crate::app::TuiApp;
use crate::dispatch::AsyncEvent;
use crate::runtime::spawn_runtime_task;
use crate::state::McpLiveServer;

/// Kick off a background probe of configured MCP servers.
///
/// Does not block the UI. Results land as [`AsyncEvent::McpStatusLoaded`].
pub(crate) fn refresh_mcp_status(app: &mut TuiApp) {
    if app.mcp_ui_state.loading {
        return;
    }
    app.mcp_ui_state.loading = true;
    app.mcp_ui_state.probe_error = None;

    let mcp = app.loaded_config.config.mcp.clone();
    let allowed = app
        .loaded_config
        .config
        .security
        .allowed_mcp_servers
        .clone();
    let tx = app.async_sender();

    spawn_runtime_task(async move {
        let result = probe_mcp_status(mcp, allowed).await;
        let _ = tx.send(AsyncEvent::McpStatusLoaded { result });
    });
}

async fn probe_mcp_status(
    mcp: McpConfig,
    allowed: Vec<String>,
) -> Result<Vec<McpLiveServer>, String> {
    if !mcp.enabled {
        return Ok(mcp
            .servers
            .iter()
            .map(|s| live_from_config(s, false, Vec::new()))
            .collect());
    }

    let loaded = navi_sdk::load_configured_mcp_servers(&mcp, &allowed).await;
    let statuses: Vec<McpLiveServer> = mcp
        .servers
        .iter()
        .map(|server| {
            let live = loaded.servers.iter().find(|c| c.id == server.id);
            live_from_config(
                server,
                live.is_some(),
                live.map(|l| l.tools.clone()).unwrap_or_default(),
            )
        })
        .collect();
    loaded.shutdown();
    Ok(statuses)
}

fn live_from_config(server: &McpServerConfig, connected: bool, tools: Vec<String>) -> McpLiveServer {
    McpLiveServer {
        id: server.id.clone(),
        enabled: server.enabled,
        connected: server.enabled && connected,
        tools,
        command: server.command.clone(),
        args: server.args.clone(),
        url: server.url.clone(),
    }
}

/// Seed live status from the session's already-connected MCP servers (instant).
/// A full probe may still upgrade this with a more complete tool list.
pub(crate) fn seed_from_session(app: &mut TuiApp) {
    if !app.mcp_ui_state.live.is_empty() {
        return;
    }
    let config_servers = &app.loaded_config.config.mcp.servers;
    let connected = app
        .engine()
        .list_mcp_servers(app.session_id.as_str())
        .unwrap_or_default();

    app.mcp_ui_state.live = config_servers
        .iter()
        .map(|server| {
            let live = connected.iter().find(|c| c.id == server.id);
            live_from_config(
                server,
                live.is_some(),
                live.map(|l| l.tools.clone()).unwrap_or_default(),
            )
        })
        .collect();
}
