part of 'navi_remote_engine.dart';

/// Skills CRUD, MCP config, session MCP tools, attachment/background routing.
extension NaviRemoteSkillsMcp on NaviRemoteEngine {
  // ── Skills CRUD ────────────────────────────────────────────

  /// `GET /skills/:id` — includes full instruction body when available.
  Future<SkillInfo> getSkill(String id) async {
    final json = await _get('/skills/${Uri.encodeComponent(id)}');
    return SkillInfo.fromJson(json);
  }

  /// `POST /skills` — create or update.
  Future<JsonMap> saveSkill(SkillWriteRequest request) =>
      _post('/skills', request.toJson());

  /// `DELETE /skills/:id`.
  Future<bool> deleteSkill(String id) async {
    final json = await _delete('/skills/${Uri.encodeComponent(id)}');
    return json['deleted'] as bool? ?? false;
  }

  // ── MCP config (global) ────────────────────────────────────

  /// `GET /mcp`.
  Future<JsonMap> getMcpConfig() => _get('/mcp');

  /// `PUT /mcp` — replace full MCP config.
  Future<JsonMap> setMcpConfig({
    required bool enabled,
    required List<JsonMap> servers,
    String saveTarget = 'auto',
  }) {
    return _put('/mcp', {
      'enabled': enabled,
      'servers': servers,
      'saveTarget': saveTarget,
    });
  }

  /// `POST /mcp/enabled`.
  Future<JsonMap> setMcpEnabled(bool enabled, {String saveTarget = 'auto'}) {
    return _post('/mcp/enabled', {
      'enabled': enabled,
      'saveTarget': saveTarget,
    });
  }

  /// `POST /mcp/servers` — upsert one server config object.
  Future<JsonMap> upsertMcpServer(JsonMap server, {String saveTarget = 'auto'}) {
    final body = JsonMap.from(server);
    body['saveTarget'] = saveTarget;
    return _post('/mcp/servers', body);
  }

  /// `DELETE /mcp/servers/:id?saveTarget=`.
  Future<JsonMap> removeMcpServer(String id, {String saveTarget = 'auto'}) {
    final q = _query({'saveTarget': saveTarget});
    return _delete('/mcp/servers/${Uri.encodeComponent(id)}$q');
  }

  /// `GET /sessions/:id/mcp/tools`.
  Future<List<String>> listSessionMcpTools(String sessionId) async {
    final json = await _get('/sessions/$sessionId/mcp/tools');
    return asStringList(json['value'] ?? json['tools'] ?? json);
  }

  // ── Routing ────────────────────────────────────────────────

  /// `GET /routing`.
  Future<JsonMap> getRouting() => _get('/routing');

  /// `POST /routing/attachment`.
  Future<JsonMap> setAttachmentModel({
    required String modality,
    required String provider,
    required String model,
    String saveTarget = 'auto',
  }) {
    return _post('/routing/attachment', {
      'modality': modality,
      'provider': provider,
      'model': model,
      'saveTarget': saveTarget,
    });
  }

  /// `DELETE /routing/attachment/:modality?saveTarget=`.
  Future<JsonMap> clearAttachmentModel(
    String modality, {
    String saveTarget = 'auto',
  }) {
    final q = _query({'saveTarget': saveTarget});
    return _delete(
      '/routing/attachment/${Uri.encodeComponent(modality)}$q',
    );
  }

  /// `POST /routing/background`.
  Future<JsonMap> setBackgroundModel({
    required String task,
    required String provider,
    required String model,
    String saveTarget = 'auto',
  }) {
    return _post('/routing/background', {
      'task': task,
      'provider': provider,
      'model': model,
      'saveTarget': saveTarget,
    });
  }

  /// `DELETE /routing/background/:task?saveTarget=`.
  Future<JsonMap> clearBackgroundModel(
    String task, {
    String saveTarget = 'auto',
  }) {
    final q = _query({'saveTarget': saveTarget});
    return _delete(
      '/routing/background/${Uri.encodeComponent(task)}$q',
    );
  }
}
