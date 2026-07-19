use crate::capability::CapabilityLedgerEntry;
use crate::goal::types::GoalStatus;
use crate::patch::PatchProposal;
use crate::tool::{ToolInvocation, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One display item in a transient subagent transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentTranscriptItem {
    /// Kind of transcript item.
    pub kind: SubagentTranscriptKind,
    /// Main one-line item text.
    pub title: String,
    /// Optional secondary text, already compacted for UI display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Optional success state for completed work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
}

/// Display item kind for a transient subagent transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubagentTranscriptKind {
    ToolRequested,
    ToolCompleted,
    Text,
}

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
    /// A capability lifecycle event was recorded by the policy layer.
    CapabilityRecorded(CapabilityLedgerEntry),
    /// The assistant has requested an interactive user choice.
    QuestionRequired(QuestionRequest),
    /// An interactive user choice has been resolved.
    QuestionResolved(QuestionResponse),
    /// Plan tool create is waiting for user review (blocks the turn).
    PlanReviewRequired(PlanReviewRequest),
    /// User finished plan review.
    PlanReviewResolved(PlanReviewResponse),
    /// Sudo password needed (no secret in event payload).
    SudoPasswordRequired(SudoPasswordRequest),
    /// A tool invocation has begun execution.
    ToolStarted(ToolInvocation),
    /// A tool invocation has completed.
    ToolCompleted(ToolResult),
    /// A nested subagent emitted a transient UI activity update.
    SubagentActivity {
        /// Parent subagent tool invocation id.
        invocation_id: String,
        /// Human-readable description of the latest nested activity.
        message: String,
    },
    /// A nested subagent emitted a transient transcript item for UI drill-down.
    SubagentTranscript {
        /// Parent subagent tool invocation id.
        invocation_id: String,
        /// Transcript item to append for the active UI session.
        item: SubagentTranscriptItem,
    },
    /// A harness-level diagnostic trace (profile, message count, tool count).
    HarnessTrace(Value),
    /// The harness stopped a turn before another model iteration.
    HarnessStopped {
        /// Machine-readable stop reason.
        reason: String,
        /// Human-readable diagnostic.
        message: String,
        /// Tool involved in the stop, when applicable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_name: Option<String>,
    },
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
        /// Number of tokens written to the prompt cache (Anthropic).
        cache_creation_tokens: u64,
        /// Number of tokens read from the prompt cache (Anthropic).
        cache_read_tokens: u64,
    },
    /// The session has been persisted to disk.
    SessionSaved {
        /// Identifier of the saved session.
        session_id: String,
    },
    /// Session display title was assigned or updated (provisional or model-named).
    SessionTitleUpdated {
        /// Identifier of the session.
        session_id: String,
        /// New display title.
        title: String,
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
        /// Model-produced summary that replaced older turns.
        #[serde(default)]
        summary: String,
        /// Non-system conversation messages retained after the summary.
        #[serde(default)]
        kept_recent_messages: usize,
    },
    /// An automatic conversation compaction has failed.
    AutoCompactFailed {
        /// Human-readable reason for the failure.
        reason: String,
    },
    /// Auto-dream memory consolidation has started.
    AutoDreamStarted {
        /// Hours since the last dream run.
        hours_since_last: u64,
        /// Number of sessions reviewed.
        sessions_reviewed: usize,
    },
    /// Auto-dream memory consolidation has completed.
    AutoDreamCompleted {
        /// Memories marked stale.
        marked_stale: usize,
        /// Duplicates merged.
        duplicates_merged: usize,
        /// Active memories remaining.
        active_count: usize,
    },
    /// Auto-dream memory consolidation has failed.
    AutoDreamFailed {
        /// Human-readable reason for the failure.
        reason: String,
    },
    /// The agent requested to set a goal via natural language.
    SetGoalRequested {
        /// The objective text.
        objective: String,
        /// Optional short UI label.
        short_description: Option<String>,
        /// Optional token budget.
        token_budget: Option<i64>,
    },
    GoalUpdated {
        /// The session this goal belongs to.
        session_id: String,
        /// Unique identifier for the goal.
        goal_id: String,
        /// The objective text.
        objective: String,
        /// Optional short UI label.
        short_description: Option<String>,
        /// Current goal status.
        status: GoalStatus,
        /// Tokens consumed so far.
        tokens_used: i64,
        /// Optional token budget.
        token_budget: Option<i64>,
    },
    /// An error occurred during agent execution.
    Error {
        /// Human-readable error message.
        message: String,
    },
    /// The agent proposed a plan in Plan mode.
    /// The UI should show a confirmation popup to implement or discard.
    PlanProposed {
        /// The session that proposed the plan.
        session_id: String,
        /// Title/summary of the plan.
        title: String,
        /// Ordered list of steps.
        steps: Vec<String>,
    },
    /// The agent mode changed (e.g. Default → Plan or Plan → Default).
    AgentModeChanged {
        /// The session whose mode changed.
        session_id: String,
        /// The new mode.
        mode: crate::plan_mode::AgentMode,
    },
    /// Progress from an external ACP agent peer (not a model provider).
    ///
    /// Text deltas from ACP are also emitted as [`AssistantDelta`] when
    /// applicable; this variant carries the full peer update for clients that
    /// want tool/plan/permission detail from the external agent.
    AcpPeerUpdate {
        /// Configured ACP agent id (e.g. `"devin"`).
        agent_id: String,
        /// ACP session id on the external agent.
        acp_session_id: String,
        /// Discriminator for the update kind (`agent_message_chunk`, `tool_call`, …).
        update_kind: String,
        /// Full update payload as JSON.
        update: Value,
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
            RuntimeEventKind::CapabilityRecorded(entry) => {
                Some(AgentEvent::CapabilityRecorded(entry))
            }
            RuntimeEventKind::QuestionRequired(request) => {
                Some(AgentEvent::QuestionRequested(request))
            }
            RuntimeEventKind::QuestionResolved(response) => {
                Some(AgentEvent::QuestionResolved(response))
            }
            RuntimeEventKind::PlanReviewRequired(request) => {
                Some(AgentEvent::PlanReviewRequested(request))
            }
            RuntimeEventKind::PlanReviewResolved(response) => {
                Some(AgentEvent::PlanReviewResolved(response))
            }
            RuntimeEventKind::SudoPasswordRequired(request) => {
                Some(AgentEvent::SudoPasswordRequested(request))
            }
            RuntimeEventKind::ToolCompleted(result) => Some(AgentEvent::ToolCompleted(result)),
            RuntimeEventKind::SubagentActivity {
                invocation_id,
                message,
            } => Some(AgentEvent::SubagentActivity {
                invocation_id,
                message,
            }),
            RuntimeEventKind::SubagentTranscript {
                invocation_id,
                item,
            } => Some(AgentEvent::SubagentTranscript {
                invocation_id,
                item,
            }),
            RuntimeEventKind::HarnessTrace(value) => Some(AgentEvent::HarnessTrace(value)),
            RuntimeEventKind::HarnessStopped {
                reason,
                message,
                tool_name,
            } => Some(AgentEvent::HarnessStopped {
                reason,
                message,
                tool_name,
            }),
            RuntimeEventKind::PatchProposed(patch) => Some(AgentEvent::PatchProposed(patch)),
            RuntimeEventKind::TokensUpdated {
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            } => Some(AgentEvent::UsageReported {
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            }),
            RuntimeEventKind::MicroCompactApplied { messages_cleared } => {
                Some(AgentEvent::MicroCompactApplied { messages_cleared })
            }
            RuntimeEventKind::AutoCompactStarted => Some(AgentEvent::AutoCompactStarted),
            RuntimeEventKind::AutoCompactCompleted {
                tokens_saved,
                summary,
                kept_recent_messages,
            } => Some(AgentEvent::AutoCompactCompleted {
                tokens_saved,
                summary,
                kept_recent_messages,
            }),
            RuntimeEventKind::AutoCompactFailed { reason } => {
                Some(AgentEvent::AutoCompactFailed { reason })
            }
            RuntimeEventKind::Error { message } => Some(AgentEvent::Error { message }),
            RuntimeEventKind::SetGoalRequested {
                objective,
                short_description,
                token_budget,
            } => Some(AgentEvent::SetGoalRequested {
                objective,
                short_description,
                token_budget,
            }),
            RuntimeEventKind::GoalUpdated {
                session_id,
                goal_id,
                objective,
                short_description,
                status,
                tokens_used,
                token_budget,
            } => Some(AgentEvent::GoalUpdated {
                session_id,
                goal_id,
                objective,
                short_description,
                status,
                tokens_used,
                token_budget,
            }),
            RuntimeEventKind::PlanProposed { title, steps, .. } => {
                Some(AgentEvent::PlanProposed { title, steps })
            }
            RuntimeEventKind::AgentModeChanged { mode, .. } => {
                Some(AgentEvent::AgentModeChanged { mode })
            }
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
        /// Optional multimodal content parts (images + text).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content_parts: Vec<crate::model::ContentPart>,
        /// Unix timestamp (seconds since epoch) when the user submitted this
        /// message. Used for wall-clock display in clients (e.g. TUI sticky bar).
        /// Optional for backward-compatible session JSON (pre-timestamp sessions).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        submitted_at: Option<u64>,
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
    /// Transient status for a nested subagent.
    SubagentActivity {
        /// Parent subagent tool invocation id.
        invocation_id: String,
        /// Human-readable description of the latest nested activity.
        message: String,
    },
    /// Transient drill-down transcript item for a nested subagent.
    SubagentTranscript {
        /// Parent subagent tool invocation id.
        invocation_id: String,
        /// Transcript item to append for this UI session.
        item: SubagentTranscriptItem,
    },
    /// A harness-level diagnostic trace.
    HarnessTrace(Value),
    /// The harness stopped a turn before another model iteration.
    HarnessStopped {
        /// Machine-readable stop reason.
        reason: String,
        /// Human-readable diagnostic.
        message: String,
        /// Tool involved in the stop, when applicable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_name: Option<String>,
    },
    /// A file patch was proposed by the assistant.
    PatchProposed(PatchProposal),
    /// A tool invocation requires user approval.
    ApprovalRequested(ApprovalRequest),
    /// An approval request was resolved.
    ApprovalResolved(ApprovalDecision),
    /// A capability lifecycle event was recorded by the policy layer.
    CapabilityRecorded(CapabilityLedgerEntry),
    /// The assistant requested an interactive user choice.
    QuestionRequested(QuestionRequest),
    /// An interactive user choice was resolved.
    QuestionResolved(QuestionResponse),
    /// Plan tool create is waiting for user review (blocks the turn).
    PlanReviewRequested(PlanReviewRequest),
    /// User finished plan review (approve / changes / quit).
    PlanReviewResolved(PlanReviewResponse),
    /// Bash/`sudo` needs a password; TUI shows a masked modal. Password is
    /// never included in this event — only a correlation id. The secret is
    /// delivered solely through the sudo password resolver oneshot.
    SudoPasswordRequested(SudoPasswordRequest),
    /// The same tool was called consecutively with identical arguments.
    /// The tool is NOT executed; this is a notification to the user.
    RepeatedToolCallWarning {
        /// Name of the repeated tool.
        tool_name: String,
        /// Warning message describing the repetition.
        message: String,
    },
    /// Repetitive/degenerate model output was detected (character runs,
    /// alternating patterns, or duplicate thinking blocks).
    RepetitionDetected {
        /// What kind of repetition was detected.
        kind: RepetitionWarningKind,
        /// Human-readable warning message.
        message: String,
    },
    /// An error occurred.
    Error {
        /// Human-readable error message.
        message: String,
    },
    /// Auto-dream memory consolidation has started.
    AutoDreamStarted {
        /// Hours since the last dream run.
        hours_since_last: u64,
        /// Number of sessions reviewed.
        sessions_reviewed: usize,
    },
    /// Auto-dream memory consolidation has completed.
    AutoDreamCompleted {
        /// Memories marked stale.
        marked_stale: usize,
        /// Duplicates merged.
        duplicates_merged: usize,
        /// Active memories remaining.
        active_count: usize,
    },
    /// Auto-dream memory consolidation has failed.
    AutoDreamFailed {
        /// Human-readable reason for the failure.
        reason: String,
    },
    /// Token usage was reported by the model provider.
    UsageReported {
        /// Number of input/prompt tokens consumed.
        input_tokens: u64,
        /// Number of output/completion tokens produced.
        output_tokens: u64,
        /// Number of tokens written to the prompt cache (Anthropic).
        cache_creation_tokens: u64,
        /// Number of tokens read from the prompt cache (Anthropic).
        cache_read_tokens: u64,
    },
    /// Short post-turn session recap ("Recap" line).
    SessionRecap {
        /// One- or two-sentence summary of the turn.
        summary: String,
        /// When true, the recap was generated but should not be shown (long-tail).
        suppressed: bool,
    },
    /// The provider stream broke mid-generation and is being resumed via
    /// prefill (assistant continuation). The UI can show a transient hint.
    StreamResuming {
        /// Characters of text accumulated before the break.
        accumulated_chars: usize,
        /// Retry attempt number (1-based).
        attempt: u32,
    },
    /// The agent requested to set a goal via natural language.
    SetGoalRequested {
        /// The objective text.
        objective: String,
        /// Optional short UI label.
        short_description: Option<String>,
        /// Optional token budget.
        token_budget: Option<i64>,
    },
    /// The session goal was updated (created, status change, budget exceeded).
    GoalUpdated {
        /// The session this goal belongs to.
        session_id: String,
        /// Unique identifier for the goal.
        goal_id: String,
        /// The objective text.
        objective: String,
        /// Optional short UI label.
        short_description: Option<String>,
        /// Current goal status.
        status: GoalStatus,
        /// Tokens consumed so far.
        tokens_used: i64,
        /// Optional token budget.
        token_budget: Option<i64>,
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
        /// Model-produced summary that replaced older turns.
        #[serde(default)]
        summary: String,
        /// Non-system conversation messages retained after the summary.
        #[serde(default)]
        kept_recent_messages: usize,
    },
    /// Automatic conversation compaction failed.
    AutoCompactFailed {
        /// Human-readable failure reason.
        reason: String,
    },
    /// The agent proposed a plan in Plan mode.
    PlanProposed {
        /// Title/summary of the plan.
        title: String,
        /// Ordered list of steps.
        steps: Vec<String>,
    },
    /// The agent mode changed (e.g. Default → Plan or Plan → Default).
    AgentModeChanged {
        /// The new mode.
        mode: crate::plan_mode::AgentMode,
    },
    /// Host should surface a user-visible notification.
    ///
    /// Desktop hosts call OS toasts; browser hosts map this to the Web
    /// Notifications API. Payload mirrors [`crate::notify::NotifyRequest`].
    NotificationRequested {
        title: String,
        body: String,
        #[serde(default)]
        urgency: crate::notify::NotificationUrgency,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        category: Option<String>,
    },
    /// A newer NAVI release is available (from update check).
    UpdateAvailable {
        current_version: String,
        latest_version: String,
        latest_tag: String,
        release_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default)]
        prerelease: bool,
    },
}

/// Kind of repetitive/degenerate output detected by the repetition detector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RepetitionWarningKind {
    /// Same character repeated many times (e.g. "aaaaaa...").
    CharRun {
        /// The repeating character.
        ch: char,
        /// How many consecutive occurrences.
        count: usize,
    },
    /// Two characters alternating many times (e.g. "-_-_-_").
    AlternatingPattern {
        /// The two-character pattern (e.g. "-_").
        pattern: String,
        /// How many cycles detected.
        cycles: usize,
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

/// A selectable option in a [`QuestionRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    /// Short option label shown in the selection UI and returned to the model.
    pub label: String,
    /// Optional explanatory text shown below the label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A pending interactive question requested by the assistant through the
/// `question` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionRequest {
    /// Unique identifier matching the tool invocation id.
    pub id: String,
    /// Prompt shown to the user.
    pub question: String,
    /// Selectable options.
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    /// Whether more than one option may be selected.
    #[serde(default)]
    pub multiple: bool,
    /// Whether the UI should allow a free-form custom answer.
    #[serde(default)]
    pub allow_custom: bool,
}

/// Resolution for an interactive [`QuestionRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuestionResponse {
    /// The user selected one or more answers.
    Answered {
        /// Question/tool invocation id.
        id: String,
        /// Selected labels or the custom answer text.
        answers: Vec<String>,
    },
    /// The user dismissed the question without answering.
    Dismissed {
        /// Question/tool invocation id.
        id: String,
    },
}

impl QuestionResponse {
    /// Returns the request id this response resolves.
    pub fn id(&self) -> &str {
        match self {
            Self::Answered { id, .. } | Self::Dismissed { id } => id,
        }
    }
}

/// Interactive plan review requested after `plan(action=create)`.
/// The turn **blocks** until the user resolves it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanReviewRequest {
    /// Tool invocation id (used as correlation key for the oneshot).
    pub id: String,
    /// Persisted plan id in SQLite.
    pub plan_id: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<String>,
}

/// User decision from the plan review modal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanReviewDecision {
    Approve,
    RequestChanges,
    Quit,
}

/// Resolution for a blocked plan review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanReviewResponse {
    /// Matching tool invocation id.
    pub id: String,
    pub plan_id: String,
    pub decision: PlanReviewDecision,
    /// Line-oriented comments from the modal.
    #[serde(default)]
    pub comments: Vec<crate::plan_store::PlanLineComment>,
    /// Freeform notes from the prompt field.
    #[serde(default)]
    pub freeform: String,
}

impl PlanReviewResponse {
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Request for a sudo password from the interactive TUI.
///
/// **Security:** this event must never carry the password itself — only a
/// correlation id and UI context (command summary). The secret is delivered
/// solely through [`crate::runtime::SudoPasswordResolver`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SudoPasswordRequest {
    /// Correlation id for the oneshot resolver.
    pub id: String,
    /// Short, non-secret description (e.g. truncated command).
    pub command_summary: String,
}

/// Resolution of a sudo password prompt.
///
/// Prefer not to serialize responses that still hold a password into logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SudoPasswordResponse {
    /// User entered a password. Cleared from memory after bash consumes it.
    Submitted {
        id: String,
        /// Secret — never write to chat history or tool observations.
        #[serde(skip_serializing)]
        password: String,
    },
    /// User cancelled the modal.
    Cancelled { id: String },
}

impl SudoPasswordResponse {
    pub fn id(&self) -> &str {
        match self {
            Self::Submitted { id, .. } | Self::Cancelled { id } => id,
        }
    }
}

/// The security risk category associated with an approval request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalRisk {
    /// Any tool execution in restricted mode.
    Tool,
    /// A file write operation.
    Write,
    /// A shell command execution.
    Command,
    /// A guarded command that requires explicit approval outside YOLO mode.
    Guarded,
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
