use crate::client::{AcpClient, AcpConnectOptions, PermissionHandler};
use crate::error::Result;
use crate::event::AcpEvent;
use crate::process::AcpProcessConfig;
use crate::types::{PromptResult, StopReason};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Result of delegating a full turn to an external ACP agent.
#[derive(Debug, Clone)]
pub struct AcpTurnResult {
    pub agent_id: String,
    pub acp_session_id: String,
    pub stop_reason: StopReason,
    pub text: String,
    pub events: Vec<AcpEvent>,
}

/// Configuration for an external ACP agent peer (mirrors navi config).
#[derive(Debug, Clone)]
pub struct AcpAgentSpec {
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub api_key_env: Option<String>,
    pub api_key: Option<String>,
    pub auth_method_id: Option<String>,
    pub auto_approve_permissions: bool,
}

impl AcpAgentSpec {
    pub fn to_connect_options(&self) -> AcpConnectOptions {
        AcpConnectOptions {
            process: AcpProcessConfig {
                command: self.command.clone(),
                args: self.args.clone(),
                env: self.env.clone(),
                cwd: self.cwd.clone(),
            },
            client_name: "navi".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
            api_key_env: self.api_key_env.clone(),
            api_key: self.api_key.clone(),
            auth_method_id: self.auth_method_id.clone(),
            skip_auth: false,
            permission: if self.auto_approve_permissions {
                PermissionHandler::AutoApprove
            } else {
                PermissionHandler::AutoReject
            },
        }
    }
}

/// External agent peer: agent-level turn delegation (not inference).
#[async_trait]
pub trait ExternalAgentPeer: Send + Sync {
    /// Delegate a single user prompt to the peer and return the collected turn.
    async fn delegate_turn(&self, cwd: PathBuf, prompt: String) -> Result<AcpTurnResult>;
}

/// Concrete peer that spawns an ACP server for each turn.
pub struct SpawnedAcpPeer {
    pub spec: AcpAgentSpec,
}

#[async_trait]
impl ExternalAgentPeer for SpawnedAcpPeer {
    async fn delegate_turn(&self, cwd: PathBuf, prompt: String) -> Result<AcpTurnResult> {
        let mut client = AcpClient::connect(self.spec.to_connect_options()).await?;
        let session = client.session_new(&cwd).await?;
        let session_id = session.session_id;
        let (result, text) = client.prompt_collect_text(&session_id, &prompt).await?;
        let mut events = client.drain_events();
        // Also fold any already-seen updates is hard; surface drained post-prompt.
        let _ = PromptResult {
            stop_reason: result.stop_reason.clone(),
            extra: result.extra.clone(),
        };
        // Include synthetic marker event if no text chunks were drained post-wait.
        if text.is_empty() {
            // text was collected during wait; events may still be empty.
        }
        let _ = &mut events;
        Ok(AcpTurnResult {
            agent_id: self.spec.id.clone(),
            acp_session_id: session_id,
            stop_reason: result.stop_reason,
            text,
            events,
        })
    }
}

/// Map an [`AcpEvent`] into a short human-readable label (for logs/tests).
pub fn event_label(event: &AcpEvent) -> String {
    match event {
        AcpEvent::SessionUpdate { update, .. } => format!("session_update:{update:?}"),
        AcpEvent::PermissionRequired { .. } => "permission_required".into(),
        AcpEvent::ServerRequest { method, .. } => format!("server_request:{method}"),
        AcpEvent::Notification { method, .. } => format!("notification:{method}"),
        AcpEvent::TransportClosed => "transport_closed".into(),
    }
}
