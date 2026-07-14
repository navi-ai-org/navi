//! Live MCP status probe for the TUI modal (same path as `navi mcp list`).

use navi_core::{McpConfig, McpServerConfig};

use crate::app::TuiApp;
use crate::dispatch::AsyncEvent;
use crate::runtime::spawn_runtime_task;
use crate::state::{ModalKind, McpLiveServer};

/// Open the MCP modal and kick off status (session seed + background probe).
///
/// Always use this instead of only `replace_modal(Mcp)` so the list never
/// flashes every server as red "failed" before the first probe finishes.
pub(crate) fn open_mcp_modal(app: &mut TuiApp) {
    app.mcp_ui_state.selected_server = 0;
    app.mcp_ui_state.selected_tool = 0;
    app.mcp_ui_state.scroll = 0;
    app.mcp_ui_state.is_focused_on_tools = false;
    crate::keybindings::replace_modal(app, ModalKind::Mcp);
    seed_from_session(app);
    refresh_mcp_status(app);
}

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
            .map(|s| live_from_config(s, /*connected*/ false, /*known*/ true, Vec::new()))
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
                /*known*/ true,
                live.map(|l| l.tools.clone()).unwrap_or_default(),
            )
        })
        .collect();
    loaded.shutdown();
    Ok(statuses)
}

fn live_from_config(
    server: &McpServerConfig,
    connected: bool,
    known: bool,
    tools: Vec<String>,
) -> McpLiveServer {
    McpLiveServer {
        id: server.id.clone(),
        enabled: server.enabled,
        connected: server.enabled && connected,
        known: if !server.enabled { true } else { known },
        tools,
        command: server.command.clone(),
        args: server.args.clone(),
        url: server.url.clone(),
    }
}

/// Seed live status from the session's already-connected MCP servers (instant).
///
/// - Session has MCP → mark those connected immediately (no red flash).
/// - Session missing / empty → leave `known: false` so UI shows "checking…"
///   until the background probe finishes (not "failed").
pub(crate) fn seed_from_session(app: &mut TuiApp) {
    let config_servers = app.loaded_config.config.mcp.servers.clone();
    if config_servers.is_empty() {
        app.mcp_ui_state.live.clear();
        return;
    }

    let session_mcp = app
        .engine()
        .list_mcp_servers(app.session_id.as_str())
        .unwrap_or_default();

    // Prefer session truth when available; otherwise keep existing known=true
    // rows (e.g. last successful probe) and only fill gaps as unknown.
    let had_known = app.mcp_ui_state.live.iter().any(|s| s.known && s.connected);

    if !session_mcp.is_empty() {
        app.mcp_ui_state.live = config_servers
            .iter()
            .map(|server| {
                let live = session_mcp.iter().find(|c| c.id == server.id);
                live_from_config(
                    server,
                    live.is_some(),
                    /*known*/ live.is_some() || !server.enabled,
                    live.map(|l| l.tools.clone()).unwrap_or_default(),
                )
            })
            .collect();
        return;
    }

    if had_known {
        // Keep last good probe; don't wipe to unknown/failed on reopen.
        // Still refresh in background via open_mcp_modal → refresh_mcp_status.
        return;
    }

    // No session MCP yet (engine rebuild / pre-first-turn): unknown, not failed.
    app.mcp_ui_state.live = config_servers
        .iter()
        .map(|server| live_from_config(server, false, /*known*/ false, Vec::new()))
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use navi_core::McpServerConfig;

    fn sample_server(id: &str, enabled: bool) -> McpServerConfig {
        McpServerConfig {
            id: id.into(),
            enabled,
            command: Some("echo".into()),
            args: vec![],
            url: None,
            env: Default::default(),
            cwd: None,
            timeout_ms: None,
            tool_prefix: None,
        }
    }

    #[test]
    fn unknown_enabled_server_is_not_marked_connected() {
        let s = sample_server("better-search", true);
        let live = live_from_config(&s, false, false, Vec::new());
        assert!(live.enabled);
        assert!(!live.connected);
        assert!(!live.known, "must render as checking, not failed");
    }

    #[test]
    fn probe_result_is_known() {
        let s = sample_server("discord", true);
        let live = live_from_config(&s, true, true, vec!["send".into()]);
        assert!(live.connected && live.known);
        assert_eq!(live.tools, vec!["send"]);
    }
}
