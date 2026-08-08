import type {
  SessionInfo,
  SavedSessionInfo,
  ModelInfo,
  TurnResponse,
  SessionSnapshot,
  PermissionMode,
  AgentMode,
  PlanReviewDecision,
  SessionGoal,
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

// ── Permission mode ──────────────────────────────────────────────────────

export async function getPermissionMode(): Promise<PermissionMode> {
  const res = await apiFetch<{ mode: PermissionMode }>("/permission-mode");
  return res.mode;
}

export async function setPermissionMode(
  mode: PermissionMode,
): Promise<PermissionMode> {
  const res = await apiFetch<{ mode: PermissionMode }>("/permission-mode", {
    method: "POST",
    body: JSON.stringify({ mode }),
  });
  return res.mode;
}

// ── Agent mode / Plan ────────────────────────────────────────────────────

export async function getAgentMode(sessionId: string): Promise<AgentMode> {
  const res = await apiFetch<{ mode: AgentMode }>(
    `/sessions/${sessionId}/mode`,
  );
  return res.mode;
}

export async function enterPlanMode(sessionId: string): Promise<void> {
  await apiFetch(`/sessions/${sessionId}/plan/enter`, { method: "POST" });
}

export async function exitPlanMode(sessionId: string): Promise<void> {
  await apiFetch(`/sessions/${sessionId}/plan/exit`, { method: "POST" });
}

export async function submitPlanReview(
  sessionId: string,
  id: string,
  planId: string,
  decision: PlanReviewDecision,
  freeform?: string,
): Promise<{ consumed: boolean }> {
  return apiFetch(`/sessions/${sessionId}/plan/review`, {
    method: "POST",
    body: JSON.stringify({ id, planId, decision, freeform: freeform ?? "" }),
  });
}

// ── Sudo password ────────────────────────────────────────────────────────

export async function submitSudoPassword(
  sessionId: string,
  id: string,
  password: string,
): Promise<{ consumed: boolean }> {
  return apiFetch(`/sessions/${sessionId}/sudo`, {
    method: "POST",
    body: JSON.stringify({ id, password }),
  });
}

export async function cancelSudoPrompt(
  sessionId: string,
  id: string,
): Promise<{ consumed: boolean }> {
  return apiFetch(`/sessions/${sessionId}/sudo`, {
    method: "POST",
    body: JSON.stringify({ id, password: "" }),
  });
}

// ── Session rename ───────────────────────────────────────────────────────

export async function renameSession(
  sessionId: string,
  title: string,
): Promise<{ renamed: boolean }> {
  return apiFetch(`/sessions/${sessionId}/rename`, {
    method: "POST",
    body: JSON.stringify({ title }),
  });
}

// ── Goal ─────────────────────────────────────────────────────────────────

export async function setGoal(
  sessionId: string,
  objective: string,
  tokenBudget?: number,
): Promise<SessionGoal> {
  return apiFetch(`/sessions/${sessionId}/goal`, {
    method: "POST",
    body: JSON.stringify({ objective, tokenBudget }),
  });
}

export async function getGoal(sessionId: string): Promise<SessionGoal | null> {
  try {
    return await apiFetch<SessionGoal>(`/sessions/${sessionId}/goal`);
  } catch {
    return null;
  }
}

export async function clearGoal(sessionId: string): Promise<void> {
  await apiFetch(`/sessions/${sessionId}/goal`, { method: "DELETE" });
}

// ── Skills ───────────────────────────────────────────────────────────────

export async function setSessionSkills(
  sessionId: string,
  skills: string[],
): Promise<void> {
  await apiFetch(`/sessions/${sessionId}/skills`, {
    method: "POST",
    body: JSON.stringify({ skills }),
  });
}

export { ApiError };
