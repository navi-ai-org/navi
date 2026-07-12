part of 'navi_remote_engine.dart';

/// Auto-memory CRUD + maintenance.
extension NaviRemoteMemory on NaviRemoteEngine {
  /// `GET /memory/status`.
  Future<JsonMap> memoryStatus() => _get('/memory/status');

  /// `GET /memory/doctor`.
  Future<JsonMap> memoryDoctor() => _get('/memory/doctor');

  /// `POST /memory/init`.
  Future<JsonMap> memoryInit({bool? embeddings, bool? force}) {
    final body = <String, dynamic>{};
    if (embeddings != null) body['embeddings'] = embeddings;
    if (force != null) body['force'] = force;
    return _post('/memory/init', body);
  }

  /// `GET /memory?status=`.
  Future<List<JsonMap>> memoryList({String? status}) async {
    final q = _query({'status': status});
    final json = await _get('/memory$q');
    return asJsonMapList(json['value'] ?? json);
  }

  /// `POST /memory`.
  Future<JsonMap> memoryWrite({
    required String id,
    required String type,
    required String name,
    String description = '',
    String body = '',
  }) {
    return _post('/memory', {
      'id': id,
      'type': type,
      'memory_type': type,
      'memoryType': type,
      'name': name,
      'description': description,
      'body': body,
    });
  }

  /// `GET /memory/count`.
  Future<int> memoryCount() async {
    final json = await _get('/memory/count');
    return json['count'] as int? ??
        json['value'] as int? ??
        int.tryParse(json['count']?.toString() ?? '') ??
        0;
  }

  /// `GET /memory/index` — prompt-oriented markdown index.
  Future<JsonMap> memoryIndex() => _get('/memory/index');

  /// `GET /memory/search?q=&limit=`.
  Future<List<JsonMap>> memorySearch(String query, {int? limit}) async {
    final q = _query({
      'q': query,
      if (limit != null) 'limit': '$limit',
    });
    final json = await _get('/memory/search$q');
    return asJsonMapList(json['value'] ?? json['results'] ?? json);
  }

  /// `GET /memory/:id`.
  Future<JsonMap> memoryRead(String id) =>
      _get('/memory/${Uri.encodeComponent(id)}');

  /// `PATCH /memory/:id`.
  Future<JsonMap> memoryUpdate(
    String id, {
    String? name,
    String? description,
    String? body,
    String? status,
  }) {
    final payload = <String, dynamic>{};
    if (name != null) payload['name'] = name;
    if (description != null) payload['description'] = description;
    if (body != null) payload['body'] = body;
    if (status != null) payload['status'] = status;
    return _patch('/memory/${Uri.encodeComponent(id)}', payload);
  }

  /// `DELETE /memory/:id`.
  Future<JsonMap> memoryDelete(String id) =>
      _delete('/memory/${Uri.encodeComponent(id)}');

  /// `GET /memory/history?q=&limit=&sessionId=`.
  Future<List<JsonMap>> memoryHistorySearch(
    String query, {
    int? limit,
    String? sessionId,
  }) async {
    final q = _query({
      'q': query,
      if (limit != null) 'limit': '$limit',
      if (sessionId != null) 'sessionId': sessionId,
      if (sessionId != null) 'session_id': sessionId,
    });
    final json = await _get('/memory/history$q');
    return asJsonMapList(json['value'] ?? json['results'] ?? json);
  }

  /// `POST /memory/dream`.
  Future<JsonMap> memoryDream({
    bool? apply,
    int? sessions,
    String? instructions,
  }) {
    final body = <String, dynamic>{};
    if (apply != null) body['apply'] = apply;
    if (sessions != null) body['sessions'] = sessions;
    if (instructions != null) body['instructions'] = instructions;
    return _post('/memory/dream', body);
  }

  /// `POST /memory/distill`.
  Future<JsonMap> memoryDistill() => _post('/memory/distill', {});

  /// `POST /memory/checkpoint`.
  Future<JsonMap> memoryCheckpoint() => _post('/memory/checkpoint', {});

  /// `GET /memory/rebuild-preview`.
  Future<JsonMap> memoryRebuildPreview() => _get('/memory/rebuild-preview');
}
