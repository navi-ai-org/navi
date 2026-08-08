import type {
  SessionInfo,
  SavedSessionInfo,
  ModelInfo,
  TurnResponse,
  SessionSnapshot,
} from "./types";

// ── API client ───────────────────────────────────────────────────────────
//
// All requests carry the X-Navi-Secret header for authentication.
// The secret is stored in localStorage and injected by the stores module.

const SECRET_KEY = "navi-secret";

export function getSecret(): string {
  return localStorage.getItem(SECRET_KEY) ?? "";
}

export function setSecret(secret: string): void {
  localStorage.setItem(SECRET_KEY, secret);
}

export function clearSecret(): void {
  localStorage.removeItem(SECRET_KEY);
}

export function hasSecret(): boolean {
  return getSecret().length > 0;
}

// ── Fetch helper ─────────────────────────────────────────────────────────

class ApiError extends Error {
  status: number;
  constructor(message: string, status: number) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

async function apiFetch<T>(
  path: string,
  options: RequestInit = {},
): Promise<T> {
  const secret = getSecret();
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    "X-Navi-Secret": secret,
    ...(options.headers as Record<string, string>),
  };

  const res = await fetch(path, { ...options, headers });

  if (!res.ok) {
    let message = `HTTP ${res.status}`;
    try {
      const body = await res.json();
      message = body.error ?? message;
    } catch {
      // Non-JSON error body
    }
    throw new ApiError(message, res.status);
  }

  // 204 No Content
  if (res.status === 204) {
    return undefined as T;
  }

  return res.json() as Promise<T>;
}

// ── Health ───────────────────────────────────────────────────────────────

export async function checkHealth(): Promise<boolean> {
  try {
    const res = await fetch("/health");
    return res.ok;
  } catch {
    return false;
  }
}

// ── Sessions ─────────────────────────────────────────────────────────────

export async function listSessions(): Promise<string[]> {
  return apiFetch<string[]>("/sessions");
}

export async function listSavedSessions(): Promise<SavedSessionInfo[]> {
  return apiFetch<SavedSessionInfo[]>("/sessions/saved");
}

export async function startSession(
  sessionId?: string,
  activeSkills?: string[],
): Promise<SessionInfo> {
  return apiFetch<SessionInfo>("/sessions", {
    method: "POST",
    body: JSON.stringify({
      sessionId,
      activeSkills: activeSkills ?? [],
    }),
  });
}

export async function loadSavedSession(sessionId: string): Promise<SessionInfo> {
  return apiFetch<SessionInfo>(`/sessions/load/${sessionId}`, {
    method: "POST",
  });
}

export async function getSessionSnapshot(
  sessionId: string,
): Promise<SessionSnapshot> {
  return apiFetch<SessionSnapshot>(`/sessions/${sessionId}/snapshot`);
}

export async function closeSession(sessionId: string): Promise<void> {
  await apiFetch(`/sessions/${sessionId}/close`, { method: "POST" });
}

export async function deleteSavedSession(sessionId: string): Promise<void> {
  await apiFetch(`/sessions/${sessionId}/delete`, { method: "POST" });
}

// ── Turns ────────────────────────────────────────────────────────────────

export async function sendTurn(
  sessionId: string,
  message: string,
): Promise<TurnResponse> {
  return apiFetch<TurnResponse>(`/sessions/${sessionId}/turns`, {
    method: "POST",
    body: JSON.stringify({ message }),
  });
}

export async function cancelTurn(sessionId: string): Promise<void> {
  await apiFetch(`/sessions/${sessionId}/cancel`, { method: "POST" });
}

// ── Approvals & Questions ────────────────────────────────────────────────

export async function approveTool(
  sessionId: string,
  requestId: string,
  approved: boolean,
  message?: string,
): Promise<{ consumed: boolean }> {
  return apiFetch(`/sessions/${sessionId}/approve`, {
    method: "POST",
    body: JSON.stringify({ requestId, approved, message }),
  });
}

export async function answerQuestion(
  sessionId: string,
  questionId: string,
  answer: string,
  custom?: string,
): Promise<{ consumed: boolean }> {
  return apiFetch(`/sessions/${sessionId}/question`, {
    method: "POST",
    body: JSON.stringify({ questionId, answer, custom }),
  });
}

// ── Models ───────────────────────────────────────────────────────────────

export async function listModels(): Promise<ModelInfo[]> {
  return apiFetch<ModelInfo[]>("/models");
}

export async function setSessionModel(
  sessionId: string,
  provider: string,
  model: string,
): Promise<void> {
  await apiFetch(`/sessions/${sessionId}/model`, {
    method: "POST",
    body: JSON.stringify({ provider, model }),
  });
}

// ── Config ───────────────────────────────────────────────────────────────

export interface ConfigSnapshot {
  model: { provider: string; name: string };
  projectDir: string;
  dataDir: string;
}

export async function getConfig(): Promise<ConfigSnapshot> {
  return apiFetch<ConfigSnapshot>("/config");
}

export { ApiError };
