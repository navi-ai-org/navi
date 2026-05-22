use crate::patch::PatchProposal;
use crate::tool::{ToolInvocation, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
