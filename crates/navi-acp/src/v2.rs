//! ACP v2 draft surface (ADR 0023).
//!
//! Gated behind the `acp-v2` cargo feature. Re-exports the stable-baseline v2
//! schema types from the official `agent-client-protocol` crate and provides
//! bridge helpers between v1 and v2 at the `navi-acp` boundary.
//!
//! Only the stable v2 baseline (`schema/v2/schema.json`) is exposed here.
//! Unstable v2 features (MCP-over-ACP, elicitation, NES, etc.) are each behind
//! their own flag on the upstream crate and are not enabled by `acp-v2`.

use crate::types::{
    AgentCapabilities as V1AgentCapabilities, AuthMethod as V1AuthMethod,
    ContentBlock as V1ContentBlock, ImplementationInfo as V1ImplementationInfo,
    InitializeParams as V1InitializeParams, InitializeResult as V1InitializeResult,
    NewSessionParams as V1NewSessionParams, NewSessionResult as V1NewSessionResult,
    PROTOCOL_VERSION, PromptParams as V1PromptParams, PromptResult as V1PromptResult,
    StopReason as V1StopReason,
};

// Re-export the v2 schema types from the official crate.
pub use agent_client_protocol::schema::ProtocolVersion as CrateProtocolVersion;
pub use agent_client_protocol::schema::v2::{
    AbsolutePath, AgentCapabilities, AgentMessage, AgentThought, AuthMethod, AuthMethodId,
    ClientCapabilities, CloseSessionRequest, CloseSessionResponse, ContentBlock, ContentChunk,
    IdleStateUpdate, Implementation, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoginAuthRequest, LoginAuthResponse, LogoutAuthRequest,
    LogoutAuthResponse, MessageId, Meta, NewSessionRequest, NewSessionResponse, OtherSessionUpdate,
    OtherStateUpdate, PlanEntry, PlanEntryPriority, PlanEntryStatus, PlanId, PlanItems, PlanUpdate,
    PlanUpdateContent, PromptRequest, PromptResponse, ReplayFrom, RequiresActionStateUpdate,
    ResumeSessionRequest, ResumeSessionResponse, RunningStateUpdate, SessionCapabilities,
    SessionId, SessionInfo, SessionListCursor, SessionUpdate, StateUpdate, StopReason,
    TerminalExitStatus, TerminalId, TerminalOutput, TerminalOutputChunk, TerminalUpdate,
    ToolCallContentChunk, ToolCallId, ToolCallUpdate, UpdateSessionNotification, UsageUpdate,
};
// Supporting types needed for v2 event projection (ADR 0023 Phase 4).
pub use agent_client_protocol::schema::MaybeUndefined;
pub use agent_client_protocol::schema::v2::{TextContent, ToolCallStatus, ToolKind};

/// v2 protocol version constant (wire value `2`).
pub const PROTOCOL_VERSION_V2: u32 = 2;

/// Maximum protocol version NAVI advertises when `acp-v2` is enabled.
pub const MAX_PROTOCOL_VERSION: u32 = PROTOCOL_VERSION_V2;

// ---------------------------------------------------------------------------
// NAVI App Profile (_meta["navi.dev"].app) — ADR 0018 §5
// ---------------------------------------------------------------------------

use serde::{Deserialize, Serialize};

/// Storage scoping settings in the app profile (ADR 0018 §5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppStorageSpec {
    /// App namespace for scoped storage. Defaults to the app `id` if absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Whether to share user credentials (default: `true`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_credentials: Option<bool>,
}

/// Tool filter policy in the app profile (ADR 0018 §5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppToolFilter {
    /// Tool profile name (e.g. `app_only`, `code_agent`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Tool allowlist (applied after profile).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    /// Tool denylist (applied after allow).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
}

/// Model selection preference in the app profile (ADR 0018 §5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppModelSpec {
    /// Provider id (e.g. `openai`, `anthropic`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Model name (e.g. `gpt-4o`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether the user may override this model selection (default: `true`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_overridable: Option<bool>,
}

/// Harness specification: either a reference to an installed pack or an inline pack.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum HarnessSpec {
    /// Reference to an installed harness pack: `{ "ref": "organize-anything/install" }`.
    Ref(String),
    /// Inline harness pack definition.
    Inline(serde_json::Value),
}

/// Prompt override specification (ADR 0018 §5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppPromptSpec {
    /// The system prompt text to inject.
    pub text: String,
}

/// Application profile requested by an ACP client in `initialize.params._meta["navi.dev"].app` (ADR 0018 §5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppProfileRequest {
    /// Unique application identifier (e.g. `organize-anything`, `tutor`).
    pub id: String,
    /// Application version string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Semver range constraint for required engine version (e.g. `>=1.2 <2.0`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_engine: Option<String>,
    /// Optional harness specification or reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<HarnessSpec>,
    /// Optional system prompt override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<AppPromptSpec>,
    /// Optional tool filter policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<AppToolFilter>,
    /// Optional storage scoping settings (namespace, shareCredentials).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<AppStorageSpec>,
    /// Optional model selection preference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<AppModelSpec>,
}

/// Extract `_meta["navi.dev"]["app"]` profile request from raw initialize params.
pub fn extract_app_profile_request(params: &serde_json::Value) -> Option<AppProfileRequest> {
    params
        .get("_meta")?
        .get("navi.dev")?
        .get("app")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
}

/// Evaluate a `requiresEngine` semver range constraint against the actual engine version.
///
/// Supports a subset of semver range syntax:
/// - `>=1.2` — minimum version
/// - `<2.0` — exclusive upper bound
/// - `>=1.2 <2.0` — range (space-separated comparators, AND)
/// - `1.2` — exact match (equivalent to `=1.2`)
/// - `*` or absent — any version satisfies
///
/// Returns `Ok(())` if the engine version satisfies the constraint, or an
/// `Err` with a human-readable message including both versions.
pub fn evaluate_requires_engine(
    requires_engine: &Option<String>,
    engine_version: &str,
) -> Result<(), String> {
    let Some(constraint) = requires_engine else {
        return Ok(());
    };
    let constraint = constraint.trim();
    if constraint.is_empty() || constraint == "*" {
        return Ok(());
    }
    let engine = parse_semver(engine_version).ok_or_else(|| {
        format!(
            "requiresEngine: cannot parse engine version '{engine_version}' (expected semver major.minor.patch)"
        )
    })?;
    for comparator in constraint.split_whitespace() {
        if !satisfies_comparator(comparator, engine) {
            return Err(format!(
                "requiresEngine: engine version {engine_version} does not satisfy constraint '{constraint}'"
            ));
        }
    }
    Ok(())
}

/// Parse a semver string into `(major, minor, patch)`.
fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches('v');
    let core = v.split('-').next()?;
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let major = parts[0].parse::<u64>().ok()?;
    let minor = parts[1].parse::<u64>().ok()?;
    let patch = if parts.len() >= 3 {
        parts[2].parse::<u64>().unwrap_or(0)
    } else {
        0
    };
    Some((major, minor, patch))
}

/// Check if a single comparator (e.g. `>=1.2`, `<2.0`, `=1.2`) is satisfied.
fn satisfies_comparator(comparator: &str, version: (u64, u64, u64)) -> bool {
    let (op, ver_str) = if let Some(rest) = comparator.strip_prefix(">=") {
        (">=", rest)
    } else if let Some(rest) = comparator.strip_prefix("<=") {
        ("<=", rest)
    } else if let Some(rest) = comparator.strip_prefix('>') {
        (">", rest)
    } else if let Some(rest) = comparator.strip_prefix('<') {
        ("<", rest)
    } else if let Some(rest) = comparator.strip_prefix('=') {
        ("=", rest)
    } else {
        ("=", comparator)
    };
    let Some(target) = parse_semver(ver_str) else {
        return false;
    };
    match op {
        ">=" => version >= target,
        "<=" => version <= target,
        ">" => version > target,
        "<" => version < target,
        "=" => version == target,
        _ => false,
    }
}

/// Helper to attach `_meta["navi.dev"]["app"]` to an initialize response or capability object.
pub fn attach_app_profile_meta(
    meta_target: &mut serde_json::Map<String, serde_json::Value>,
    app_meta: AppProfileRequest,
) {
    let navi_dev = meta_target
        .entry("navi.dev".to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if let serde_json::Value::Object(map) = navi_dev {
        if let Ok(val) = serde_json::to_value(app_meta) {
            map.insert("app".to_string(), val);
        }
    }
}

/// Resolve a requested app profile to its effective values (ADR 0018 §5).
///
/// - `storage.namespace` defaults to the app `id` when absent.
/// - `storage.shareCredentials` defaults to `true` when absent.
///
/// The returned profile is what the engine must echo back as the *resolved*
/// profile, so the client can observe the defaults it did not specify.
pub fn resolve_app_profile(request: &AppProfileRequest) -> AppProfileRequest {
    let mut resolved = request.clone();
    let storage = resolved.storage.get_or_insert_with(AppStorageSpec::default);
    if storage.namespace.is_none() {
        storage.namespace = Some(resolved.id.clone());
    }
    if storage.share_credentials.is_none() {
        storage.share_credentials = Some(true);
    }
    resolved
}

// ---------------------------------------------------------------------------
// Bridge: v1 ↔ v2 conversions
// ---------------------------------------------------------------------------

/// Convert a v1 [`V1StopReason`] to a v2 [`StopReason`].
pub fn stop_reason_v1_to_v2(reason: &V1StopReason) -> StopReason {
    match reason {
        V1StopReason::EndTurn => StopReason::EndTurn,
        V1StopReason::MaxTokens => StopReason::MaxTokens,
        V1StopReason::MaxTurnRequests => StopReason::MaxTurnRequests,
        V1StopReason::Refusal => StopReason::Refusal,
        V1StopReason::Cancelled => StopReason::Cancelled,
        V1StopReason::Other(raw) => StopReason::Other(raw.clone()),
    }
}

/// Convert a v2 [`StopReason`] to a v1 [`V1StopReason`].
pub fn stop_reason_v2_to_v1(reason: &StopReason) -> V1StopReason {
    match reason {
        StopReason::EndTurn => V1StopReason::EndTurn,
        StopReason::MaxTokens => V1StopReason::MaxTokens,
        StopReason::MaxTurnRequests => V1StopReason::MaxTurnRequests,
        StopReason::Refusal => V1StopReason::Refusal,
        StopReason::Cancelled => V1StopReason::Cancelled,
        StopReason::Other(raw) => V1StopReason::Other(raw.clone()),
        // `StopReason` is #[non_exhaustive]; future variants are preserved.
        _ => V1StopReason::Other(format!("{reason:?}")),
    }
}

/// Convert a v1 [`V1ContentBlock`] to a v2 [`ContentBlock`].
///
/// Only `Text` is mapped structurally; `Other` is round-tripped through
/// `serde_json::Value` so unknown payloads survive the conversion.
pub fn content_block_v1_to_v2(block: &V1ContentBlock) -> ContentBlock {
    match block {
        V1ContentBlock::Text { text, .. } => {
            // ContentBlock::Text is #[non_exhaustive] — use the builder.
            // TextContent::new returns a TextContent with the text set.
            use agent_client_protocol::schema::v2::TextContent;
            ContentBlock::Text(TextContent::new(text.clone()))
        }
        V1ContentBlock::Other(value) => {
            // Re-parse the raw value as a v2 ContentBlock; if it doesn't
            // match a known variant, the v2 enum's `Other` variant catches it.
            serde_json::from_value(serde_json::to_value(value).unwrap_or_default()).unwrap_or(
                ContentBlock::Text(agent_client_protocol::schema::v2::TextContent::new(
                    String::new(),
                )),
            )
        }
    }
}

/// Convert a v2 [`ContentBlock`] to a v1 [`V1ContentBlock`].
pub fn content_block_v2_to_v1(block: &ContentBlock) -> V1ContentBlock {
    match block {
        ContentBlock::Text(t) => V1ContentBlock::text(&t.text),
        other => {
            let value = serde_json::to_value(other).unwrap_or_default();
            V1ContentBlock::Other(value)
        }
    }
}

/// Build a v2 [`InitializeRequest`] from a v1 [`V1InitializeParams`].
///
/// Used by the default `initialize_v2` handler implementation to delegate to
/// the v1 handler, and by the client to up-convert when needed.
pub fn initialize_params_v1_to_v2(params: &V1InitializeParams) -> InitializeRequest {
    let info = params
        .client_info
        .as_ref()
        .map(|i| Implementation::new(i.name.clone(), i.version.clone().unwrap_or_default()))
        .unwrap_or_else(|| Implementation::new("navi", env!("CARGO_PKG_VERSION")));
    let req = InitializeRequest::new(CrateProtocolVersion::V2, info);
    // v2 ClientCapabilities is a different shape from v1; pass empty defaults.
    // The v2 client capabilities are all unstable-featured, so the stable
    // baseline is just an empty struct with _meta.
    req
}

/// Build a v1 [`V1InitializeResult`] from a v2 [`InitializeResponse`].
///
/// Used by the server to down-convert a v2 handler response to the v1 shape
/// when a v1 client connects, and by the client to expose a uniform
/// `initialize_result()` accessor.
pub fn initialize_response_v2_to_v1(resp: &InitializeResponse) -> V1InitializeResult {
    let agent_info = V1ImplementationInfo {
        name: resp.info.name.clone(),
        title: resp.info.title.clone(),
        version: Some(resp.info.version.clone()),
    };
    V1InitializeResult {
        protocol_version: resp.protocol_version.as_u16() as u32,
        agent_capabilities: V1AgentCapabilities::default(),
        auth_methods: resp
            .auth_methods
            .iter()
            .map(|m| V1AuthMethod {
                id: m.method_id().to_string(),
                name: Some(m.name().to_string()),
                description: m.description().map(str::to_string),
                r#type: None,
            })
            .collect(),
        agent_info: Some(agent_info),
        meta: None,
    }
}

/// Build a v2 [`InitializeResponse`] from a v1 [`V1InitializeResult`].
///
/// Used by the default `initialize_v2` handler implementation to delegate to
/// the v1 handler and up-convert.
pub fn initialize_response_v1_to_v2(result: &V1InitializeResult) -> InitializeResponse {
    let info = result
        .agent_info
        .as_ref()
        .map(|i| Implementation::new(i.name.clone(), i.version.clone().unwrap_or_default()))
        .unwrap_or_else(|| Implementation::new("navi", env!("CARGO_PKG_VERSION")));
    let resp = InitializeResponse::new(CrateProtocolVersion::V2, info);
    // v1 AgentCapabilities.loadSession has no v2 equivalent in the stable
    // baseline; the v2 session capability is advertised as {} (baseline
    // methods). We don't up-convert the v1 capability shape because the v2
    // shape is structurally different (session-scoped groups, object markers).
    resp
}

/// Build a v2 [`NewSessionRequest`] from a v1 [`V1NewSessionParams`].
pub fn new_session_params_v1_to_v2(params: &V1NewSessionParams) -> NewSessionRequest {
    NewSessionRequest::new(params.cwd.clone())
}

/// Convert a v2 [`NewSessionResponse`] to a v1 [`V1NewSessionResult`].
pub fn new_session_response_v2_to_v1(resp: &NewSessionResponse) -> V1NewSessionResult {
    V1NewSessionResult {
        session_id: resp.session_id.to_string(),
        extra: std::collections::BTreeMap::new(),
    }
}

/// Build a v2 [`PromptRequest`] from a v1 [`V1PromptParams`].
pub fn prompt_params_v1_to_v2(params: &V1PromptParams) -> PromptRequest {
    let prompt: Vec<ContentBlock> = params.prompt.iter().map(content_block_v1_to_v2).collect();
    PromptRequest::new(params.session_id.clone(), prompt)
}

/// Build a v1 [`V1PromptResult`] from a v2 [`PromptResponse`].
///
/// In v2, the prompt response is an empty ack — the stop reason is delivered
/// via `state_update`. This conversion returns `EndTurn` as a placeholder;
/// the actual stop reason is projected separately in Phase 4.
pub fn prompt_response_v2_to_v1(_resp: &PromptResponse) -> V1PromptResult {
    V1PromptResult {
        stop_reason: V1StopReason::EndTurn,
        extra: std::collections::BTreeMap::new(),
    }
}

/// Negotiate the protocol version for a connection.
///
/// The client sends its highest supported version; the agent replies with
/// `min(client_version, agent_max)`. Returns the negotiated version as a
/// `u32` wire value.
pub fn negotiate_version(client_requested: u32) -> u32 {
    if client_requested >= PROTOCOL_VERSION_V2 {
        PROTOCOL_VERSION_V2
    } else {
        PROTOCOL_VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiate_v1_client_gets_v1() {
        assert_eq!(negotiate_version(1), 1);
    }

    #[test]
    fn negotiate_v2_client_gets_v2() {
        assert_eq!(negotiate_version(2), 2);
    }

    #[test]
    fn negotiate_future_version_gets_v2() {
        assert_eq!(negotiate_version(3), 2);
    }

    #[test]
    fn stop_reason_round_trip() {
        let cases = [
            V1StopReason::EndTurn,
            V1StopReason::MaxTokens,
            V1StopReason::MaxTurnRequests,
            V1StopReason::Refusal,
            V1StopReason::Cancelled,
            V1StopReason::Other("_custom".into()),
        ];
        for reason in &cases {
            let v2 = stop_reason_v1_to_v2(reason);
            let back = stop_reason_v2_to_v1(&v2);
            assert_eq!(&back, reason);
        }
    }

    #[test]
    fn content_block_text_round_trip() {
        let v1 = V1ContentBlock::text("hello");
        let v2 = content_block_v1_to_v2(&v1);
        let back = content_block_v2_to_v1(&v2);
        assert_eq!(back.as_text(), Some("hello"));
    }

    #[test]
    fn v2_initialize_response_round_trips() {
        let resp = InitializeResponse::new(
            CrateProtocolVersion::V2,
            Implementation::new("navi", "0.0.1"),
        );
        let value = serde_json::to_value(&resp).unwrap();
        let parsed: InitializeResponse = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.protocol_version, CrateProtocolVersion::V2);
        assert_eq!(parsed.info.name, "navi");
    }

    #[test]
    fn v2_initialize_response_has_required_info_and_capabilities() {
        let resp = InitializeResponse::new(
            CrateProtocolVersion::V2,
            Implementation::new("navi", "0.0.1"),
        );
        let value = serde_json::to_value(&resp).unwrap();
        // v2 requires `info` (not `agentInfo`) and `capabilities` (not `agentCapabilities`)
        assert!(value.get("info").is_some(), "v2 response must have `info`");
        assert!(
            value.get("capabilities").is_some(),
            "v2 response must have `capabilities`"
        );
        assert!(
            value.get("agentInfo").is_none(),
            "v2 response must NOT have v1 `agentInfo`"
        );
        assert!(
            value.get("agentCapabilities").is_none(),
            "v2 response must NOT have v1 `agentCapabilities`"
        );
    }

    #[test]
    fn v2_session_update_serializes_state_update() {
        let update = SessionUpdate::StateUpdate(StateUpdate::Running(RunningStateUpdate::new()));
        let value = serde_json::to_value(&update).unwrap();
        assert_eq!(value["sessionUpdate"], "state_update");
        assert_eq!(value["state"], "running");
    }

    #[test]
    fn v2_session_update_serializes_agent_message_chunk() {
        use agent_client_protocol::schema::v2::TextContent;
        let chunk = ContentChunk::new(
            ContentBlock::Text(TextContent::new("hi")),
            MessageId::new("msg-1"),
        );
        let update = SessionUpdate::AgentMessageChunk(chunk);
        let value = serde_json::to_value(&update).unwrap();
        assert_eq!(value["sessionUpdate"], "agent_message_chunk");
        assert_eq!(value["messageId"], "msg-1");
        assert_eq!(value["content"]["text"], "hi");
    }

    #[test]
    fn extract_app_profile_request_parses_navi_dev_meta() {
        let raw = serde_json::json!({
            "protocolVersion": 2,
            "_meta": {
                "navi.dev": {
                    "app": {
                        "id": "organize-anything",
                        "version": "0.3.1",
                        "requiresEngine": ">=0.7 <1.0",
                        "harness": { "ref": "organize-anything/install" },
                        "prompt": { "text": "You are a helpful organizer." },
                        "tools": { "profile": "app_only", "allow": [], "deny": [] },
                        "storage": { "namespace": "organize-anything", "shareCredentials": true },
                        "model": { "provider": "openai", "name": "gpt-4o", "userOverridable": true }
                    }
                }
            }
        });
        let req = extract_app_profile_request(&raw).expect("extract app profile");
        assert_eq!(req.id, "organize-anything");
        assert_eq!(req.version.as_deref(), Some("0.3.1"));
        assert_eq!(req.requires_engine.as_deref(), Some(">=0.7 <1.0"));
        // Typed fields
        assert_eq!(
            req.harness,
            Some(HarnessSpec::Ref("organize-anything/install".into()))
        );
        assert_eq!(
            req.prompt.as_ref().map(|p| p.text.as_str()),
            Some("You are a helpful organizer.")
        );
        assert_eq!(
            req.tools.as_ref().and_then(|t| t.profile.as_deref()),
            Some("app_only")
        );
        assert_eq!(
            req.storage.as_ref().and_then(|s| s.namespace.as_deref()),
            Some("organize-anything")
        );
        assert_eq!(
            req.storage.as_ref().and_then(|s| s.share_credentials),
            Some(true)
        );
        assert_eq!(
            req.model.as_ref().and_then(|m| m.provider.as_deref()),
            Some("openai")
        );
        assert_eq!(
            req.model.as_ref().and_then(|m| m.name.as_deref()),
            Some("gpt-4o")
        );
        assert_eq!(
            req.model.as_ref().and_then(|m| m.user_overridable),
            Some(true)
        );

        let mut map = serde_json::Map::new();
        attach_app_profile_meta(&mut map, req);
        assert_eq!(
            map["navi.dev"]["app"]["id"].as_str().unwrap(),
            "organize-anything"
        );
    }

    #[test]
    fn extract_app_profile_request_minimal() {
        let raw = serde_json::json!({
            "_meta": { "navi.dev": { "app": { "id": "tutor" } } }
        });
        let req = extract_app_profile_request(&raw).expect("extract app profile");
        assert_eq!(req.id, "tutor");
        assert!(req.version.is_none());
        assert!(req.requires_engine.is_none());
        assert!(req.harness.is_none());
        assert!(req.storage.is_none());
    }

    #[test]
    fn extract_app_profile_request_absent_returns_none() {
        let raw = serde_json::json!({ "protocolVersion": 2 });
        assert!(extract_app_profile_request(&raw).is_none());
    }

    #[test]
    fn evaluate_requires_engine_satisfies() {
        // Engine 0.7.0 satisfies >=0.7 <1.0
        assert!(evaluate_requires_engine(&Some(">=0.7 <1.0".into()), "0.7.0").is_ok());
        // Engine 1.0.0 does not satisfy <1.0
        assert!(evaluate_requires_engine(&Some("<1.0".into()), "1.0.0").is_err());
        // Engine 0.6.0 does not satisfy >=0.7
        assert!(evaluate_requires_engine(&Some(">=0.7".into()), "0.6.0").is_err());
        // No constraint → always ok
        assert!(evaluate_requires_engine(&None, "0.7.0").is_ok());
        // Wildcard → always ok
        assert!(evaluate_requires_engine(&Some("*".into()), "0.7.0").is_ok());
        // Empty → always ok
        assert!(evaluate_requires_engine(&Some("".into()), "0.7.0").is_ok());
        // Exact match
        assert!(evaluate_requires_engine(&Some("0.7.0".into()), "0.7.0").is_ok());
        assert!(evaluate_requires_engine(&Some("0.7.1".into()), "0.7.0").is_err());
        // v-prefix tolerated
        assert!(evaluate_requires_engine(&Some(">=v0.7".into()), "v0.7.0").is_ok());
        // Pre-release suffix tolerated
        assert!(evaluate_requires_engine(&Some(">=0.7".into()), "0.7.0-beta.1").is_ok());
    }

    #[test]
    fn evaluate_requires_engine_error_message_is_human_readable() {
        let err = evaluate_requires_engine(&Some(">=1.2 <2.0".into()), "0.7.0").unwrap_err();
        assert!(
            err.contains("0.7.0"),
            "error should mention engine version: {err}"
        );
        assert!(
            err.contains(">=1.2 <2.0"),
            "error should mention constraint: {err}"
        );
    }

    #[test]
    fn harness_spec_ref_round_trips() {
        let spec = HarnessSpec::Ref("my-pack".into());
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v["ref"], "my-pack");
        let back: HarnessSpec = serde_json::from_value(v).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn harness_spec_inline_round_trips() {
        let spec = HarnessSpec::Inline(serde_json::json!({"id": "inline-pack"}));
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v["inline"]["id"], "inline-pack");
        let back: HarnessSpec = serde_json::from_value(v).unwrap();
        assert_eq!(back, spec);
    }
}
