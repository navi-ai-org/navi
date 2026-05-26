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

    pub fn into_agent_event(self) -> Option<AgentEvent> {
        self.kind.into_agent_event()
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
    ApprovalResolved(ApprovalDecision),
    ToolStarted(ToolInvocation),
    ToolCompleted(ToolResult),
    HarnessTrace(Value),
    PatchProposed(PatchProposal),
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
    Error {
        message: String,
    },
}

impl RuntimeEventKind {
    pub fn into_agent_event(self) -> Option<AgentEvent> {
        match self {
            RuntimeEventKind::AssistantDelta { text } => Some(AgentEvent::ModelDelta { text }),
            RuntimeEventKind::AssistantThinkingDelta { text } => {
                Some(AgentEvent::ModelThinkingDelta { text })
            }
            RuntimeEventKind::ToolRequested(invocation) => {
                Some(AgentEvent::ToolRequested(invocation))
            }
            RuntimeEventKind::ApprovalRequired(request) => {
                Some(AgentEvent::ApprovalRequested(request))
            }
            RuntimeEventKind::ApprovalResolved(decision) => {
                Some(AgentEvent::ApprovalResolved(decision))
            }
            RuntimeEventKind::ToolCompleted(result) => Some(AgentEvent::ToolCompleted(result)),
            RuntimeEventKind::HarnessTrace(value) => Some(AgentEvent::HarnessTrace(value)),
            RuntimeEventKind::PatchProposed(patch) => Some(AgentEvent::PatchProposed(patch)),
            RuntimeEventKind::TokensUpdated {
                input_tokens,
                output_tokens,
            } => Some(AgentEvent::UsageReported {
                input_tokens,
                output_tokens,
            }),
            RuntimeEventKind::MicroCompactApplied { messages_cleared } => {
                Some(AgentEvent::MicroCompactApplied { messages_cleared })
            }
            RuntimeEventKind::AutoCompactStarted => Some(AgentEvent::AutoCompactStarted),
            RuntimeEventKind::AutoCompactCompleted { tokens_saved } => {
                Some(AgentEvent::AutoCompactCompleted { tokens_saved })
            }
            RuntimeEventKind::AutoCompactFailed { reason } => {
                Some(AgentEvent::AutoCompactFailed { reason })
            }
            RuntimeEventKind::Error { message } => Some(AgentEvent::Error { message }),
            _ => None,
        }
    }
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
