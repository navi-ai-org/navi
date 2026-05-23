use crate::patch::PatchProposal;
use crate::tool::{ToolInvocation, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub version: u32,
    pub kind: RuntimeEventKind,
}

impl RuntimeEvent {
    pub fn new(kind: RuntimeEventKind) -> Self {
        Self { version: 1, kind }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuntimeEventKind {
    SessionStarted {
        session_id: String,
    },
    TurnStarted {
        turn_id: String,
    },
    AssistantDelta {
        text: String,
    },
    AssistantThinkingDelta {
        text: String,
    },
    ToolRequested(ToolInvocation),
    ApprovalRequired(ApprovalRequest),
    ToolStarted(ToolInvocation),
    ToolCompleted(ToolResult),
    ContextUpdated,
    TokensUpdated {
        input_tokens: u64,
        output_tokens: u64,
    },
    SessionSaved {
        session_id: String,
    },
    TurnCompleted {
        turn_id: String,
        text: String,
    },
    SessionFinished {
        session_id: String,
    },
    Error {
        message: String,
    },
    LegacyAgentEvent(AgentEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentEvent {
    UserTaskSubmitted {
        text: String,
    },
    ModelOutput {
        text: String,
        #[serde(default)]
        thinking: Option<String>,
    },
    ModelDelta {
        text: String,
    },
    ModelThinkingDelta {
        text: String,
    },
    ToolRequested(ToolInvocation),
    ToolCompleted(ToolResult),
    HarnessTrace(Value),
    PatchProposed(PatchProposal),
    ApprovalRequested(ApprovalRequest),
    ApprovalResolved(ApprovalDecision),
    Error {
        message: String,
    },
    UsageReported {
        input_tokens: u64,
        output_tokens: u64,
    },
    MicroCompactApplied {
        messages_cleared: usize,
    },
    AutoCompactStarted,
    AutoCompactCompleted {
        tokens_saved: u64,
    },
    AutoCompactFailed {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub summary: String,
    pub risk: ApprovalRisk,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApprovalRisk {
    Write,
    Command,
    ExternalPlugin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApprovalDecision {
    Approved { id: String },
    Denied { id: String },
}
