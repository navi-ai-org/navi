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

export class NaviNapiEngine {
  constructor(projectDir: string, learningTutor?: boolean | null);
  static learningTutor(projectDir: string): NaviNapiEngine;
  startSession(sessionId?: string | null): Promise<SessionInfo>;
  sendTurn(sessionId: string, message: string): Promise<TurnResponse>;
  snapshotSession(sessionId: string): Promise<string>;
  closeSession(sessionId: string): Promise<boolean>;
  cancelTurn(sessionId: string): Promise<void>;
  resolveApproval(sessionId: string, approvalId: string, approved: boolean): Promise<boolean>;
  addContextPacket(sessionId: string, packet: ContextPacket): Promise<void>;
  listModels(): JsonValue;
  listTuiComponents(sessionId: string): string[];
  setModel(sessionId: string, provider: string, model: string): Promise<void>;
  subscribeEvents(sessionId: string): NaviNapiEventStream;
}

export class NaviNapiEventStream {
  next(): Promise<RuntimeEvent | null>;
}
