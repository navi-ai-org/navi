// Type definitions mirroring the Rust RuntimeEvent, AgentEvent, and API DTOs.
// Only the subset needed for the chat-only MVP is included.
//
// IMPORTANT: Rust enums without #[serde(tag = ...)] serialize as
// externally-tagged: { "VariantName": { ...fields } }.

// ── Runtime events (WebSocket, live streaming) ───────────────────────────

export type RuntimeEventKind =
  | { SessionStarted: { session_id: string } }
  | { TurnStarted: { turn_id: string } }
  | { AssistantDelta: { text: string } }
  | { AssistantThinkingDelta: { text: string } }
  | { ToolRequested: ToolInvocation }
  | { ToolStarted: ToolInvocation }
  | { ToolCompleted: ToolResult }
  | { ApprovalRequired: ApprovalRequest }
  | { ApprovalResolved: ApprovalDecision }
  | { QuestionRequired: QuestionRequest }
  | { QuestionResolved: QuestionResponse }
  | {
      TokensUpdated: {
        input_tokens: number;
        output_tokens: number;
        cache_creation_tokens?: number;
        cache_read_tokens?: number;
      };
    }
  | { TurnCompleted: { turn_id: string; text: string } }
  | { SessionFinished: { session_id: string } }
  | { Error: { message: string } }
  | { SessionTitleUpdated: { session_id: string; title: string } }
  | { ContextUpdated: Record<string, never> }
  | { AutoCompactStarted: Record<string, never> }
  | {
      AutoCompactCompleted: {
        tokens_saved: number;
        summary: string;
        kept_recent_messages?: number;
      };
    }
  | { AutoCompactFailed: { reason: string } }
  | { PlanProposed: { session_id: string; title: string; steps: string[] } }
  | { AgentModeChanged: { session_id: string; mode: string } }
  | {
      PlanReviewRequired: {
        id: string;
        plan_id: string;
        title: string;
        description: string;
        steps: string[];
        body_markdown?: string;
        plan_file_path?: string;
      };
    }
  | {
      PlanReviewResolved: {
        id: string;
        plan_id: string;
        decision: "approve" | "request_changes" | "quit";
        freeform?: string;
      };
    }
  | {
      SudoPasswordRequired: {
        id: string;
        command_summary: string;
      };
    };

export interface RuntimeEvent {
  version: number;
  kind: RuntimeEventKind;
}

// ── Agent events (persisted in snapshots) ────────────────────────────────

export type AgentEvent =
  | { UserTaskSubmitted: { text: string; content_parts?: ContentPart[]; submitted_at?: number } }
  | { ModelOutput: { text: string; thinking?: string } }
  | { ModelDelta: { text: string } }
  | { ModelThinkingDelta: { text: string } }
  | { ToolRequested: ToolInvocation }
  | { ToolCompleted: ToolResult }
  | { ApprovalRequested: ApprovalRequest }
  | { ApprovalResolved: ApprovalDecision }
  | { QuestionRequested: QuestionRequest }
  | { QuestionResolved: QuestionResponse }
  | { Error: { message: string } }
  | { UsageReported: { input_tokens: number; output_tokens: number; cache_creation_tokens?: number; cache_read_tokens?: number } }
  | { PlanProposed: { title: string; steps: string[] } }
  | { AgentModeChanged: { mode: string } }
  | { SessionRecap: { text: string } }
  | { AutoCompactStarted: Record<string, never> }
  | { AutoCompactCompleted: { tokens_saved: number; summary: string; kept_recent_messages?: number } }
  | { AutoCompactFailed: { reason: string } }
  // Catch-all for variants we don't render in the chat
  | Record<string, unknown>;

// ── Shared event payload types ───────────────────────────────────────────

export interface ToolInvocation {
  id: string;
  tool_name: string;
  input: unknown;
}

export interface ToolResult {
  invocation_id: string;
  ok: boolean;
  output: unknown;
}

export interface ApprovalRequest {
  request_id: string;
  tool_name: string;
  description: string;
  input?: unknown;
}

export interface ApprovalDecision {
  request_id: string;
  approved: boolean;
  message?: string;
}

export interface QuestionRequest {
  question_id: string;
  question: string;
  options?: string[];
}

export interface QuestionResponse {
  question_id: string;
  answer?: string;
  custom?: string;
}

export interface ContentPart {
  type: "text" | "image" | "audio" | "video" | "document";
  text?: string;
  media_type?: string;
  data?: string;
  name?: string;
}

// ── Session snapshot ─────────────────────────────────────────────────────

export interface SessionSnapshot {
  version: number;
  id: string;
  title?: string;
  project: string;
  created_at: number;
  updated_at: number;
  events: AgentEvent[];
  memory?: unknown;
  goal?: unknown;
  usage?: SessionUsageSnapshot;
}

export interface SessionUsageSnapshot {
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  cost_known: boolean;
  credits_spent?: number;
  credit_unit?: string;
}

// ── API DTOs ─────────────────────────────────────────────────────────────

export interface SessionInfo {
  id: string;
  projectDir?: string;
  model?: string;
  provider?: string;
  title?: string;
  snapshot?: SessionSnapshot;
}

export interface SavedSessionInfo {
  id: string;
  title: string;
  updated_at: string;
  message_count: number;
}

export interface ModelInfo {
  provider: string;
  name: string;
  label?: string;
  context_window_tokens?: number;
}

export interface TurnResponse {
  sessionId: string;
  text: string;
}

// ── UI state ─────────────────────────────────────────────────────────────

export interface ChatMessage {
  id: string;
  role: "user" | "assistant" | "system";
  text: string;
  thinking?: string;
  timestamp: number;
  streaming?: boolean;
  toolCalls?: ToolCallInfo[];
  toolName?: string;
  toolResult?: { ok: boolean; output: unknown };
}

export interface ToolCallInfo {
  name: string;
  status: "requested" | "started" | "completed" | "failed";
  summary?: string;
}

export interface PendingApproval {
  requestId: string;
  toolName: string;
  description: string;
}

export interface PendingQuestion {
  questionId: string;
  question: string;
  options?: string[];
}

// ── Permission mode ──────────────────────────────────────────────────────

export type PermissionMode = "restricted" | "accept_edits" | "auto" | "yolo";

// ── Agent mode ───────────────────────────────────────────────────────────

export type AgentMode = "default" | "plan";

// ── Token usage ──────────────────────────────────────────────────────────

export interface TokenUsage {
  inputTokens: number;
  outputTokens: number;
}

// ── Plan review ──────────────────────────────────────────────────────────

export interface PlanReviewRequestPayload {
  id: string;
  planId: string;
  title: string;
  description: string;
  steps: string[];
  bodyMarkdown?: string;
  planFilePath?: string;
}

export type PlanReviewDecision = "approve" | "request_changes" | "quit";

export interface PendingPlanReview {
  id: string;
  planId: string;
  title: string;
  description: string;
  steps: string[];
  bodyMarkdown?: string;
}

// ── Sudo password ────────────────────────────────────────────────────────

export interface PendingSudoPrompt {
  id: string;
  commandSummary: string;
}

// ── Auto-compact notification ────────────────────────────────────────────

export interface CompactNotification {
  type: "started" | "completed" | "failed";
  tokensSaved?: number;
  summary?: string;
  reason?: string;
}

// ── Goal ─────────────────────────────────────────────────────────────────

export type GoalStatus =
  | "active"
  | "paused"
  | "blocked"
  | "usage_limited"
  | "budget_limited"
  | "complete";

export interface SessionGoal {
  sessionId: string;
  goalId: string;
  objective: string;
  shortDescription?: string;
  status: GoalStatus;
  tokenBudget?: number;
  tokensUsed: number;
  timeUsedSeconds: number;
  consecutiveBlockedTurns: number;
  blockReason?: string;
  checklist: GoalTask[];
  createdAt: number;
  updatedAt: number;
}

export interface GoalTask {
  id: number;
  description: string;
  status: "pending" | "in_progress" | "done" | "verified" | "skipped";
  verification?: string;
  verified: boolean;
}
