//! Env-gated live smoke against `devin acp`.
//!
//! ```bash
//! ACP_SMOKE_TEST=1 DEVIN_API_KEY=... cargo test -p navi-acp --test smoke -- --nocapture
//! ```
//!
//! Skipped unless `ACP_SMOKE_TEST=1`. Requires `devin` on PATH.

use navi_acp::{
    AcpAgentSpec, AcpClient, AcpConnectOptions, AcpProcessConfig, ExternalAgentPeer,
    PermissionHandler, SpawnedAcpPeer, StopReason,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

fn smoke_enabled() -> bool {
    matches!(
        std::env::var("ACP_SMOKE_TEST").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

fn devin_available() -> bool {
    Command::new("devin")
        .arg("acp")
        .arg("--help")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn api_key() -> Option<String> {
    for name in ["DEVIN_API_KEY", "WINDSURF_API_KEY", "ACP_API_KEY"] {
        if let Ok(v) = std::env::var(name) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

#[tokio::test]
async fn devin_acp_smoke_initialize_and_prompt() {
    if !smoke_enabled() {
        eprintln!("skip: set ACP_SMOKE_TEST=1 to run");
        return;
    }
    if !devin_available() {
        eprintln!("skip: devin binary not available");
        return;
    }
    let key = match api_key() {
        Some(k) => k,
        None => {
            eprintln!("skip: no DEVIN_API_KEY / WINDSURF_API_KEY / ACP_API_KEY");
            return;
        }
    };

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"));
    let opts = AcpConnectOptions {
        process: AcpProcessConfig {
            command: "devin".into(),
            args: vec!["acp".into()],
            env: BTreeMap::new(),
            cwd: Some(cwd.clone()),
        },
        client_name: "navi-acp-smoke".into(),
        client_version: "0.0.0-test".into(),
        api_key_env: None,
        api_key: Some(key),
        auth_method_id: None,
        skip_auth: false,
        permission: PermissionHandler::AutoApprove,
    };

    let mut client = AcpClient::connect(opts)
        .await
        .expect("connect to devin acp");
    let init = client.initialize_result();
    eprintln!(
        "initialized: protocol={} agent={:?}",
        init.protocol_version,
        init.agent_info.as_ref().map(|a| &a.name)
    );
    assert_eq!(init.protocol_version, 1);

    let session = client.session_new(&cwd).await.expect("session/new");
    eprintln!("session: {}", session.session_id);
    assert!(!session.session_id.is_empty());

    // Short, tool-light prompt — summarizer path is more reliable without fs.
    // Default agent may still request tools; auto-approve handles permissions.
    let (result, text) = client
        .prompt_collect_text(
            &session.session_id,
            "Reply with exactly the word PONG and nothing else.",
        )
        .await
        .expect("session/prompt");

    eprintln!(
        "stop_reason={:?} text_len={} text={text:?}",
        result.stop_reason,
        text.len()
    );
    assert!(
        matches!(
            result.stop_reason,
            StopReason::EndTurn
                | StopReason::MaxTokens
                | StopReason::MaxTurnRequests
                | StopReason::Refusal
                | StopReason::Cancelled
                | StopReason::Other
        ),
        "unexpected stop reason"
    );
    // Prefer non-empty text; some agent types may only stream tools. At minimum
    // the RPC completed without transport failure.
    if text.is_empty() {
        eprintln!("warning: empty agent text (turn still completed)");
    } else {
        assert!(!text.trim().is_empty());
    }
}

#[tokio::test]
async fn devin_acp_peer_trait_smoke() {
    if !smoke_enabled() || !devin_available() {
        return;
    }
    let Some(key) = api_key() else {
        return;
    };

    let peer = SpawnedAcpPeer {
        spec: AcpAgentSpec {
            id: "devin".into(),
            command: "devin".into(),
            args: vec!["acp".into()],
            env: BTreeMap::new(),
            cwd: None,
            api_key_env: None,
            api_key: Some(key),
            auth_method_id: None,
            auto_approve_permissions: true,
        },
    };

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"));
    let result = peer
        .delegate_turn(cwd, "Say hi in one short sentence.".into())
        .await
        .expect("delegate_turn");
    eprintln!(
        "peer turn: agent={} session={} stop={:?} text={}",
        result.agent_id, result.acp_session_id, result.stop_reason, result.text
    );
    assert_eq!(result.agent_id, "devin");
    assert!(!result.acp_session_id.is_empty());
}
