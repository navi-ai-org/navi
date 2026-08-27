# navi_remote

Dart client for the **NAVI gateway** (`navi-server`).

Connects over HTTP/WebSocket (e.g. Tailscale) and covers the full gateway API
surface: sessions, plan/sudo/permission, memory, voice, plugins, credentials,
skills/MCP/routing, registry.

## Usage

```dart
import 'package:navi_remote/navi_remote.dart';

final engine = await NaviRemoteEngine.connect(
  host: '100.x.y.z',
  port: 9800,
  secret: 'my-secret',
);

final session = await engine.startSession(projectDir: '/path/on/server');
engine.subscribeEvents(session.id).listen((e) {
  print(e.kindName);
});

await engine.sendTurn(session.id, 'Hello');
await engine.setPermissionMode(PermissionMode.auto);
final skills = await engine.listSkills();
engine.dispose();
```

## API map (gateway)

### Core
| Method | Endpoint |
|--------|----------|
| `health` | `GET /health` |
| `loadedConfig` | `GET /config` |
| `usageReport` | `GET /usage` |
| `startSession` / `sessionIds` / `sessionInfo` / `closeSession` | sessions |
| `sendTurn` / `cancelTurn` | turns |
| `approve` / `deny` | approvals |
| `resolveQuestion` / `dismissQuestion` | questions |
| `setGoal` / `getGoal` / `clearGoal` | goals |
| `listSavedSessions` / `loadSavedSession` / `deleteSavedSession` | saved |
| `snapshotSession` | snapshot |
| `listModels` / `selectModel` / `setSessionModel` | models |
| `syncProviderModels` / `syncAllModels` | providers |
| `listSkills` / `setSessionSkills` | skills list + bind |
| `listMcpServers` / `listBackgroundCommands` | session mcp/bg |
| `subscribeEvents` | `WS /sessions/:id/events` |

### Session ops
| Method | Endpoint |
|--------|----------|
| `getSessionMode` / `enterPlanMode` / `exitPlanMode` / `resolvePlanReview` | plan |
| `resolveSudo` | sudo |
| `addContextPacket` / `rewindSession` | context / rewind |
| `updateGoalStatus` / `setGoalChecklist` / `updateGoalTaskStatus` | goal ext |
| `getBackgroundCommand` / `cancelBackgroundCommand` | bg tasks |
| `renameSavedSession` | rename |
| `getPermissionMode` / `setPermissionMode` | permission |

### Memory
`memoryStatus`, `memoryDoctor`, `memoryInit`, `memoryList`, `memoryWrite`,
`memoryCount`, `memoryIndex`, `memorySearch`, `memoryRead`, `memoryUpdate`,
`memoryDelete`, `memoryHistorySearch`, `memoryDream`, `memoryDistill`,
`memoryCheckpoint`, `memoryRebuildPreview`

### Voice
`voiceStatus`, `voiceDoctor`, `voiceProviders`, `voiceEngineInstalled`,
`voiceInit`, `voiceTranscribe`, `voiceStreamStart` / `Pcm` / `End` / `Cancel`,
`subscribeVoiceEvents`

### Plugins
`listPlugins`, `searchPlugins`, `getPlugin`,
`installPluginFromPath` / `Marketplace`,
`updatePluginFromPath` / `Marketplace`,
`removePlugin`, `reloadWasmPlugins`

### Auth
`listCredentials`, `getCredential`, `setProviderApiKey`, `deleteProviderApiKey`,
`listCredentialAccounts`, `addProviderAccount`, `selectProviderAccount`,
`deleteProviderAccount`, `providerSupportsDeviceOauth`, `startDeviceOauth`

### Skills / MCP / routing
`getSkill`, `saveSkill`, `deleteSkill`,
`getMcpConfig`, `setMcpConfig`, `setMcpEnabled`, `upsertMcpServer`,
`removeMcpServer`, `listSessionMcpTools`,
`getRouting`, `setAttachmentModel`, `clearAttachmentModel`,
`setBackgroundModel`, `clearBackgroundModel`

### Registry
`getRegistry`, `syncRegistry`

## Layout

```
lib/
  navi_remote.dart
  src/
    types.dart
    navi_remote_engine.dart   # core + HTTP/WS
    engine_session_ops.dart
    engine_memory.dart
    engine_voice.dart
    engine_plugins.dart
    engine_auth.dart
    engine_skills_mcp.dart
    engine_registry.dart
```
