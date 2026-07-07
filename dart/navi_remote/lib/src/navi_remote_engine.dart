/// Remote NAVI engine client.
///
/// Connects to a navi-server instance over HTTP/WebSocket and provides
/// the same API surface as the local NaviEngine from navi_agent.
import 'dart:async';
import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:web_socket_channel/web_socket_channel.dart';

import 'types.dart';

/// Remote connection to a NAVI server.
///
/// Use [NaviRemoteEngine.connect] to create an instance. Always call
/// [dispose] when done to clean up HTTP clients and WebSocket connections.
class NaviRemoteEngine {
  final String _baseUrl;
  final String _secret;
  final http.Client _httpClient;
  bool _disposed = false;

  NaviRemoteEngine._(this._baseUrl, this._secret, this._httpClient);

  /// Connects to a NAVI server at the given [host] and [port].
  ///
  /// [secret] is the shared secret for authentication (X-Navi-Secret header).
  /// Set [useTls] to true if the server uses HTTPS.
  static Future<NaviRemoteEngine> connect({
    required String host,
    int port = 9800,
    required String secret,
    bool useTls = false,
  }) async {
    final scheme = useTls ? 'https' : 'http';
    final baseUrl = '$scheme://$host:$port';
    final client = http.Client();

    final engine = NaviRemoteEngine._(baseUrl, secret, client);

    // Verify connectivity with a health check.
    try {
      final response = await client.get(Uri.parse('$baseUrl/health'));
      if (response.statusCode != 200) {
        throw NaviRemoteException(
          'Server health check failed: ${response.statusCode}',
        );
      }
    } catch (e) {
      client.close();
      if (e is NaviRemoteException) rethrow;
      throw NaviRemoteException('Cannot reach NAVI server at $baseUrl: $e');
    }

    return engine;
  }

  /// Releases all resources (HTTP client, WebSocket connections).
  void dispose() {
    if (!_disposed) {
      _disposed = true;
      _httpClient.close();
    }
  }

  // ── Sessions ─────────────────────────────────────────────────

  /// Starts a new agent session on the server.
  Future<SessionInfo> startSession({
    String? sessionId,
    String? projectDir,
    List<String>? activeSkills,
  }) async {
    final body = <String, dynamic>{};
    if (sessionId != null) body['sessionId'] = sessionId;
    if (projectDir != null) body['projectDir'] = projectDir;
    if (activeSkills != null && activeSkills.isNotEmpty) {
      body['activeSkills'] = activeSkills;
    }

    final json = await _post('/sessions', body);
    return SessionInfo.fromJson(json);
  }

  /// Returns the IDs of all active sessions on the server.
  Future<List<String>> sessionIds() async {
    final json = await _get('/sessions');
    return (json['value'] as List<dynamic>? ?? json as List<dynamic>)
        .map((e) => e as String)
        .toList();
  }

  /// Gets info about a specific active session.
  Future<Map<String, dynamic>> sessionInfo(String sessionId) async {
    return _get('/sessions/$sessionId');
  }

  /// Closes an active session on the server.
  Future<bool> closeSession(String sessionId) async {
    final json = await _post('/sessions/$sessionId/close', {});
    return json['removed'] as bool? ?? json['status'] == 'closed';
  }

  /// Takes a point-in-time snapshot of a session.
  Future<Map<String, dynamic>> snapshotSession(String sessionId) async {
    return _get('/sessions/$sessionId/snapshot');
  }

  // ── Saved sessions ───────────────────────────────────────────

  /// Lists all persisted sessions with titles and timestamps.
  Future<List<Map<String, dynamic>>> listSavedSessions() async {
    final json = await _get('/sessions/saved');
    final list = json['value'] as List<dynamic>? ?? json as List<dynamic>;
    return list.cast<Map<String, dynamic>>();
  }

  /// Loads a persisted session snapshot by ID.
  Future<Map<String, dynamic>> loadSavedSession(String sessionId) async {
    return _post('/sessions/load/$sessionId', {});
  }

  /// Deletes a persisted session. Returns true if a session was removed.
  Future<bool> deleteSavedSession(String sessionId) async {
    final json = await _post('/sessions/$sessionId/delete', {});
    return json['deleted'] as bool? ?? false;
  }

  // ── Turns ────────────────────────────────────────────────────

  /// Sends a user message to an active session and waits for the response.
  Future<TurnResponse> sendTurn(
    String sessionId,
    String message, {
    List<Map<String, dynamic>>? contentParts,
    Map<String, dynamic>? thinking,
  }) async {
    final body = <String, dynamic>{'message': message};
    if (contentParts != null && contentParts.isNotEmpty) {
      body['contentParts'] = contentParts;
    }
    if (thinking != null) body['thinking'] = thinking;

    final json = await _post('/sessions/$sessionId/turns', body);
    return TurnResponse.fromJson(json);
  }

  /// Cancels the currently active turn for the given session.
  Future<void> cancelTurn(String sessionId) async {
    await _post('/sessions/$sessionId/cancel', {});
  }

  // ── Approvals ────────────────────────────────────────────────

  /// Approves a pending tool approval request.
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

  /// Denies a pending tool approval request.
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

  /// Resolves a pending interactive question with an answer.
  Future<bool> resolveQuestion(
    String sessionId,
    String questionId,
    String answer,
  ) async {
    final json = await _post('/sessions/$sessionId/question', {
      'questionId': questionId,
      'answer': answer,
    });
    return json['consumed'] as bool? ?? false;
  }

  /// Dismisses a pending interactive question without answering.
  Future<bool> dismissQuestion(String sessionId, String questionId) async {
    final json = await _post('/sessions/$sessionId/question', {
      'questionId': questionId,
      'answer': '',
    });
    return json['consumed'] as bool? ?? false;
  }

  // ── Goals ────────────────────────────────────────────────────

  /// Sets a goal for a session that guides the agent across turns.
  Future<Map<String, dynamic>> setGoal(
    String sessionId,
    String objective, {
    int? tokenBudget,
  }) async {
    final body = <String, dynamic>{'objective': objective};
    if (tokenBudget != null) body['tokenBudget'] = tokenBudget;
    return _post('/sessions/$sessionId/goal', body);
  }

  /// Gets the current goal for a session.
  Future<Map<String, dynamic>?> getGoal(String sessionId) async {
    final json = await _get('/sessions/$sessionId/goal');
    if (json.containsKey('value') && json['value'] == null) return null;
    if (json.isEmpty) return null;
    return json;
  }

  /// Clears the goal for a session.
  Future<void> clearGoal(String sessionId) async {
    await _delete('/sessions/$sessionId/goal');
  }

  // ── Skills ───────────────────────────────────────────────────

  /// Lists all discovered skills (project + global).
  Future<List<Map<String, dynamic>>> listSkills() async {
    final json = await _get('/skills');
    final list = json['value'] as List<dynamic>? ?? json as List<dynamic>;
    return list.cast<Map<String, dynamic>>();
  }

  /// Sets the active skills for a session.
  Future<void> setSessionSkills(
    String sessionId,
    List<String> skills,
  ) async {
    await _post('/sessions/$sessionId/skills', {'skills': skills});
  }

  // ── Session model ────────────────────────────────────────────

  /// Changes the model used by an active session.
  Future<void> setSessionModel(
    String sessionId,
    String provider,
    String model,
  ) async {
    await _post('/sessions/$sessionId/model', {
      'provider': provider,
      'model': model,
    });
  }

  // ── Models ───────────────────────────────────────────────────

  /// Lists all available models on the server.
  Future<List<ModelInfo>> listModels() async {
    final json = await _get('/models');
    final list = json['value'] as List<dynamic>? ?? json as List<dynamic>;
    return list
        .map((e) => ModelInfo.fromJson(e as Map<String, dynamic>))
        .toList();
  }

  /// Selects a model globally and optionally persists the config change.
  Future<Map<String, dynamic>> selectModel({
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

  /// Syncs the model list from a specific provider.
  Future<Map<String, dynamic>> syncProviderModels({
    required String providerId,
    String saveTarget = 'auto',
  }) async {
    return _post('/providers/sync', {
      'providerId': providerId,
      'saveTarget': saveTarget,
    });
  }

  /// Syncs model lists from all providers.
  Future<Map<String, dynamic>> syncAllModels({
    String saveTarget = 'auto',
  }) async {
    return _post('/providers/sync-all?save=$saveTarget', {});
  }

  // ── Config ───────────────────────────────────────────────────

  /// Returns the current server configuration.
  Future<EngineConfig> loadedConfig() async {
    final json = await _get('/config');
    return EngineConfig.fromJson(json);
  }

  // ── Credentials ──────────────────────────────────────────────

  /// Lists all providers with their credential status.
  Future<List<Map<String, dynamic>>> listCredentials() async {
    final json = await _get('/credentials');
    final list = json['value'] as List<dynamic>? ?? json as List<dynamic>;
    return list.cast<Map<String, dynamic>>();
  }

  // ── MCP ──────────────────────────────────────────────────────

  /// Lists MCP servers connected to a session.
  Future<List<Map<String, dynamic>>> listMcpServers(String sessionId) async {
    final json = await _get('/sessions/$sessionId/mcp');
    final list = json['value'] as List<dynamic>? ?? json as List<dynamic>;
    return list.cast<Map<String, dynamic>>();
  }

  // ── Background commands ──────────────────────────────────────

  /// Lists all active background bash commands for a session.
  Future<List<Map<String, dynamic>>> listBackgroundCommands(
    String sessionId,
  ) async {
    final json = await _get('/sessions/$sessionId/background');
    final list = json['value'] as List<dynamic>? ?? json as List<dynamic>;
    return list.cast<Map<String, dynamic>>();
  }

  // ── Usage ────────────────────────────────────────────────────

  /// Fetches usage report (currently OpenAI only).
  Future<Map<String, dynamic>> usageReport() async {
    return _get('/usage');
  }

  // ── Events ───────────────────────────────────────────────────

  /// Subscribes to the event stream for a session via WebSocket.
  ///
  /// Returns a [Stream] of [RuntimeEvent]s. Events include assistant deltas,
  /// tool calls, approval requests, and completion signals.
  Stream<RuntimeEvent> subscribeEvents(String sessionId) {
    final wsScheme = _baseUrl.startsWith('https') ? 'wss' : 'ws';
    final httpBase = _baseUrl.replaceFirst(RegExp(r'^https?://'), '');
    final url =
        '$wsScheme://$httpBase/sessions/$sessionId/events?secret=$_secret';

    final channel = WebSocketChannel.connect(Uri.parse(url));

    return channel.stream.map((data) {
      try {
        final parsed = json.decode(data as String);
        if (parsed is Map<String, dynamic>) {
          return RuntimeEvent.fromJson(parsed);
        }
      } catch (_) {}
      return RuntimeEvent(version: 1, kind: {});
    });
  }

  // ── HTTP helpers ─────────────────────────────────────────────

  Future<Map<String, dynamic>> _get(String path) async {
    final uri = Uri.parse('$_baseUrl$path');
    final response = await _httpClient.get(uri, headers: _headers());

    if (response.statusCode == 401) {
      throw NaviRemoteException('Authentication failed: invalid secret');
    }
    if (response.statusCode != 200) {
      throw NaviRemoteException(
        'GET $path failed: ${response.statusCode} ${response.body}',
      );
    }

    final decoded = json.decode(response.body);
    if (decoded is Map<String, dynamic> && decoded.containsKey('error')) {
      throw NaviRemoteException(decoded['error'] as String);
    }
    return decoded is Map<String, dynamic> ? decoded : {'value': decoded};
  }

  Future<Map<String, dynamic>> _post(
    String path,
    Map<String, dynamic> body,
  ) async {
    final uri = Uri.parse('$_baseUrl$path');
    final response = await _httpClient.post(
      uri,
      headers: _headers(),
      body: json.encode(body),
    );

    if (response.statusCode == 401) {
      throw NaviRemoteException('Authentication failed: invalid secret');
    }
    if (response.statusCode >= 400) {
      try {
        final decoded = json.decode(response.body);
        if (decoded is Map<String, dynamic> && decoded.containsKey('error')) {
          throw NaviRemoteException(decoded['error'] as String);
        }
      } catch (e) {
        if (e is NaviRemoteException) rethrow;
      }
      throw NaviRemoteException(
        'POST $path failed: ${response.statusCode} ${response.body}',
      );
    }

    final decoded = json.decode(response.body);
    if (decoded is Map<String, dynamic> && decoded.containsKey('error')) {
      throw NaviRemoteException(decoded['error'] as String);
    }
    return decoded as Map<String, dynamic>;
  }

  Future<Map<String, dynamic>> _delete(String path) async {
    final uri = Uri.parse('$_baseUrl$path');
    final response = await _httpClient.delete(uri, headers: _headers());

    if (response.statusCode == 401) {
      throw NaviRemoteException('Authentication failed: invalid secret');
    }
    if (response.statusCode >= 400) {
      throw NaviRemoteException(
        'DELETE $path failed: ${response.statusCode} ${response.body}',
      );
    }

    final decoded = json.decode(response.body);
    return decoded as Map<String, dynamic>;
  }

  Map<String, String> _headers() => {
        'Content-Type': 'application/json',
        'X-Navi-Secret': _secret,
      };
}
