# @navi-agent/napi

Node.js bindings for the NAVI runtime SDK.

[![npm](https://img.shields.io/npm/v/@navi-agent/napi)](https://www.npmjs.com/package/@navi-agent/napi)

> **Full documentation:** [docs/navi-napi-guide.md](docs/navi-napi-guide.md)

## Install

```sh
npm install @navi-agent/napi
```

Prebuilt native binaries are included automatically for Linux (x64/arm64),
macOS (x64/arm64), and Windows (x64). No Rust toolchain required.

## Usage

```ts
import { NaviNapiEngineBuilder } from '@navi-agent/napi';

const builder = new NaviNapiEngineBuilder(process.cwd());
builder.hostTool(
  { name: 'lookup_docs', description: 'Look up docs.', kind: 'read' },
  async ({ input }) => ({ ok: true, output: { input } }),
);

const engine = builder.build();

const session = await engine.startSession();
const response = await engine.sendTurn(session.id, 'Hello!');
console.log(response.text);
await engine.closeSession(session.id);
```

## API Surface

The binding exposes the full `NaviEngine` SDK surface:

| Category | Methods |
|----------|---------|
| Sessions | `startSession`, `closeSession`, `snapshotSession`, `sessionIds` |
| Turns | `sendTurn` (with multimodal content, context packets, thinking), `cancelTurn` |
| Events | `subscribeEvents` → `stream.next()` |
| Goals | `getGoal`, `setGoal`, `clearGoal` |
| Questions | `resolveQuestion` |
| Approvals | `resolveApproval` |
| Background tasks | `listBackgroundCommands`, `pollBackgroundCommand`, `cancelBackgroundCommand` |
| Models | `listModels`, `setModel`, `selectModel` |
| Providers & credentials | `listProviderAccounts`, `credentialStatus`, `setProviderApiKey`, `deleteProviderApiKey` |
| Provider sync | `syncProviderModels`, `syncModels` |
| Usage | `usageReport` |
| Skills | `listSkills`, `setSessionSkills` |
| MCP | `listMcpServers`, `listMcpTools` |
| Saved sessions | `listSavedSessions`, `loadSavedSession`, `deleteSavedSession` |
| Registry & plugins | `syncRegistry`, `reloadWasmPlugins` |
| Config | `loadedConfig` |
| Host tools | `builder.hostTool(definition, handler)` |
| Lifecycle hooks | `onSessionStart`, `onTurnStart`, `onToolCall`, `onToolResult`, `onTurnEnd`, `onSessionEnd` |

A panic hook is installed on module load to prevent agent runtime panics
from crashing the host Node.js/Electron process.

## Development

To build the native addon from source (requires [Rust](https://rustup.rs)):

```sh
cd crates/navi-napi
npm run build   # debug build
npm test        # build + smoke tests
```
