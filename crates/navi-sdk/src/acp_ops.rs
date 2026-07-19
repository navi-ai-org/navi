//! ACP external agent peer operations on [`NaviEngine`].
//!
//! ACP agents are **not** model providers. See `navi-acp/DESIGN.md`.

use navi_acp::{
    AcpAgentSpec, AcpClient, AcpConnectOptions, AcpEvent, AcpProcessConfig, AcpTurnResult,
    ExternalAgentPeer, PermissionHandler, SessionUpdate, SpawnedAcpPeer, StopReason,
};
use navi_core::{AcpAgentConfig, RuntimeEvent, RuntimeEventKind};
use serde_json::json;
use std::path::PathBuf;

use crate::engine::NaviEngine;
use crate::types::NaviError;

type Result<T> = std::result::Result<T, NaviError>;

/// Summary of a configured ACP agent for listing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviAcpAgentInfo {
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    pub enabled: bool,
    pub api_key_env: Option<String>,
    pub auth_method_id: Option<String>,
    pub auto_approve_permissions: bool,
}

/// Request to delegate a full turn to an external ACP agent.
#[derive(Debug, Clone)]
pub struct NaviAcpTurnRequest {
    pub agent_id: String,
    pub prompt: String,
    pub cwd: Option<PathBuf>,
    /// Optional navi session id (used for logging correlation in v1).
    pub session_id: Option<String>,
}

/// Response from an ACP delegated turn.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaviAcpTurnResponse {
    pub agent_id: String,
    pub acp_session_id: String,
    pub stop_reason: String,
    pub text: String,
    /// Runtime events mapped from ACP updates (for callers that do not subscribe).
    pub events: Vec<RuntimeEvent>,
}

fn agent_config_to_spec(cfg: &AcpAgentConfig) -> AcpAgentSpec {
    AcpAgentSpec {
        id: cfg.id.clone(),
        command: cfg.command.clone(),
        args: cfg.args.clone(),
        env: cfg.env.clone(),
        cwd: cfg.cwd.clone(),
        api_key_env: cfg.api_key_env.clone(),
        api_key: None,
        auth_method_id: cfg.auth_method_id.clone(),
        auto_approve_permissions: cfg.auto_approve_permissions,
    }
}

fn stop_reason_label(reason: &StopReason) -> String {
    match reason {
        StopReason::EndTurn => "end_turn".into(),
        StopReason::MaxTokens => "max_tokens".into(),
        StopReason::MaxTurnRequests => "max_turn_requests".into(),
        StopReason::Refusal => "refusal".into(),
        StopReason::Cancelled => "cancelled".into(),
        StopReason::Other => "other".into(),
    }
}

fn update_kind(update: &SessionUpdate) -> &'static str {
    match update {
        SessionUpdate::AgentMessageChunk { .. } => "agent_message_chunk",
        SessionUpdate::AgentThoughtChunk { .. } => "agent_thought_chunk",
        SessionUpdate::UserMessageChunk { .. } => "user_message_chunk",
        SessionUpdate::ToolCall { .. } => "tool_call",
        SessionUpdate::ToolCallUpdate { .. } => "tool_call_update",
        SessionUpdate::Plan { .. } => "plan",
        SessionUpdate::UsageUpdate { .. } => "usage_update",
        SessionUpdate::Other => "other",
    }
}

/// Map an ACP session update into navi runtime events (shared bus surface).
pub fn map_acp_update_to_runtime_events(
    agent_id: &str,
    acp_session_id: &str,
    update: &SessionUpdate,
) -> Vec<RuntimeEvent> {
    let mut events = Vec::new();
    let payload = serde_json::to_value(update).unwrap_or(json!({}));
    events.push(RuntimeEvent::new(RuntimeEventKind::AcpPeerUpdate {
        agent_id: agent_id.to_string(),
        acp_session_id: acp_session_id.to_string(),
        update_kind: update_kind(update).to_string(),
        update: payload,
    }));

    if let Some(text) = update.agent_text() {
        events.push(RuntimeEvent::new(RuntimeEventKind::AssistantDelta {
            text: text.to_string(),
        }));
    }
    if let Some(text) = update.thought_text() {
        events.push(RuntimeEvent::new(
            RuntimeEventKind::AssistantThinkingDelta {
                text: text.to_string(),
            },
        ));
    }
    if let SessionUpdate::UsageUpdate { used, .. } = update {
        events.push(RuntimeEvent::new(RuntimeEventKind::TokensUpdated {
            input_tokens: *used,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        }));
    }
    events
}

impl NaviEngine {
    /// Lists configured ACP agents from config (not model providers).
    pub fn list_acp_agents(&self) -> Vec<NaviAcpAgentInfo> {
        let loaded = self.loaded_config();
        if !loaded.config.acp.enabled {
            return Vec::new();
        }
        loaded
            .config
            .acp_agents
            .iter()
            .filter(|a| a.enabled)
            .map(|a| NaviAcpAgentInfo {
                id: a.id.clone(),
                command: a.command.clone(),
                args: a.args.clone(),
                enabled: a.enabled,
                api_key_env: a.api_key_env.clone(),
                auth_method_id: a.auth_method_id.clone(),
                auto_approve_permissions: a.auto_approve_permissions,
            })
            .collect()
    }

    /// Delegates a full turn to an external ACP agent peer.
    ///
    /// This is **not** a `ModelProvider` call. The external agent runs its own
    /// harness; mapped runtime events are returned on the response for the
    /// caller to forward to UI / session subscribers.
    pub async fn delegate_acp_turn(
        &self,
        request: NaviAcpTurnRequest,
    ) -> Result<NaviAcpTurnResponse> {
        let loaded = self.loaded_config();
        if !loaded.config.acp.enabled {
            return Err(NaviError::from(anyhow::anyhow!(
                "ACP integration is disabled (set [acp] enabled = true)"
            )));
        }
        let agent_cfg = loaded
            .config
            .acp_agents
            .iter()
            .find(|a| a.enabled && a.id == request.agent_id)
            .cloned()
            .ok_or_else(|| {
                NaviError::from(anyhow::anyhow!(
                    "ACP agent `{}` not found or disabled",
                    request.agent_id
                ))
            })?;

        let cwd = request
            .cwd
            .clone()
            .or(agent_cfg.cwd.clone())
            .unwrap_or_else(|| self.inner.project_dir.clone());

        let permission = if agent_cfg.auto_approve_permissions {
            PermissionHandler::AutoApprove
        } else {
            PermissionHandler::AutoReject
        };

        let opts = AcpConnectOptions {
            process: AcpProcessConfig {
                command: agent_cfg.command.clone(),
                args: agent_cfg.args.clone(),
                env: agent_cfg.env.clone(),
                cwd: Some(cwd.clone()),
            },
            client_name: "navi".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
            api_key_env: agent_cfg.api_key_env.clone(),
            api_key: None,
            auth_method_id: agent_cfg.auth_method_id.clone(),
            skip_auth: false,
            permission,
        };

        let mut client = AcpClient::connect(opts)
            .await
            .map_err(|e| NaviError::from(anyhow::anyhow!("ACP connect failed: {e}")))?;

        let session = client
            .session_new(&cwd)
            .await
            .map_err(|e| NaviError::from(anyhow::anyhow!("ACP session/new failed: {e}")))?;
        let acp_session_id = session.session_id;
        let agent_id = agent_cfg.id.clone();

        let (result, text) = client
            .prompt_collect_text(&acp_session_id, &request.prompt)
            .await
            .map_err(|e| NaviError::from(anyhow::anyhow!("ACP session/prompt failed: {e}")))?;

        let mut events = Vec::new();
        for ev in client.drain_events() {
            if let AcpEvent::SessionUpdate { session_id, update } = &ev {
                events.extend(map_acp_update_to_runtime_events(
                    &agent_id, session_id, update,
                ));
            }
        }

        if !text.is_empty()
            && !events
                .iter()
                .any(|e| matches!(e.kind, RuntimeEventKind::AssistantDelta { .. }))
        {
            events.push(RuntimeEvent::new(RuntimeEventKind::AssistantDelta {
                text: text.clone(),
            }));
        }

        if let Some(sid) = &request.session_id {
            tracing::debug!(
                session = %sid,
                agent = %agent_id,
                acp_session = %acp_session_id,
                event_count = events.len(),
                "ACP turn completed"
            );
        }

        Ok(NaviAcpTurnResponse {
            agent_id,
            acp_session_id,
            stop_reason: stop_reason_label(&result.stop_reason),
            text,
            events,
        })
    }

    /// Convenience: resolve agent by id and run [`SpawnedAcpPeer::delegate_turn`].
    pub async fn delegate_acp_turn_simple(
        &self,
        agent_id: &str,
        prompt: impl Into<String>,
    ) -> Result<AcpTurnResult> {
        let loaded = self.loaded_config();
        if !loaded.config.acp.enabled {
            return Err(NaviError::from(anyhow::anyhow!("ACP integration disabled")));
        }
        let agent_cfg = loaded
            .config
            .acp_agents
            .iter()
            .find(|a| a.enabled && a.id == agent_id)
            .cloned()
            .ok_or_else(|| NaviError::from(anyhow::anyhow!("ACP agent `{agent_id}` not found")))?;
        let peer = SpawnedAcpPeer {
            spec: agent_config_to_spec(&agent_cfg),
        };
        let cwd = agent_cfg
            .cwd
            .clone()
            .unwrap_or_else(|| self.inner.project_dir.clone());
        peer.delegate_turn(cwd, prompt.into()).await.map_err(|e| {
            NaviError::from(anyhow::anyhow!(
                "ACP agent `{agent_id}` delegate_turn failed: {e}"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use navi_acp::ContentBlock;

    #[test]
    fn maps_agent_message_to_assistant_delta() {
        let update = SessionUpdate::AgentMessageChunk {
            message_id: None,
            content: ContentBlock::text("hi"),
        };
        let events = map_acp_update_to_runtime_events("devin", "s1", &update);
        assert!(events.iter().any(|e| {
            matches!(
                e.kind,
                RuntimeEventKind::AssistantDelta { ref text } if text == "hi"
            )
        }));
        assert!(
            events
                .iter()
                .any(|e| matches!(e.kind, RuntimeEventKind::AcpPeerUpdate { .. }))
        );
    }

    #[test]
    fn integration_surface_is_not_model_provider() {
        let peer_name = std::any::type_name::<SpawnedAcpPeer>();
        let provider_name = std::any::type_name::<dyn navi_core::ModelProvider>();
        assert!(peer_name.contains("SpawnedAcpPeer"));
        assert!(!peer_name.contains("ModelProvider"));
        assert!(provider_name.contains("ModelProvider"));
        let _ = std::any::type_name::<dyn ExternalAgentPeer>();
    }
}
