/// Remote NAVI client library.
///
/// Connects to a navi-server instance over HTTP/WebSocket and provides
/// the same API surface as the local navi_agent package.
///
/// ```dart
/// final engine = await NaviRemoteEngine.connect(
///   host: '100.x.y.z',
///   port: 9800,
///   secret: 'my-secret',
/// );
/// final session = await engine.startSession();
/// final response = await engine.sendTurn(session.id, 'Hello!');
/// print(response.text);
/// engine.dispose();
/// ```
library navi_remote;

export 'src/navi_remote_engine.dart';
export 'src/types.dart';
