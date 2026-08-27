use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const PROTOCOL_VERSION: u32 = 1;

/// v2 protocol version (ADR 0023). Only meaningful when the `acp-v2` feature
/// is enabled; v1 remains the default.
pub const PROTOCOL_VERSION_V2: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: u32,
    pub client_capabilities: ClientCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_info: Option<ImplementationInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs: Option<FsCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FsCapabilities {
    #[serde(default)]
    pub read_text_file: bool,
    #[serde(default)]
    pub write_text_file: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImplementationInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: u32,
    #[serde(default)]
    pub agent_capabilities: AgentCapabilities,
    #[serde(default)]
    pub auth_methods: Vec<AuthMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_info: Option<ImplementationInfo>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    #[serde(default)]
    pub load_session: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_capabilities: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_capabilities: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_capabilities: Option<Value>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthMethod {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticateParams {
    pub method_id: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionParams {
    pub cwd: String,
    #[serde(default)]
    pub mcp_servers: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionResult {
    pub session_id: String,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptParams {
    pub session_id: String,
    pub prompt: Vec<ContentBlock>,
}

#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text {
        text: String,
        annotations: Option<Value>,
    },
    /// Unknown content block type. The raw tagged payload is preserved so a
    /// session saved by a newer engine and replayed by an older one keeps the
    /// data. `_`-prefixed types are third-party extensions; non-prefixed types
    /// are reserved future ACP variants — both are preserved, never discarded.
    Other(Value),
}

impl<'de> Deserialize<'de> for ContentBlock {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let tag = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| serde::de::Error::custom("missing `type` tag on content block"))?;
        match tag {
            "text" => {
                let text = value
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        serde::de::Error::custom("missing `text` on text content block")
                    })?
                    .to_string();
                let annotations = value.get("annotations").cloned();
                Ok(Self::Text { text, annotations })
            }
            // Unknown values — `_`-prefixed (third-party extension) or not
            // (reserved future ACP) — are preserved verbatim, never discarded.
            _ => Ok(Self::Other(value)),
        }
    }
}

impl Serialize for ContentBlock {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Text { text, annotations } => {
                use serde::ser::SerializeStruct;
                let mut s = serializer.serialize_struct("ContentBlock", 2)?;
                s.serialize_field("type", "text")?;
                s.serialize_field("text", text)?;
                if let Some(annotations) = annotations {
                    s.serialize_field("annotations", annotations)?;
                }
                s.end()
            }
            Self::Other(value) => value.serialize(serializer),
        }
    }
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            annotations: None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text, .. } => Some(text.as_str()),
            Self::Other(_) => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResult {
    pub stop_reason: StopReason,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    /// Unknown stop reason. The raw value is preserved so session replay
    /// round-trips losslessly.
    Other(String),
}

impl<'de> Deserialize<'de> for StopReason {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "end_turn" => Self::EndTurn,
            "max_tokens" => Self::MaxTokens,
            "max_turn_requests" => Self::MaxTurnRequests,
            "refusal" => Self::Refusal,
            "cancelled" => Self::Cancelled,
            _ => Self::Other(raw),
        })
    }
}

impl Serialize for StopReason {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let s = match self {
            Self::EndTurn => "end_turn",
            Self::MaxTokens => "max_tokens",
            Self::MaxTurnRequests => "max_turn_requests",
            Self::Refusal => "refusal",
            Self::Cancelled => "cancelled",
            Self::Other(raw) => return serializer.serialize_str(raw),
        };
        serializer.serialize_str(s)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelParams {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNotification {
    pub session_id: String,
    pub update: SessionUpdate,
}

#[derive(Debug, Clone)]
pub enum SessionUpdate {
    AgentMessageChunk {
        message_id: Option<String>,
        content: ContentBlock,
    },
    AgentThoughtChunk {
        message_id: Option<String>,
        content: ContentBlock,
    },
    UserMessageChunk {
        message_id: Option<String>,
        content: ContentBlock,
    },
    ToolCall {
        tool_call_id: String,
        title: Option<String>,
        kind: Option<String>,
        status: Option<String>,
        content: Option<Value>,
        locations: Option<Value>,
        raw_input: Option<Value>,
        raw_output: Option<Value>,
    },
    ToolCallUpdate {
        tool_call_id: String,
        title: Option<String>,
        kind: Option<String>,
        status: Option<String>,
        content: Option<Value>,
        locations: Option<Value>,
        raw_input: Option<Value>,
        raw_output: Option<Value>,
    },
    Plan {
        entries: Vec<Value>,
    },
    UsageUpdate {
        used: u64,
        size: u64,
        cost: Option<Value>,
    },
    /// Unknown `sessionUpdate` value. The raw tagged payload is preserved so a
    /// session saved by a newer engine and replayed by an older one keeps the
    /// data. `_`-prefixed values are third-party extensions; non-prefixed
    /// values are reserved future ACP variants — both are preserved, never
    /// treated as an extension and never discarded.
    Other(Value),
}

impl<'de> Deserialize<'de> for SessionUpdate {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let tag = value
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .ok_or_else(|| serde::de::Error::custom("missing `sessionUpdate` tag on update"))?;
        let field = |name: &str| value.get(name).cloned();
        let message_id = || -> Result<Option<String>, D::Error> {
            match field("messageId") {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(s)) => Ok(Some(s)),
                Some(_) => Err(serde::de::Error::custom(
                    "`messageId` must be a string or null",
                )),
            }
        };
        let required_string = |name: &str| {
            value
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| {
                    serde::de::Error::custom(format!("missing `{name}` on `{tag}` update"))
                })
        };
        Ok(match tag {
            "agent_message_chunk" => Self::AgentMessageChunk {
                message_id: message_id()?,
                content: serde_json::from_value(field("content").unwrap_or(Value::Null))
                    .map_err(serde::de::Error::custom)?,
            },
            "agent_thought_chunk" => Self::AgentThoughtChunk {
                message_id: message_id()?,
                content: serde_json::from_value(field("content").unwrap_or(Value::Null))
                    .map_err(serde::de::Error::custom)?,
            },
            "user_message_chunk" => Self::UserMessageChunk {
                message_id: message_id()?,
                content: serde_json::from_value(field("content").unwrap_or(Value::Null))
                    .map_err(serde::de::Error::custom)?,
            },
            "tool_call" => Self::ToolCall {
                tool_call_id: required_string("toolCallId")?,
                title: field("title").and_then(string_or_null),
                kind: field("kind").and_then(string_or_null),
                status: field("status").and_then(string_or_null),
                content: field("content").filter(|v| !v.is_null()),
                locations: field("locations").filter(|v| !v.is_null()),
                raw_input: field("rawInput").filter(|v| !v.is_null()),
                raw_output: field("rawOutput").filter(|v| !v.is_null()),
            },
            "tool_call_update" => Self::ToolCallUpdate {
                tool_call_id: required_string("toolCallId")?,
                title: field("title").and_then(string_or_null),
                kind: field("kind").and_then(string_or_null),
                status: field("status").and_then(string_or_null),
                content: field("content").filter(|v| !v.is_null()),
                locations: field("locations").filter(|v| !v.is_null()),
                raw_input: field("rawInput").filter(|v| !v.is_null()),
                raw_output: field("rawOutput").filter(|v| !v.is_null()),
            },
            "plan" => Self::Plan {
                entries: field("entries")
                    .and_then(|v| match v {
                        Value::Array(a) => Some(a),
                        _ => None,
                    })
                    .unwrap_or_default(),
            },
            "usage_update" => Self::UsageUpdate {
                used: field("used")
                    .as_ref()
                    .and_then(Value::as_u64)
                    .ok_or_else(|| serde::de::Error::custom("missing `used` on `usage_update`"))?,
                size: field("size")
                    .as_ref()
                    .and_then(Value::as_u64)
                    .ok_or_else(|| serde::de::Error::custom("missing `size` on `usage_update`"))?,
                cost: field("cost").filter(|v| !v.is_null()),
            },
            // Unknown values — `_`-prefixed (third-party extension) or not
            // (reserved future ACP) — are preserved verbatim, never discarded.
            _ => Self::Other(value),
        })
    }
}

fn string_or_null(v: Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s),
        _ => None,
    }
}

impl Serialize for SessionUpdate {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        match self {
            Self::AgentMessageChunk {
                message_id,
                content,
            } => {
                let mut s = serializer.serialize_struct("SessionUpdate", 3)?;
                s.serialize_field("sessionUpdate", "agent_message_chunk")?;
                if let Some(message_id) = message_id {
                    s.serialize_field("messageId", message_id)?;
                }
                s.serialize_field("content", content)?;
                s.end()
            }
            Self::AgentThoughtChunk {
                message_id,
                content,
            } => {
                let mut s = serializer.serialize_struct("SessionUpdate", 3)?;
                s.serialize_field("sessionUpdate", "agent_thought_chunk")?;
                if let Some(message_id) = message_id {
                    s.serialize_field("messageId", message_id)?;
                }
                s.serialize_field("content", content)?;
                s.end()
            }
            Self::UserMessageChunk {
                message_id,
                content,
            } => {
                let mut s = serializer.serialize_struct("SessionUpdate", 3)?;
                s.serialize_field("sessionUpdate", "user_message_chunk")?;
                if let Some(message_id) = message_id {
                    s.serialize_field("messageId", message_id)?;
                }
                s.serialize_field("content", content)?;
                s.end()
            }
            Self::ToolCall {
                tool_call_id,
                title,
                kind,
                status,
                content,
                locations,
                raw_input,
                raw_output,
            } => {
                let mut s = serializer.serialize_struct("SessionUpdate", 9)?;
                s.serialize_field("sessionUpdate", "tool_call")?;
                s.serialize_field("toolCallId", tool_call_id)?;
                s.serialize_field("title", title)?;
                s.serialize_field("kind", kind)?;
                s.serialize_field("status", status)?;
                s.serialize_field("content", content)?;
                s.serialize_field("locations", locations)?;
                s.serialize_field("rawInput", raw_input)?;
                s.serialize_field("rawOutput", raw_output)?;
                s.end()
            }
            Self::ToolCallUpdate {
                tool_call_id,
                title,
                kind,
                status,
                content,
                locations,
                raw_input,
                raw_output,
            } => {
                let mut s = serializer.serialize_struct("SessionUpdate", 9)?;
                s.serialize_field("sessionUpdate", "tool_call_update")?;
                s.serialize_field("toolCallId", tool_call_id)?;
                s.serialize_field("title", title)?;
                s.serialize_field("kind", kind)?;
                s.serialize_field("status", status)?;
                s.serialize_field("content", content)?;
                s.serialize_field("locations", locations)?;
                s.serialize_field("rawInput", raw_input)?;
                s.serialize_field("rawOutput", raw_output)?;
                s.end()
            }
            Self::Plan { entries } => {
                let mut s = serializer.serialize_struct("SessionUpdate", 2)?;
                s.serialize_field("sessionUpdate", "plan")?;
                s.serialize_field("entries", entries)?;
                s.end()
            }
            Self::UsageUpdate { used, size, cost } => {
                let mut s = serializer.serialize_struct("SessionUpdate", 4)?;
                s.serialize_field("sessionUpdate", "usage_update")?;
                s.serialize_field("used", used)?;
                s.serialize_field("size", size)?;
                s.serialize_field("cost", cost)?;
                s.end()
            }
            Self::Other(value) => value.serialize(serializer),
        }
    }
}

impl SessionUpdate {
    pub fn agent_text(&self) -> Option<&str> {
        match self {
            Self::AgentMessageChunk { content, .. } => content.as_text(),
            _ => None,
        }
    }

    pub fn thought_text(&self) -> Option<&str> {
        match self {
            Self::AgentThoughtChunk { content, .. } => content.as_text(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPermissionParams {
    pub session_id: String,
    pub tool_call: Value,
    pub options: Vec<PermissionOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPermissionResult {
    pub outcome: PermissionOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PermissionOutcome {
    Selected {
        #[serde(rename = "optionId")]
        option_id: String,
    },
    Cancelled,
}
