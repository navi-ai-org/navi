import { writable, derived, type Writable } from "svelte/store";
import type {
  AgentEvent,
  ChatMessage,
  PendingApproval,
  PendingQuestion,
  SessionInfo,
  SavedSessionInfo,
  ModelInfo,
  ToolInvocation,
} from "./types";
import { getSecret } from "./api";
import type { ConnectionStatus } from "./ws";

// ── Auth ─────────────────────────────────────────────────────────────────

export const secret: Writable<string> = writable(getSecret());

export const isAuthenticated = derived(secret, ($s) => $s.length > 0);

// ── Sessions ─────────────────────────────────────────────────────────────

export const activeSession: Writable<SessionInfo | null> = writable(null);

export const sessions: Writable<string[]> = writable([]);

export const savedSessions: Writable<SavedSessionInfo[]> = writable([]);

// ── Chat ─────────────────────────────────────────────────────────────────

export const messages: Writable<ChatMessage[]> = writable([]);

export const pendingApproval: Writable<PendingApproval | null> = writable(null);

export const pendingQuestion: Writable<PendingQuestion | null> = writable(null);

export const isStreaming: Writable<boolean> = writable(false);

export const wsStatus: Writable<ConnectionStatus> = writable("disconnected");

// ── Models ───────────────────────────────────────────────────────────────

export const models: Writable<ModelInfo[]> = writable([]);

// ── UI ───────────────────────────────────────────────────────────────────

export const showSidebar: Writable<boolean> = writable(false);

export const error: Writable<string | null> = writable(null);

// ── Helpers ──────────────────────────────────────────────────────────────

let msgIdCounter = 0;

export function makeMessage(
  role: ChatMessage["role"],
  text: string,
  streaming = false,
): ChatMessage {
  return {
    id: `msg-${++msgIdCounter}`,
    role,
    text,
    timestamp: Date.now(),
    streaming,
  };
}

export function clearChat(): void {
  messages.set([]);
  pendingApproval.set(null);
  pendingQuestion.set(null);
  isStreaming.set(false);
}

// ── Snapshot → ChatMessage conversion ────────────────────────────────────
//
// Mirrors navi-tui's persistence.rs replay logic:
// - UserTaskSubmitted → user message
// - ModelOutput → assistant message (with optional thinking)
// - ToolRequested → tracked in a map by invocation ID
// - ToolCompleted → paired with stored invocation → tool result message
// - ModelDelta events are accumulated into the last assistant message

export function eventsToMessages(events: AgentEvent[]): ChatMessage[] {
  const result: ChatMessage[] = [];
  const toolInvocations = new Map<string, ToolInvocation>();

  for (const event of events) {
    // Externally-tagged enum: the variant name is the single key.
    const keys = Object.keys(event);
    if (keys.length !== 1) continue;
    const variant = keys[0];
    const payload = (event as Record<string, unknown>)[variant] as Record<
      string,
      unknown
    >;

    switch (variant) {
      case "UserTaskSubmitted": {
        const text = (payload.text as string) ?? "";
        const submittedAt = payload.submitted_at as number | undefined;
        result.push({
          id: `msg-${++msgIdCounter}`,
          role: "user",
          text,
          timestamp: submittedAt ? submittedAt * 1000 : Date.now(),
        });
        break;
      }
      case "ModelOutput": {
        const text = (payload.text as string) ?? "";
        const thinking = payload.thinking as string | undefined;
        result.push({
          id: `msg-${++msgIdCounter}`,
          role: "assistant",
          text,
          thinking: thinking || undefined,
          timestamp: Date.now(),
        });
        break;
      }
      case "ToolRequested": {
        const inv = payload as unknown as ToolInvocation;
        toolInvocations.set(inv.id, inv);
        break;
      }
      case "ToolCompleted": {
        const invocationId = payload.invocation_id as string;
        const ok = payload.ok as boolean;
        const invocation = toolInvocations.get(invocationId);
        if (invocation) {
          result.push({
            id: `msg-${++msgIdCounter}`,
            role: "assistant",
            text: "",
            toolName: invocation.tool_name,
            toolResult: { ok, output: payload.output },
            timestamp: Date.now(),
          });
        }
        break;
      }
      case "Error": {
        const message = payload.message as string;
        result.push({
          id: `msg-${++msgIdCounter}`,
          role: "system",
          text: `Error: ${message}`,
          timestamp: Date.now(),
        });
        break;
      }
      // Other variants (UsageReported, ApprovalResolved, etc.) are not
      // rendered as chat messages — they're metadata.
    }
  }

  return result;
}

export function clearAllOnLogout(): void {
  clearChat();
  activeSession.set(null);
  sessions.set([]);
  savedSessions.set([]);
  models.set([]);
  error.set(null);
}
