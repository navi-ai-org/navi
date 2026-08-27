import 'dart:convert';
import 'dart:io';

import 'package:navi_remote/navi_remote.dart';
import 'package:test/test.dart';

HttpServer? _server;

Future<int> _startServer({required String expectedSecret}) async {
  _server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  _server!.listen((request) async {
    final path = request.uri.path;
    final query = request.uri.queryParameters;

    if (path == '/health') {
      request.response
        ..headers.contentType = ContentType.json
        ..write(jsonEncode({'status': 'ok'}));
      await request.response.close();
      return;
    }

    if (path.startsWith('/sessions/') && path.endsWith('/events')) {
      final secret = query['secret'];
      if (secret != expectedSecret) {
        request.response
          ..statusCode = HttpStatus.forbidden
          ..write('invalid secret: $secret');
        await request.response.close();
        return;
      }

      final ws = await WebSocketTransformer.upgrade(request);
      ws.add(jsonEncode({
        'version': 1,
        'kind': {
          'AssistantDelta': {'text': 'hello'},
        },
      }));
      await ws.close();
      return;
    }

    request.response.statusCode = HttpStatus.notFound;
    await request.response.close();
  });
  return _server!.port;
}

Future<void> _stopServer() async {
  await _server?.close();
  _server = null;
}

void main() {
  tearDown(() async => _stopServer());

  test('WebSocket event stream connects with URL-unsafe secret', () async {
    const secret = 'a&b=c#1';
    final port = await _startServer(expectedSecret: secret);

    final engine = await NaviRemoteEngine.connect(
      host: 'localhost',
      port: port,
      secret: secret,
    );
    addTearDown(engine.dispose);

    final events = await engine.subscribeEventsReady('session-1');
    final first = await events.first.timeout(const Duration(seconds: 2));

    expect(first.kindName, 'AssistantDelta');
    expect((first.kindData as Map)['text'], 'hello');
  });
}
