use crate::patch::PatchProposal;
use crate::tool::{ToolInvocation, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A versioned runtime event emitted during agent execution.
///
/// Wraps a [`RuntimeEventKind`] with a schema version so consumers can handle
/// forward-compatible event streams.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEvent {
    /// Event schema version. Currently `1`.
    #[serde(default)]
    pub version: u32,
    /// The specific event payload.
    pub kind: RuntimeEventKind,
}

impl RuntimeEvent {
    /// Creates a new event with version 1.
    pub fn new(kind: RuntimeEventKind) -> Self {
        Self { version: 1, kind }
    }

    /// Converts this event into an [`AgentEvent`] if the kind maps to one.
    ///
    /// Lifecycle-only events (session started/saved/finished, turn
    /// started/completed, tool started, context updated) return `None` because
    /// they have no direct agent-level counterpart.
    pub fn into_agent_event(self) -> Option<AgentEvent> {
        self.kind.into_agent_event()
    }
}

/// Discriminates the kind of runtime event emitted by the agent loop.
///
/// Variants cover the full session lifecycle from start through turn
/// execution, tool invocation, approval flow, compaction, and error reporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuntimeEventKind {
    /// A new session has been created.
    SessionStarted {
        /// Unique identifier for the session.
        session_id: String,
    },
    /// A new turn within the session has started.
    TurnStarted {
        /// Unique identifier for the turn.
        turn_id: String,
    },
    /// A streaming text delta from the assistant.
    AssistantDelta {
        /// Incremental text content.
        text: String,
    },
    /// A streaming thinking/reasoning delta from the assistant.
    AssistantThinkingDelta {
        /// Incremental thinking content.
        text: String,
    },
    /// The assistant has requested a tool invocation.
    ToolRequested(ToolInvocation),
    /// A tool invocation requires user approval before execution.
    ApprovalRequired(ApprovalRequest),
    /// An approval request has been resolved (approved or denied).
    ApprovalResolved(ApprovalDecision),
    /// A tool invocation has begun execution.
    ToolStarted(ToolInvocation),
    /// A tool invocation has completed.
    ToolCompleted(ToolResult),
    /// A harness-level diagnostic trace (profile, message count, tool count).
    HarnessTrace(Value),
    /// A file patch has been proposed by the assistant.
    PatchProposed(PatchProposal),
    /// The conversation context has been updated.
    ContextUpdated,
    /// Token usage has been reported by the model provider.
    TokensUpdated {
        /// Number of input/prompt tokens consumed.
        input_tokens: u64,
        /// Number of output/completion tokens produced.
        output_tokens: u64,
    },
    /// The session has been persisted to disk.
    SessionSaved {
        /// Identifier of the saved session.
        session_id: String,
    },
    /// A turn has completed with a final text response.
    TurnCompleted {
        /// Identifier of the completed turn.
        turn_id: String,
        /// Final assistant text for the turn.
        text: String,
    },
    /// The session has ended.
    SessionFinished {
        /// Identifier of the finished session.
        session_id: String,
    },
    /// Micro-compaction cleared stale read-only tool results from history.
    MicroCompactApplied {
        /// Number of tool result messages that were cleared.
        messages_cleared: usize,
    },
    /// An automatic conversation compaction has started.
    AutoCompactStarted,
    /// An automatic conversation compaction has completed.
    AutoCompactCompleted {
        /// Estimated number of tokens saved by compaction.
        tokens_saved: u64,
    },
    /// An automatic conversation compaction has failed.
    AutoCompactFailed {
        /// Human-readable reason for the failure.
        reason: String,
    },
    /// An error occurred during agent execution.
    Error {
        /// Human-readable error message.
        message: String,
    },
}

impl RuntimeEventKind {
    /// Converts this event kind into an [`AgentEvent`] if applicable.
    ///
    /// Returns `None` for lifecycle-only events that have no direct
    /// agent-level counterpart (session/turn lifecycle, tool started,
    /// context updated).
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

/// A high-level agent event suitable for client consumption.
///
/// Unlike [`RuntimeEventKind`], agent events represent the semantic actions
/// a client cares about: user input, model output, tool calls, approvals,
/// compaction, usage, and errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentEvent {
    /// The user submitted a new task or message.
    UserTaskSubmitted {
        /// The user's input text.
        text: String,
    },
    /// A complete model output with optional thinking/reasoning content.
    ModelOutput {
        /// The assistant's response text.
        text: String,
        /// Optional thinking or reasoning trace from the model.
        #[serde(default)]
        thinking: Option<String>,
    },
    /// A streaming text delta from the model.
    ModelDelta {
        /// Incremental text content.
        text: String,
    },
    /// A streaming thinking/reasoning delta from the model.
    ModelThinkingDelta {
        /// Incremental thinking content.
        text: String,
    },
    /// The assistant requested a tool invocation.
    ToolRequested(ToolInvocation),
    /// A tool invocation completed.
    ToolCompleted(ToolResult),
    /// A harness-level diagnostic trace.
    HarnessTrace(Value),
    /// A file patch was proposed by the assistant.
    PatchProposed(PatchProposal),
    /// A tool invocation requires user approval.
    ApprovalRequested(ApprovalRequest),
    /// An approval request was resolved.
    ApprovalResolved(ApprovalDecision),
    /// The same tool was called consecutively with identical arguments.
    /// The tool still executes; this is a notification to the user.
    RepeatedToolCallWarning {
        /// Name of the repeated tool.
        tool_name: String,
        /// Warning message describing the repetition.
        message: String,
    },
    /// An error occurred.
    Error {
        /// Human-readable error message.
        message: String,
    },
    /// Token usage was reported by the model provider.
    UsageReported {
        /// Number of input/prompt tokens consumed.
        input_tokens: u64,
        /// Number of output/completion tokens produced.
        output_tokens: u64,
    },
    /// Micro-compaction cleared stale tool results from history.
    MicroCompactApplied {
        /// Number of tool result messages cleared.
        messages_cleared: usize,
    },
    /// Automatic conversation compaction started.
    AutoCompactStarted,
    /// Automatic conversation compaction completed.
    AutoCompactCompleted {
        /// Estimated tokens saved by compaction.
        tokens_saved: u64,
    },
    /// Automatic conversation compaction failed.
    AutoCompactFailed {
        /// Human-readable failure reason.
        reason: String,
    },
}

/// A pending approval request for a tool invocation that requires user consent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// Unique identifier for this approval request.
    pub id: String,
    /// Human-readable summary of what the tool will do.
    pub summary: String,
    /// The security risk category that triggered the approval requirement.
    pub risk: ApprovalRisk,
}

/// The security risk category associated with an approval request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApprovalRisk {
    /// A file write operation.
    Write,
    /// A shell command execution.
    Command,
    /// Loading or executing an external plugin.
    ExternalPlugin,
}

/// The outcome of an approval request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApprovalDecision {
    /// The user approved the action.
    Approved {
        /// Identifier matching the [`ApprovalRequest::id`].
        id: String,
    },
    /// The user denied the action.
    Denied {
        /// Identifier matching the [`ApprovalRequest::id`].
        id: String,
    },
}
