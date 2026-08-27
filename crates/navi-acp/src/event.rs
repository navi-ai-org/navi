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
    /// A v2 `session/update` notification (ADR 0023).
    ///
    /// Only present when the `acp-v2` feature is enabled.
    #[cfg(feature = "acp-v2")]
    SessionUpdateV2 {
        session_id: String,
        update: crate::v2::SessionUpdate,
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

    /// Returns the v2 stop reason when this event is the completion `state_update`
    /// transition to `idle`.
    #[cfg(feature = "acp-v2")]
    pub fn v2_idle_stop_reason(&self) -> Option<&crate::v2::StopReason> {
        use crate::v2::StateUpdate;
        match self {
            Self::SessionUpdateV2 {
                update: crate::v2::SessionUpdate::StateUpdate(StateUpdate::Idle(idle)),
                ..
            } => idle.stop_reason.as_ref(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "acp-v2")]
    #[test]
    fn acp_event_v2_idle_stop_reason_round_trips() {
        use crate::v2::{IdleStateUpdate, StateUpdate, StopReason};

        let update = crate::v2::SessionUpdate::StateUpdate(StateUpdate::Idle(
            IdleStateUpdate::new().stop_reason(StopReason::EndTurn),
        ));
        let event = AcpEvent::SessionUpdateV2 {
            session_id: "s1".into(),
            update,
        };

        // serde round-trip preserves the discriminated tag + state + stopReason.
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["kind"], "session_update_v2");
        let decoded: AcpEvent = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.v2_idle_stop_reason(), Some(&StopReason::EndTurn));
    }
}
