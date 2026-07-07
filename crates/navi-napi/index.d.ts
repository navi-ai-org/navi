export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

export type ToolKind = 'read' | 'write' | 'command' | 'custom';

export interface SessionInfo {
  id: string;
  projectDir: string;
  model: string;
  provider: string;
}

export interface TurnResponse {
  sessionId: string;
  text: string;
}

export interface HostToolDefinition {
  name: string;
  description: string;
  kind?: ToolKind;
  inputSchema?: JsonValue;
}

export interface HostToolInvocation {
  invocationId: string;
  input: JsonValue;
}

export interface HostToolResult {
  ok?: boolean;
  output?: JsonValue;
}

export interface LearningRuntimeConfig {
  maxConsecutiveErrors?: number;
  stopOnRepeatedTool?: boolean;
  compactObservationMaxBytes?: number;
  role?: string;
  style?: string;
  language?: string;
  keepAllAssessments?: boolean;
  exemptToolNames?: string[];
}

export type ContextSource =
  | 'File'
  | 'Project'
  | 'UserSelection'
  | 'CanvasNode'
  | 'StudyBlock'
  | 'FocusThread'
  | 'MaterialExcerpt'
  | 'SessionSummary'
  | 'Decision'
  | 'MemorySearch'
  | { Other: string };

export interface ContextPacket {
  id?: string | null;
  source: ContextSource;
  title?: string | null;
  content: string;
  priority?: number;
  metadata?: JsonValue;
}

export interface HookPayload {
  sessionId?: string;
  task?: string;
  output?: string;
  invocation?: JsonValue;
  result?: JsonValue;
}

export type RuntimeEvent = {
  version: number;
  kind: JsonValue;
};

export type ContentPart =
  | { type: 'text'; text: string }
  | { type: 'image'; media_type: string; data: string }
  | { type: 'audio'; media_type: string; data: string; name?: string | null }
  | { type: 'video'; media_type: string; data: string; name?: string | null }
  | { type: 'document'; media_type: string; data: string; name?: string | null };

export type HostToolHandler = (invocation: HostToolInvocation) => Promise<HostToolResult | JsonValue>;
export type HookHandler = (payload: HookPayload) => void;

export class NaviNapiEngineBuilder {
  constructor(projectDir: string);
  setLearningTutor(enabled?: boolean | null): void;
  configureLearning(config: LearningRuntimeConfig): void;
  onSessionStart(handler: HookHandler): void;
  onTurnStart(handler: HookHandler): void;
  onToolCall(handler: HookHandler): void;
  onToolResult(handler: HookHandler): void;
  onTurnEnd(handler: HookHandler): void;
  onSessionEnd(handler: HookHandler): void;
  hostTool(definition: HostToolDefinition, handler: HostToolHandler): void;
  build(): NaviNapiEngine;
}

export interface TurnOptions {
  contentParts?: ContentPart[] | JsonValue[];
  contextPackets?: JsonValue[];
  thinking?: 'max' | 'high' | 'medium' | 'low' | 'off' | 'adaptive';
}

export type SaveTarget = 'auto' | 'project' | 'global' | 'none';

export interface ModelSelectionResult {
  providerId: string;
  model: string;
  contextWindowTokens?: number;
  providerConfigured: boolean;
  savedTo?: string;
}

export interface ProviderSyncReport {
  savedTo?: string;
  updated: JsonValue[];
  failed: JsonValue[];
  skipped: JsonValue[];
}

export interface EngineConfig {
  model: { provider: string; name: string };
  attachmentModels?: {
    image?: { provider: string; name: string } | null;
    audio?: { provider: string; name: string } | null;
    video?: { provider: string; name: string } | null;
    document?: { provider: string; name: string } | null;
  };
  globalConfigPath?: string;
  projectConfigPath?: string;
  dataDir: string;
}

export interface ActiveSessions {
  sessionIds: string[];
  activeSessionId?: string | null;
}

export class NaviNapiEngine {
  constructor(projectDir: string, learningTutor?: boolean | null);
  static learningTutor(projectDir: string): NaviNapiEngine;
  startSession(sessionId?: string | null, projectDir?: string | null): Promise<SessionInfo>;
  sendTurn(sessionId: string, message: string, options?: TurnOptions): Promise<TurnResponse>;
  snapshotSession(sessionId: string): Promise<string>;
  closeSession(sessionId: string): Promise<boolean>;
  cancelTurn(sessionId: string): Promise<void>;
  resolveApproval(sessionId: string, approvalId: string, approved: boolean): Promise<boolean>;
  resolveQuestion(sessionId: string, response: JsonValue): Promise<boolean>;
  addContextPacket(sessionId: string, packet: ContextPacket): Promise<void>;
  listModels(): JsonValue;
  listTuiComponents(sessionId: string): string[];
  setModel(sessionId: string, provider: string, model: string): Promise<void>;
  selectModel(providerId: string, model: string, saveTarget?: SaveTarget): ModelSelectionResult;
  subscribeEvents(sessionId: string): NaviNapiEventStream;
  // Goals
  getGoal(sessionId: string): Promise<JsonValue>;
  setGoal(sessionId: string, objective: string, tokenBudget?: number | null): Promise<JsonValue>;
  clearGoal(sessionId: string): Promise<void>;
  // Background tasks
  listBackgroundCommands(sessionId: string): Promise<JsonValue>;
  pollBackgroundCommand(sessionId: string, taskId: string): Promise<JsonValue>;
  cancelBackgroundCommand(sessionId: string, taskId: string): Promise<JsonValue>;
  // Providers & credentials
  listProviderAccounts(): JsonValue;
  credentialStatus(providerId: string): JsonValue;
  setProviderApiKey(providerId: string, apiKey: string): void;
  deleteProviderApiKey(providerId: string): boolean;
  syncProviderModels(providerId: string, saveTarget?: SaveTarget): Promise<ProviderSyncReport>;
  syncModels(saveTarget?: SaveTarget): Promise<ProviderSyncReport>;
  // Usage
  usageReport(): Promise<JsonValue>;
  // Skills
  listSkills(): JsonValue;
  setSessionSkills(sessionId: string, skills: string[]): Promise<void>;
  // MCP
  listMcpServers(sessionId: string): JsonValue;
  listMcpTools(sessionId: string): string[];
  // Registry & plugins
  syncRegistry(force?: boolean): Promise<boolean>;
  reloadWasmPlugins(): Promise<string[]>;
  // Saved sessions
  listSavedSessions(): Promise<JsonValue>;
  loadSavedSession(sessionId: string): Promise<JsonValue>;
  deleteSavedSession(sessionId: string): Promise<boolean>;
  // Auto-memory
  memoryWrite(id: string, memoryType: string, name: string, description: string, body: string): void;
  memoryRead(id: string): JsonValue;
  memoryList(status?: string): JsonValue;
  memorySearch(query: string, limit?: number): JsonValue;
  memoryUpdate(id: string, name?: string, description?: string, body?: string, status?: string): void;
  memoryDelete(id: string): void;
  memoryCount(): number;
  memoryIndex(): string;
  // Permission mode
  getPermissionMode(): string;
  setPermissionMode(mode: string): void;
  // Session management
  sessionIds(): string[];
  loadedConfig(): EngineConfig;
}

export class NaviNapiEventStream {
  next(): Promise<RuntimeEvent | null>;
}
