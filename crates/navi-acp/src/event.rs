use crate::types::{RequestPermissionParams, SessionUpdate};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Typed events surfaced by the ACP client to navi.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AcpEvent {
    SessionUpdate {
        session_id: String,
        update: SessionUpdate,
    },
    PermissionRequired {
        request_id: Value,
        params: RequestPermissionParams,
    },
    /// Server issued an unknown client method request.
    ServerRequest {
        request_id: Value,
        method: String,
        params: Value,
    },
    /// Unrecognized notification.
    Notification {
        method: String,
        params: Value,
    },
    TransportClosed,
}

impl AcpEvent {
    pub fn agent_text(&self) -> Option<&str> {
        match self {
            Self::SessionUpdate { update, .. } => update.agent_text(),
            _ => None,
        }
    }
}
