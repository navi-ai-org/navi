// Type definitions mirroring the Rust RuntimeEvent and API DTOs.
// Only the subset needed for the chat-only MVP is included.

// ── Runtime events (WebSocket) ───────────────────────────────────────────

export type RuntimeEventKind =
  | { kind: "SessionStarted"; session_id: string }
  | { kind: "TurnStarted"; turn_id: string }
  | { kind: "AssistantDelta"; text: string }
  | { kind: "AssistantThinkingDelta"; text: string }
  | { kind: "ToolRequested"; tool_name: string; summary?: string }
  | { kind: "ToolStarted"; tool_name: string; summary?: string }
  | {
      kind: "ToolCompleted";
      tool_name: string;
      success: boolean;
      summary?: string;
    }
  | {
      kind: "ApprovalRequired";
      request_id: string;
      tool_name: string;
      description: string;
    }
  | { kind: "ApprovalResolved"; approved: boolean }
  | {
      kind: "QuestionRequired";
      question_id: string;
      question: string;
      options?: string[];
    }
  | { kind: "QuestionResolved"; answer?: string }
  | { kind: "TokensUpdated"; input_tokens: number; output_tokens: number }
  | { kind: "TurnCompleted"; turn_id: string; text: string }
  | { kind: "SessionFinished"; session_id: string }
  | { kind: "Error"; message: string }
  | { kind: "SessionTitleUpdated"; session_id: string; title: string }
  | { kind: "ContextUpdated" }
  | { kind: "AutoCompactStarted" }
  | {
      kind: "AutoCompactCompleted";
      tokens_saved: number;
      summary: string;
    }
  | { kind: "AutoCompactFailed"; reason: string }
  | { kind: "PlanProposed"; session_id: string; title: string; steps: string[] }
  | { kind: "AgentModeChanged"; session_id: string; mode: string };

export interface RuntimeEvent {
  version: number;
  kind: RuntimeEventKind;
  // The raw event may have fields at the top level too.
  [key: string]: unknown;
}

// ── API DTOs ─────────────────────────────────────────────────────────────

export interface SessionInfo {
  id: string;
  projectDir?: string;
  model?: string;
  provider?: string;
  title?: string;
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
  timestamp: number;
  streaming?: boolean;
  toolCalls?: ToolCallInfo[];
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
