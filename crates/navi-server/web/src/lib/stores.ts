import { writable, derived, type Writable } from "svelte/store";
import type {
  ChatMessage,
  PendingApproval,
  PendingQuestion,
  SessionInfo,
  SavedSessionInfo,
  ModelInfo,
} from "./types";
import { hasSecret, getSecret } from "./api";
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

export function clearAllOnLogout(): void {
  clearChat();
  activeSession.set(null);
  sessions.set([]);
  savedSessions.set([]);
  models.set([]);
  error.set(null);
}
