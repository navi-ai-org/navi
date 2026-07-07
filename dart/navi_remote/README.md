# navi_remote

Remote Dart client for the NAVI agent engine.

Connects to a `navi-server` instance over HTTP/WebSocket (e.g. via Tailscale) and provides the same API surface as the local `navi_agent` FFI package.

## Usage

```dart
import 'package:navi_remote/navi_remote.dart';

final engine = await NaviRemoteEngine.connect(
  host: '100.x.y.z',  // Tailscale IP of your PC
  port: 9800,
  secret: 'my-secret',
);

// Start a session
final session = await engine.startSession();
print('Session: ${session.id}');

// Send a turn
final response = await engine.sendTurn(session.id, 'Explain this code');
print(response.text);

// Stream events
engine.subscribeEvents(session.id).listen((event) {
  print('Event: ${event.kindName}');
});

// Clean up
engine.dispose();
```

## API

### NaviRemoteEngine

| Method | Description |
|--------|-------------|
| `connect()` | Connect to server |
| `dispose()` | Release resources |
| `startSession()` | Start agent session |
| `sessionIds()` | List active sessions |
| `closeSession()` | Close session |
| `sendTurn()` | Send message, get response |
| `cancelTurn()` | Cancel active turn |
| `resolveApproval()` | Resolve tool approval |
| `listModels()` | List available models |
| `loadedConfig()` | Get server config |
| `subscribeEvents()` | Stream RuntimeEvents |
