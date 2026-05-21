use crate::patch::PatchProposal;
use crate::tool::{ToolInvocation, ToolResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentEvent {
    UserTaskSubmitted { text: String },
    ModelOutput { text: String },
    ToolRequested(ToolInvocation),
    ToolCompleted(ToolResult),
    PatchProposed(PatchProposal),
    ApprovalRequested(ApprovalRequest),
    ApprovalResolved(ApprovalDecision),
    Error { message: String },
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
