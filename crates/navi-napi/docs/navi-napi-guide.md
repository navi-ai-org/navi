# @navi-agent/napi — TypeScript / Node.js Binding Guide

`@navi-agent/napi` is the official Node.js binding for the NAVI agent runtime SDK. It exposes the full NAVI engine — sessions, turns, tools, events, context injection, and lifecycle hooks — as native N-API classes consumable from JavaScript and TypeScript.

---

## Table of Contents

- [Installation](#installation)
- [Quick Start](#quick-start)
- [Engine Construction](#engine-construction)
  - [Simple Constructor](#simple-constructor)
  - [Builder Pattern](#builder-pattern)
- [Sessions](#sessions)
  - [Starting a Session](#starting-a-session)
  - [Closing a Session](#closing-a-session)
  - [Snapshotting a Session](#snapshotting-a-session)
- [Sending Turns](#sending-turns)
- [Cancelling a Turn](#cancelling-a-turn)
- [Event Streaming](#event-streaming)
  - [Event Types](#event-types)
- [Context Packets](#context-packets)
  - [Context Sources](#context-sources)
- [Model Management](#model-management)
- [Tool Approvals](#tool-approvals)
- [Host Tools](#host-tools)
  - [Defining a Host Tool](#defining-a-host-tool)
  - [Host Tool Input and Output](#host-tool-input-and-output)
- [Lifecycle Hooks](#lifecycle-hooks)
- [Learning Tutor Mode](#learning-tutor-mode)
  - [Learning Configuration](#learning-configuration)
- [Type Reference](#type-reference)
- [Platform Notes](#platform-notes)
- [Building from Source](#building-from-source)

---

## Installation

The package is not published to npm. It is a local path dependency built from the NAVI workspace.

```bash
# From the workspace root (where Cargo.toml lives)
cd crates/navi-napi
npm install   # installs no external deps; native binary is built separately
npm run build # compiles the Rust native addon
```

The build step runs `cargo build -p navi-napi` and copies the resulting native library into the package directory with a platform-specific filename (e.g. `navi.linux-x64.node`).

**Requirements:**

- Node.js ≥ 18
- Rust toolchain with the `navi-napi` crate buildable
- The full NAVI workspace (it depends on `navi-core` and `navi-sdk`)

---

## Quick Start

```ts
import { NaviNapiEngine, NaviNapiEngineBuilder } from '@navi-agent/napi';

// 1. Create an engine with the builder
const builder = new NaviNapiEngineBuilder('/path/to/your/project');

// 2. (Optional) Register host tools and hooks
builder.hostTool(
  {
    name: 'lookup_docs',
    description: 'Search project documentation by keyword.',
    kind: 'read',
    inputSchema: {
      type: 'object',
      properties: { query: { type: 'string' } },
      required: ['query'],
    },
  },
  async ({ input }) => {
    // your tool logic here
    return { ok: true, output: { results: ['doc1.md', 'doc2.md'] } };
  },
);

// 3. Build the engine
const engine = builder.build();

// 4. Start a session, send a turn, read the response
const session = await engine.startSession();
console.log(`Session started: ${session.id} (model: ${session.model})`);

const response = await engine.sendTurn(session.id, 'Explain the project structure.');
console.log(response.text);

// 5. Clean up
await engine.closeSession(session.id);
```

---

## Engine Construction

### Simple Constructor

For quick usage without host tools or hooks:

```ts
const engine = new NaviNapiEngine('/path/to/project');

// Or in learning-tutor mode:
const engine = NaviNapiEngine.learningTutor('/path/to/project');
```

### Builder Pattern

The builder gives you full control over host tools, lifecycle hooks, and learning configuration before the engine is created:

```ts
const builder = new NaviNapiEngineBuilder('/path/to/project');

// Configure learning mode (optional)
builder.setLearningTutor(true);
builder.configureLearning({
  language: 'pt-BR',
  style: 'socratico',
  maxConsecutiveErrors: 6,
  keepAllAssessments: true,
});

// Register host tools (optional, see Host Tools section)
builder.hostTool(definition, handler);

// Register lifecycle hooks (optional, see Lifecycle Hooks section)
builder.onSessionStart((payload) => { /* ... */ });
builder.onTurnStart((payload) => { /* ... */ });
builder.onToolCall((payload) => { /* ... */ });
builder.onToolResult((payload) => { /* ... */ });
builder.onTurnEnd((payload) => { /* ... */ });
builder.onSessionEnd((payload) => { /* ... */ });

// Build the engine — host tools and hooks become immutable after this
const engine = builder.build();
```

---

## Sessions

### Starting a Session

```ts
// Auto-generated session ID
const session = await engine.startSession();
// session => { id: string, projectDir: string, model: string, provider: string }

// Or provide your own session ID
const session = await engine.startSession('my-custom-session-id');
```

The returned `SessionInfo` tells you the resolved model and provider for this session.

### Closing a Session

```ts
const closed = await engine.closeSession(sessionId);
// closed => boolean (true if the session existed and was closed)
```

### Snapshotting a Session

Persists the current session state to disk and returns the snapshot path:

```ts
const snapshotPath = await engine.snapshotSession(sessionId);
console.log(`Session saved to: ${snapshotPath}`);
```

---

## Sending Turns

A turn is a single user→assistant exchange. The engine processes the message, runs tools as needed, and returns the final text response.

```ts
const response = await engine.sendTurn(session.id, 'What does the justfile do?');
// response => { sessionId: string, text: string }

console.log(response.text);
```

`sendTurn` is **async and blocking** — it waits for the full turn to complete (including all tool-call iterations) before returning. For streaming updates during the turn, use [Event Streaming](#event-streaming).

---

## Cancelling a Turn

If a turn is in progress (e.g. the user wants to interrupt a long-running tool loop):

```ts
await engine.cancelTurn(sessionId);
```

Cancellation is cooperative. The current tool iteration finishes, and the turn ends early.

---

## Event Streaming

To observe fine-grained events during a turn (streaming text deltas, tool calls, approvals, token usage), subscribe to the event stream **before** sending the turn:

```ts
const stream = engine.subscribeEvents(session.id);

// Send the turn asynchronously
const turnPromise = engine.sendTurn(session.id, 'Analyze the codebase.');

// Read events as they arrive
let event = await stream.next();
while (event !== null) {
  const kind = event.kind;

  if (kind.AssistantDelta) {
    process.stdout.write(kind.AssistantDelta.text);
  } else if (kind.ToolRequested) {
    console.log(`\n[Tool requested: ${kind.ToolRequested.tool_name}]`);
  } else if (kind.TokensUpdated) {
    console.log(`\n[Tokens: in=${kind.TokensUpdated.input_tokens}, out=${kind.TokensUpdated.output_tokens}]`);
  }

  event = await stream.next();
}

// The turn response is also available
const response = await turnPromise;
```

`stream.next()` returns `null` when the session ends or the stream is exhausted.

### Event Types

Each event has `{ version: number, kind: RuntimeEventKind }`. The `kind` field is one of:

| Kind | Description |
|------|-------------|
| `SessionStarted { session_id }` | A new session was created |
| `TurnStarted { turn_id }` | A turn started |
| `AssistantDelta { text }` | Streaming text from the assistant |
| `AssistantThinkingDelta { text }` | Streaming reasoning/thinking text |
| `ToolRequested(ToolInvocation)` | The assistant wants to call a tool |
| `ApprovalRequired(ApprovalRequest)` | A tool needs user approval |
| `ApprovalResolved(ApprovalDecision)` | An approval was accepted or denied |
| `ToolStarted(ToolInvocation)` | A tool began executing |
| `ToolCompleted(ToolResult)` | A tool finished |
| `ContextUpdated` | Context was refreshed |
| `TokensUpdated { input_tokens, output_tokens, ... }` | Token usage reported |
| `SessionSaved { session_id }` | Session persisted to disk |
| `TurnCompleted { turn_id, text }` | Turn finished with final text |
| `SessionFinished { session_id }` | Session ended |
| `MicroCompactApplied { messages_cleared }` | Stale tool results cleared |
| `AutoCompactStarted` | Auto-compaction started |
| `AutoCompactCompleted { tokens_saved }` | Auto-compaction finished |
| `AutoCompactFailed { reason }` | Auto-compaction failed |
| `GoalUpdated { ... }` | Session goal status changed |
| `Error { message }` | An error occurred |
| `SubagentActivity { invocation_id, message }` | Nested subagent activity |
| `HarnessTrace(...)` | Diagnostic harness data |
| `HarnessStopped { reason, message, ... }` | Harness halted a turn |
| `PatchProposed(...)` | A file patch was proposed |
| `CapabilityRecorded(...)` | Policy capability event |
| `QuestionRequired(...)` | Interactive user choice requested |
| `QuestionResolved(...)` | User choice resolved |

---

## Context Packets

Context packets let you inject external information into the agent's conversation — file content, canvas nodes, study blocks, memory search results, and more.

```ts
await engine.addContextPacket(session.id, {
  source: 'File',
  title: 'README.md',
  content: '# My Project\nThis is the project readme...',
  priority: 10,
});

// With metadata
await engine.addContextPacket(session.id, {
  id: 'canvas-node-42',
  source: 'CanvasNode',
  title: 'Architecture diagram',
  content: 'The system uses a layered architecture...',
  priority: 5,
  metadata: { nodeId: '42', x: 100, y: 200 },
});
```

### Context Sources

The `source` field identifies where the context came from:

| Source | Description |
|--------|-------------|
| `'File'` | Content from a file on disk |
| `'Project'` | Project-level metadata or state |
| `'UserSelection'` | Text selected by the user in an editor/UI |
| `'CanvasNode'` | A node from a visual canvas (e.g. NAVI Tutor) |
| `'StudyBlock'` | A study block from a learning workspace |
| `'FocusThread'` | The user's current area of focus |
| `'MaterialExcerpt'` | An excerpt from study material or docs |
| `'SessionSummary'` | Summary from a previous session |
| `'Decision'` | A recorded decision or rationale |
| `'MemorySearch'` | Results from memory/knowledge-base search |
| `{ Other: 'custom-tag' }` | A custom source with an arbitrary string tag |

---

## Model Management

```ts
// List all available models across all configured providers
const models = engine.listModels();
// models => JsonValue (array of provider/model entries)

// Change the model for an active session
await engine.setModel(session.id, 'anthropic', 'claude-sonnet-4-20250514');
```

---

## Tool Approvals

When the security policy requires approval for a tool call, the engine emits an `ApprovalRequired` event. You resolve it programmatically:

```ts
const stream = engine.subscribeEvents(session.id);

// ...during event processing:
if (event.kind.ApprovalRequired) {
  const { approval_id, tool_name } = event.kind.ApprovalRequired;
  console.log(`Tool "${tool_name}" needs approval`);

  // Approve or deny
  const approved = await engine.resolveApproval(session.id, approval_id, true);
  // approved => boolean
}
```

---

## Host Tools

Host tools let your TypeScript code provide custom tools to the NAVI agent. The agent can call these tools as part of its reasoning loop, and your handler executes the logic.

### Defining a Host Tool

```ts
builder.hostTool(
  {
    // Required
    name: 'search_knowledge_base',
    description: 'Search the local knowledge base for relevant articles.',

    // Optional: 'read' | 'write' | 'command' | 'custom'
    // Defaults to 'custom' if omitted. Affects security policy evaluation.
    kind: 'read',

    // Optional: JSON Schema describing the tool's input
    inputSchema: {
      type: 'object',
      properties: {
        query: { type: 'string', description: 'Search query' },
        limit: { type: 'number', description: 'Max results', default: 5 },
      },
      required: ['query'],
    },
  },
  async (invocation) => {
    // invocation => { invocationId: string, input: JsonValue }

    const { query, limit } = invocation.input;
    const results = await mySearchFunction(query, limit ?? 5);

    return {
      ok: true,
      output: { results },
    };
  },
);
```

### Host Tool Input and Output

**Invocation** (what your handler receives):

```ts
interface HostToolInvocation {
  invocationId: string;  // Unique ID for this specific call
  input: JsonValue;      // The arguments the agent passed
}
```

**Result** (what your handler returns):

```ts
interface HostToolResult {
  ok?: boolean;          // true = success, false = error (default: true if omitted)
  output?: JsonValue;    // The result data sent back to the agent
}
```

You can also return a plain `JsonValue` directly — it will be treated as `{ ok: true, output: yourValue }`.

---

## Lifecycle Hooks

Hooks let your application observe the session lifecycle without blocking the engine. They fire asynchronously and receive a payload object.

```ts
builder.onSessionStart((payload) => {
  console.log(`Session started: ${payload.sessionId}`);
});

builder.onTurnStart((payload) => {
  console.log(`Turn started for session ${payload.sessionId}: ${payload.task}`);
});

builder.onToolCall((payload) => {
  console.log(`Tool called:`, payload.invocation);
});

builder.onToolResult((payload) => {
  console.log(`Tool result:`, payload.result);
});

builder.onTurnEnd((payload) => {
  console.log(`Turn ended. Output: ${payload.output?.slice(0, 100)}...`);
});

builder.onSessionEnd((payload) => {
  console.log(`Session ended: ${payload.sessionId}`);
});
```

**Hook Payload:**

```ts
interface HookPayload {
  sessionId?: string;
  task?: string;
  output?: string;
  invocation?: JsonValue;
  result?: JsonValue;
}
```

Not all fields are populated for every hook — only the fields relevant to that lifecycle stage.

---

## Learning Tutor Mode

NAVI can run in a specialized "learning tutor" mode designed for educational applications (e.g. NAVI Tutor). In this mode, the engine applies a learning-specific system prompt, assessment tracking, and pedagogical behavior.

```ts
// Quick way:
const engine = NaviNapiEngine.learningTutor('/path/to/project');

// Or via builder:
const builder = new NaviNapiEngineBuilder('/path/to/project');
builder.setLearningTutor(true);
builder.configureLearning({ /* ... */ });
const engine = builder.build();
```

### Learning Configuration

```ts
interface LearningRuntimeConfig {
  // Stop the turn after this many consecutive tool errors (default: engine default)
  maxConsecutiveErrors?: number;

  // Stop if the same tool is called with the same input repeatedly
  stopOnRepeatedTool?: boolean;

  // Max bytes for tool observation content in the learning context
  compactObservationMaxBytes?: number;

  // System role description for the learning agent
  role?: string;

  // Teaching style (e.g. 'socratico', 'direct', 'collaborative')
  style?: string;

  // Language for the agent's responses (e.g. 'pt-BR', 'en', 'es')
  language?: string;

  // Keep all assessment results in context (don't compact them)
  keepAllAssessments?: boolean;

  // Tool names that are exempt from compaction/error limits
  exemptToolNames?: string[];
}
```

Example:

```ts
builder.configureLearning({
  language: 'pt-BR',
  style: 'socratico',
  maxConsecutiveErrors: 6,
  keepAllAssessments: true,
  exemptToolNames: ['questionario', 'avaliacao'],
});
```

---

## Type Reference

All types are exported from `@navi-agent/napi`:

```ts
// Engine classes
NaviNapiEngine        // Main engine — send turns, manage sessions
NaviNapiEngineBuilder // Builder — configure tools, hooks, learning before build
NaviNapiEventStream   // Async iterator for runtime events

// Data types
SessionInfo           // { id, projectDir, model, provider }
TurnResponse          // { sessionId, text }
RuntimeEvent          // { version: number, kind: JsonValue }
ContextPacket         // { id?, source, title?, content, priority?, metadata? }
ContextSource         // 'File' | 'Project' | ... | { Other: string }

// Host tool types
HostToolDefinition    // { name, description, kind?, inputSchema? }
HostToolInvocation    // { invocationId, input }
HostToolResult        // { ok?, output? }
ToolKind              // 'read' | 'write' | 'command' | 'custom'

// Learning types
LearningRuntimeConfig // { maxConsecutiveErrors?, style?, language?, ... }

// Hook types
HookPayload           // { sessionId?, task?, output?, invocation?, result? }
HookHandler           // (payload: HookPayload) => void
HostToolHandler       // (invocation: HostToolInvocation) => Promise<HostToolResult | JsonValue>

// Utility
JsonValue             // null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue }
```

---

## Platform Notes

The native addon is a platform-specific shared library:

| Platform | Library name |
|----------|-------------|
| Linux x64 | `navi.linux-x64.node` |
| macOS (arm64/x64) | `navi.darwin-arm64.node` / `navi.darwin-x64.node` |
| Windows | `navi.win32-x64.node` |

The loader (`index.js`) checks these locations in order:

1. `NAVI_NAPI_BINARY` environment variable (absolute path)
2. `navi.<platform>-<arch>.node` in the package directory
3. `navi.node` in the package directory
4. `target/release/libnavi_napi.so` (or `.dylib`/`.dll`) in the workspace
5. `target/debug/libnavi_napi.so` (or `.dylib`/`.dll`) in the workspace

Set `NAVI_NAPI_BINARY` to use a custom-built binary:

```bash
NAVI_NAPI_BINARY=/opt/navi-custom/libnavi_napi.so node my-app.mjs
```

---

## Building from Source

```bash
# Debug build (faster compile, slower runtime)
cd crates/navi-napi
npm run build

# Release build (slower compile, faster runtime)
npm run build -- --release
# or
NODE_ENV=production npm run build
```

The build script runs `cargo build -p navi-napi` and copies the output to the package directory.

**Running tests:**

```bash
# Build + run smoke tests
npm test

# TypeScript type checking only (no native binary needed for this)
npm run test:types
```

---

## Complete Example

```ts
import {
  NaviNapiEngineBuilder,
  type RuntimeEvent,
  type HostToolInvocation,
  type HostToolResult,
} from '@navi-agent/napi';

// --- Setup ---

const builder = new NaviNapiEngineBuilder(process.cwd());

// Configure learning mode
builder.configureLearning({
  language: 'en',
  style: 'collaborative',
  maxConsecutiveErrors: 3,
});

// Register a custom tool
builder.hostTool(
  {
    name: 'get_file_summary',
    description: 'Returns a one-line summary of a file.',
    kind: 'read',
    inputSchema: {
      type: 'object',
      properties: { path: { type: 'string' } },
      required: ['path'],
    },
  },
  async (inv: HostToolInvocation): Promise<HostToolResult> => {
    const filePath = (inv.input as any).path;
    // In real code, read and summarize the file
    return { ok: true, output: { summary: `Summary of ${filePath}` } };
  },
);

// Observe tool calls
builder.onToolCall((payload) => {
  console.error(`[hook] tool called:`, JSON.stringify(payload.invocation));
});

const engine = builder.build();

// --- Run a session ---

async function main() {
  const session = await engine.startSession();
  console.log(`Session: ${session.id} | Model: ${session.provider}/${session.model}`);

  // Subscribe to events for streaming
  const stream = engine.subscribeEvents(session.id);

  // Send a turn (non-blocking event consumption)
  const turnPromise = engine.sendTurn(session.id, 'Summarize the project.');

  // Consume streaming events
  let event: RuntimeEvent | null = await stream.next();
  while (event !== null) {
    const kind = event.kind as any;

    if (kind.AssistantDelta) {
      process.stdout.write(kind.AssistantDelta.text);
    }
    if (kind.TurnCompleted) {
      console.log('\n--- Turn complete ---');
    }

    event = await stream.next();
  }

  const result = await turnPromise;
  console.log(`\nFinal response length: ${result.text.length}`);

  // Save and close
  await engine.snapshotSession(session.id);
  await engine.closeSession(session.id);
}

main().catch(console.error);
```
