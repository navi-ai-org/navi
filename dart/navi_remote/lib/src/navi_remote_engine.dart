/// Remote NAVI engine client — full navi-server (gateway) binding.
///
/// Connects over HTTP/WebSocket and covers the complete gateway surface:
/// sessions, session ops, memory, voice, plugins, credentials/OAuth,
/// skills/MCP/routing, registry.
import 'dart:async';
import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:web_socket_channel/web_socket_channel.dart';

import 'types.dart';

part 'engine_session_ops.dart';
part 'engine_memory.dart';
part 'engine_voice.dart';
part 'engine_plugins.dart';
part 'engine_auth.dart';
part 'engine_skills_mcp.dart';
part 'engine_registry.dart';

/// Remote connection to a NAVI gateway (`navi-server`).
///
/// Use [NaviRemoteEngine.connect] to create an instance. Always call
/// [dispose] when done.
class NaviRemoteEngine {
  final String _baseUrl;
  final String _secret;
  final http.Client _httpClient;
  bool _disposed = false;

  /// Open WebSocket channels owned by this client (closed on [dispose]).
  final List<WebSocketChannel> _channels = [];

  NaviRemoteEngine._(this._baseUrl, this._secret, this._httpClient);

  /// Base URL including scheme and port, e.g. `http://100.x.y.z:9800`.
  String get baseUrl => _baseUrl;

  /// Test / advanced constructor that injects an [http.Client].
  ///
  /// Does not perform a health check. Prefer [connect] for production.
  factory NaviRemoteEngine.forTesting({
    required String baseUrl,
    required String secret,
    required http.Client client,
  }) {
    return NaviRemoteEngine._(baseUrl, secret, client);
  }

  /// Connects to a NAVI server at the given [host] and [port].
  static Future<NaviRemoteEngine> connect({
    required String host,
    int port = 9800,
    required String secret,
    bool useTls = false,
    bool skipHealthCheck = false,
  }) async {
    var cleanHost = host.trim();
    final lowerHost = cleanHost.toLowerCase();
    if (lowerHost.startsWith('https://')) {
      useTls = true;
      cleanHost = cleanHost.substring(8);
    } else if (lowerHost.startsWith('http://')) {
      useTls = false;
      cleanHost = cleanHost.substring(7);
    }

    final slash = cleanHost.indexOf('/');
    if (slash != -1) cleanHost = cleanHost.substring(0, slash);

    var probeUri = Uri.tryParse('http://$cleanHost');
    if (probeUri == null &&
        cleanHost.contains(':') &&
        !(cleanHost.startsWith('[') && cleanHost.endsWith(']'))) {
      // IPv6 addresses need brackets before Uri parsing can extract host/port.
      cleanHost = '[$cleanHost]';
      probeUri = Uri.tryParse('http://$cleanHost');
    }
    if (probeUri != null && probeUri.host.isNotEmpty) {
      cleanHost = probeUri.host;
      if (probeUri.hasPort) port = probeUri.port;
    }

    final scheme = useTls ? 'https' : 'http';
    final baseUrl = '$scheme://$cleanHost:$port';
    final client = http.Client();
    final engine = NaviRemoteEngine._(baseUrl, secret, client);

    if (!skipHealthCheck) {
      try {
        final response = await client.get(Uri.parse('$baseUrl/health'));
        if (response.statusCode != 200) {
          throw NaviRemoteException(
            'Server health check failed: ${response.statusCode}',
            statusCode: response.statusCode,
          );
        }
      } catch (e) {
        client.close();
        if (e is NaviRemoteException) rethrow;
        throw NaviRemoteException('Cannot reach NAVI server at $baseUrl: $e');
      }
    }

    return engine;
  }

  /// Releases HTTP client and WebSocket connections.
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    for (final ch in _channels) {
      try {
        ch.sink.close();
      } catch (_) {}
    }
    _channels.clear();
    _httpClient.close();
  }

  // ── Health / config ──────────────────────────────────────────

  /// `GET /health` (no auth).
  Future<JsonMap> health() async {
    final response = await _httpClient.get(Uri.parse('$_baseUrl/health'));
    return _decodeResponse(response, method: 'GET', path: '/health');
  }

  /// `GET /config`.
  Future<EngineConfig> loadedConfig() async {
    final json = await _get('/config');
    return EngineConfig.fromJson(json);
  }

  /// `GET /usage`.
  Future<JsonMap> usageReport() => _get('/usage');

  // ── Sessions ─────────────────────────────────────────────────

  /// `POST /sessions`.
  ///
  /// **Important:** send each field only once. serde rejects JSON that contains
  /// both `project_dir` and `projectDir` (duplicate field) → "invalid request body".
  Future<SessionInfo> startSession({
    String? sessionId,
    String? projectDir,
    List<String>? activeSkills,
  }) async {
    final body = <String, dynamic>{};
    // Prefer camelCase only (server aliases accept these).
    if (sessionId != null && sessionId.isNotEmpty) {
      body['sessionId'] = sessionId;
    }
    if (projectDir != null && projectDir.isNotEmpty) {
      body['projectDir'] = projectDir;
    }
    if (activeSkills != null && activeSkills.isNotEmpty) {
      body['activeSkills'] = activeSkills;
    }
    final json = await _post('/sessions', body);
    return SessionInfo.fromJson(json);
  }

  /// `GET /sessions`.
  Future<List<String>> sessionIds() async {
    final json = await _get('/sessions');
    return asStringList(json['value'] ?? json);
  }

  /// `GET /sessions/:id`.
  Future<JsonMap> sessionInfo(String sessionId) =>
      _get('/sessions/$sessionId');

  /// `POST /sessions/:id/close`.
  Future<bool> closeSession(String sessionId) async {
    final json = await _post('/sessions/$sessionId/close', {});
    if (json['removed'] == true) return true;
    final status = json['status']?.toString();
    return status == 'closed';
  }

  /// `GET /sessions/:id/snapshot`.
  Future<JsonMap> snapshotSession(String sessionId) =>
      _get('/sessions/$sessionId/snapshot');

  /// `GET /sessions/saved`.
  Future<List<SavedSessionInfo>> listSavedSessions() async {
    final json = await _get('/sessions/saved');
    return asJsonMapList(json['value'] ?? json)
        .map(SavedSessionInfo.fromJson)
        .toList();
  }

  /// `POST /sessions/load/:id` — load + resume live session.
  Future<LoadedSession> loadSavedSession(String sessionId) async {
    final json = await _post('/sessions/load/$sessionId', {});
    return LoadedSession.fromJson(json);
  }

  /// `POST /sessions/:id/delete`.
  Future<bool> deleteSavedSession(String sessionId) async {
    final json = await _post('/sessions/$sessionId/delete', {});
    return json['deleted'] as bool? ?? false;
  }

  // ── Turns ────────────────────────────────────────────────────

  /// `POST /sessions/:id/turns`.
  /// [thinking] is the server `ThinkingConfig` wire value: a lowercase effort
  /// string (`max` / `high` / `medium` / `low` / `off`), matching
  /// `navi_core::ThinkingConfig` serde. Accepts [String] or a pre-built JSON
  /// value for forward compatibility.
  Future<TurnResponse> sendTurn(
    String sessionId,
    String message, {
    List<JsonMap>? contentParts,
    Object? thinking,
  }) async {
    final body = <String, dynamic>{'message': message};
    // Single key only — dual snake/camel breaks serde with "duplicate field".
    if (contentParts != null && contentParts.isNotEmpty) {
      body['contentParts'] = contentParts;
    }
    if (thinking != null) body['thinking'] = thinking;
    final json = await _post('/sessions/$sessionId/turns', body);
    return TurnResponse.fromJson(json);
  }

  /// `POST /sessions/:id/cancel`.
  Future<void> cancelTurn(String sessionId) async {
    await _post('/sessions/$sessionId/cancel', {});
  }

  // ── Approvals ────────────────────────────────────────────────

  /// `POST /sessions/:id/approve`.
  Future<bool> approve(
    String sessionId,
    String requestId, {
    String? message,
  }) async {
    final body = <String, dynamic>{
      'requestId': requestId,
      'approved': true,
    };
    if (message != null) body['message'] = message;
    final json = await _post('/sessions/$sessionId/approve', body);
    return json['consumed'] as bool? ?? false;
  }

  /// `POST /sessions/:id/deny`.
  Future<bool> deny(
    String sessionId,
    String requestId, {
    String? message,
  }) async {
    final body = <String, dynamic>{
      'requestId': requestId,
      'approved': false,
    };
    if (message != null) body['message'] = message;
    final json = await _post('/sessions/$sessionId/deny', body);
    return json['consumed'] as bool? ?? false;
  }

  // ── Questions ────────────────────────────────────────────────

  /// `POST /sessions/:id/question` with answer.
  Future<bool> resolveQuestion(
    String sessionId,
    String questionId,
    String answer, {
    String? custom,
  }) async {
    final body = <String, dynamic>{
      'questionId': questionId,
      'answer': answer,
    };
    if (custom != null) body['custom'] = custom;
    final json = await _post('/sessions/$sessionId/question', body);
    return json['consumed'] as bool? ?? false;
  }

  /// Dismiss question (empty answer).
  Future<bool> dismissQuestion(String sessionId, String questionId) =>
      resolveQuestion(sessionId, questionId, '');

  // ── Goals (basic) ────────────────────────────────────────────

  /// `POST /sessions/:id/goal`.
  Future<JsonMap> setGoal(
    String sessionId,
    String objective, {
    int? tokenBudget,
  }) async {
    final body = <String, dynamic>{'objective': objective};
    if (tokenBudget != null) body['tokenBudget'] = tokenBudget;
    return _post('/sessions/$sessionId/goal', body);
  }

  /// `GET /sessions/:id/goal`.
  Future<JsonMap?> getGoal(String sessionId) async {
    final json = await _get('/sessions/$sessionId/goal');
    if (json.containsKey('value') && json['value'] == null) return null;
    if (json.isEmpty) return null;
    return json;
  }

  /// `DELETE /sessions/:id/goal`.
  Future<void> clearGoal(String sessionId) async {
    await _delete('/sessions/$sessionId/goal');
  }

  // ── Skills (session bind + list) ─────────────────────────────

  /// `GET /skills`.
  Future<List<SkillInfo>> listSkills() async {
    final json = await _get('/skills');
    return asJsonMapList(json['value'] ?? json).map(SkillInfo.fromJson).toList();
  }

  /// `POST /sessions/:id/skills`.
  Future<void> setSessionSkills(String sessionId, List<String> skills) async {
    await _post('/sessions/$sessionId/skills', {'skills': skills});
  }

  // ── Session model ────────────────────────────────────────────

  /// `POST /sessions/:id/model`.
  Future<void> setSessionModel(
    String sessionId,
    String provider,
    String model,
  ) async {
    // Single keys only — dual camel/snake duplicates break serde.
    await _post('/sessions/$sessionId/model', {
      'provider': provider,
      'model': model,
    });
  }

  // ── Models ───────────────────────────────────────────────────

  /// `GET /models`.
  Future<List<ModelInfo>> listModels() async {
    final json = await _get('/models');
    return asJsonMapList(json['value'] ?? json).map(ModelInfo.fromJson).toList();
  }

  /// `POST /model/select`.
  Future<JsonMap> selectModel({
    required String providerId,
    required String model,
    String saveTarget = 'auto',
  }) async {
    return _post('/model/select', {
      'providerId': providerId,
      'model': model,
      'saveTarget': saveTarget,
    });
  }

  /// `POST /providers/sync`.
  Future<JsonMap> syncProviderModels({
    required String providerId,
    String saveTarget = 'auto',
  }) async {
    return _post('/providers/sync', {
      'providerId': providerId,
      'saveTarget': saveTarget,
    });
  }

  /// `POST /providers/sync-all?save=`.
  Future<JsonMap> syncAllModels({String saveTarget = 'auto'}) async {
    return _post('/providers/sync-all?save=$saveTarget', {});
  }

  // ── MCP (session live) ───────────────────────────────────────

  /// `GET /sessions/:id/mcp`.
  Future<List<JsonMap>> listMcpServers(String sessionId) async {
    final json = await _get('/sessions/$sessionId/mcp');
    return asJsonMapList(json['value'] ?? json);
  }

  // ── Background ───────────────────────────────────────────────

  /// `GET /sessions/:id/background`.
  Future<List<JsonMap>> listBackgroundCommands(String sessionId) async {
    final json = await _get('/sessions/$sessionId/background');
    return asJsonMapList(json['value'] ?? json);
  }

  // ── Events ───────────────────────────────────────────────────

  /// `WS /sessions/:id/events?secret=`.
  ///
  /// Cancelling the subscription closes the underlying WebSocket.
  /// Prefer [subscribeEventsReady] before [sendTurn] so early deltas are not lost.
  Stream<RuntimeEvent> subscribeEvents(String sessionId) {
    final channel = _connectWs('/sessions/$sessionId/events');
    return _eventStreamFromChannel(channel);
  }

  /// Like [subscribeEvents], but waits until the WebSocket is fully open.
  ///
  /// Call this (and await it) before [sendTurn] for reliable streaming on mobile.
  Future<Stream<RuntimeEvent>> subscribeEventsReady(String sessionId) async {
    final channel = _connectWs('/sessions/$sessionId/events');
    await channel.ready.timeout(
      const Duration(seconds: 10),
      onTimeout: () =>
          throw NaviRemoteException('WebSocket connect timeout for session events'),
    );
    return _eventStreamFromChannel(channel);
  }

  Stream<RuntimeEvent> _eventStreamFromChannel(WebSocketChannel channel) {
    late final StreamController<RuntimeEvent> controller;
    StreamSubscription? sub;

    controller = StreamController<RuntimeEvent>(
      onListen: () {
        sub = channel.stream.listen(
          (data) {
            try {
              final parsed = json.decode(data as String);
              if (parsed is Map) {
                final event = RuntimeEvent.fromJson(JsonMap.from(parsed));
                if (event.kind.isEmpty) return;
                if (!controller.isClosed) controller.add(event);
              }
            } catch (_) {
              // ignore malformed frames
            }
          },
          onError: (Object e, StackTrace st) {
            if (!controller.isClosed) controller.addError(e, st);
          },
          onDone: () {
            if (!controller.isClosed) controller.close();
          },
          cancelOnError: false,
        );
      },
      onCancel: () async {
        await sub?.cancel();
        try {
          await channel.sink.close();
        } catch (_) {}
        _channels.remove(channel);
      },
    );

    return controller.stream;
  }

  // ── HTTP helpers ─────────────────────────────────────────────

  Map<String, String> _headers() => {
        'Content-Type': 'application/json',
        'X-Navi-Secret': _secret,
      };

  String _query(Map<String, String?> params) {
    final entries = params.entries
        .where((e) => e.value != null && e.value!.isNotEmpty)
        .toList();
    if (entries.isEmpty) return '';
    return '?${entries.map((e) => '${Uri.encodeQueryComponent(e.key)}='
        '${Uri.encodeQueryComponent(e.value!)}').join('&')}';
  }

  WebSocketChannel _connectWs(String path) {
    final baseUri = Uri.parse(_baseUrl);
    final pathUri = Uri.parse(path.startsWith('/') ? path : '/$path');
    final wsScheme = baseUri.scheme == 'https' ? 'wss' : 'ws';
    final uri = baseUri.replace(
      scheme: wsScheme,
      path: pathUri.path,
      queryParameters: {
        ...pathUri.queryParameters,
        'secret': _secret,
      },
    );
    final channel = WebSocketChannel.connect(uri);
    _channels.add(channel);
    return channel;
  }

  Future<JsonMap> _get(String path) async {
    final response =
        await _httpClient.get(Uri.parse('$_baseUrl$path'), headers: _headers());
    return _decodeResponse(response, method: 'GET', path: path);
  }

  Future<JsonMap> _post(String path, [JsonMap? body]) async {
    final response = await _httpClient.post(
      Uri.parse('$_baseUrl$path'),
      headers: _headers(),
      body: json.encode(body ?? {}),
    );
    return _decodeResponse(response, method: 'POST', path: path);
  }

  Future<JsonMap> _put(String path, [JsonMap? body]) async {
    final response = await _httpClient.put(
      Uri.parse('$_baseUrl$path'),
      headers: _headers(),
      body: json.encode(body ?? {}),
    );
    return _decodeResponse(response, method: 'PUT', path: path);
  }

  Future<JsonMap> _patch(String path, [JsonMap? body]) async {
    final response = await _httpClient.patch(
      Uri.parse('$_baseUrl$path'),
      headers: _headers(),
      body: json.encode(body ?? {}),
    );
    return _decodeResponse(response, method: 'PATCH', path: path);
  }

  Future<JsonMap> _delete(String path, {JsonMap? body}) async {
    final request = http.Request('DELETE', Uri.parse('$_baseUrl$path'));
    request.headers.addAll(_headers());
    if (body != null) request.body = json.encode(body);
    final streamed = await _httpClient.send(request);
    final response = await http.Response.fromStream(streamed);
    return _decodeResponse(response, method: 'DELETE', path: path);
  }

  JsonMap _decodeResponse(
    http.Response response, {
    required String method,
    required String path,
  }) {
    if (response.statusCode == 401) {
      throw NaviRemoteException(
        'Authentication failed: invalid secret',
        statusCode: 401,
      );
    }
    if (response.statusCode >= 400) {
      String msg = '$method $path failed: ${response.statusCode}';
      try {
        final decoded = json.decode(response.body);
        if (decoded is Map && decoded['error'] != null) {
          msg = decoded['error'].toString();
        } else if (response.body.isNotEmpty) {
          msg = '$msg ${response.body}';
        }
      } catch (_) {
        if (response.body.isNotEmpty) msg = '$msg ${response.body}';
      }
      throw NaviRemoteException(msg, statusCode: response.statusCode);
    }
    if (response.body.isEmpty) return <String, dynamic>{};
    final decoded = json.decode(response.body);
    if (decoded is Map) {
      final map = JsonMap.from(decoded);
      if (map.containsKey('error') && map.length == 1) {
        throw NaviRemoteException(map['error'].toString());
      }
      return map;
    }
    return {'value': decoded};
  }
}
