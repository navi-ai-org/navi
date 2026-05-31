use agent_client_protocol::{Client, ConnectionTo};
use anyhow::Result;
use navi_sdk::{NaviEngine, NaviTurnRequest, RuntimeEventKind};

use crate::acp::events::{forward_runtime_event, send_text_update};
use crate::acp::permissions::request_permission;

pub(crate) async fn run_prompt_task(
    engine: NaviEngine,
    acp_session_id: String,
    prompt: String,
    connection: ConnectionTo<Client>,
) -> Result<()> {
    let mut events = engine.subscribe_events(&acp_session_id)?;
    let turn = engine.send_turn(NaviTurnRequest {
        session_id: acp_session_id.clone(),
        message: prompt,
        context_packets: Vec::new(),
    });
    tokio::pin!(turn);

    let mut saw_text_delta = false;
    let final_text = loop {
        tokio::select! {
            response = &mut turn => {
                break response?.text;
            }
            event = events.recv() => {
                let Ok(event) = event else { continue };
                saw_text_delta |= forward_runtime_event(&connection, &acp_session_id, &event)?;
                if let RuntimeEventKind::ApprovalRequired(request) = &event.kind {
                    let decision = request_permission(&connection, &acp_session_id, request).await?;
                    engine.resolve_approval(&acp_session_id, decision).await?;
                }
            }
        }
    };

    while let Ok(event) = events.try_recv() {
        saw_text_delta |= forward_runtime_event(&connection, &acp_session_id, &event)?;
    }

    if !saw_text_delta && !final_text.trim().is_empty() {
        send_text_update(&connection, &acp_session_id, final_text, false)?;
    }

    engine.snapshot_session(&acp_session_id).await?;

    Ok(())
}
