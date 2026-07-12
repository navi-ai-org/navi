part of 'navi_remote_engine.dart';

/// Session ops: plan mode, sudo, permission, rewind, context, goals, bg, rename.
extension NaviRemoteSessionOps on NaviRemoteEngine {
  // ── Agent / plan mode ──────────────────────────────────────

  /// `GET /sessions/:id/mode`.
  Future<AgentMode> getSessionMode(String sessionId) async {
    final json = await _get('/sessions/$sessionId/mode');
    final raw = (json['mode'] ?? json['value'] ?? 'default').toString();
    return AgentMode.parse(raw);
  }

  /// `POST /sessions/:id/plan/enter`.
  Future<JsonMap> enterPlanMode(String sessionId) =>
      _post('/sessions/$sessionId/plan/enter', {});

  /// `POST /sessions/:id/plan/exit`.
  Future<JsonMap> exitPlanMode(String sessionId) =>
      _post('/sessions/$sessionId/plan/exit', {});

  /// `POST /sessions/:id/plan/review`.
  ///
  /// [decision]: `approve` | `request_changes` | `quit`.
  Future<JsonMap> resolvePlanReview(
    String sessionId, {
    required String id,
    required String planId,
    required String decision,
    List<JsonMap> comments = const [],
    String freeform = '',
  }) {
    return _post('/sessions/$sessionId/plan/review', {
      'id': id,
      'planId': planId,
      'decision': decision,
      'comments': comments,
      'freeform': freeform,
    });
  }

  // ── Sudo ───────────────────────────────────────────────────

  /// `POST /sessions/:id/sudo` — password is never logged server-side.
  Future<JsonMap> resolveSudo(
    String sessionId, {
    required String id,
    String? password,
  }) {
    return _post('/sessions/$sessionId/sudo', {
      'id': id,
      if (password != null) 'password': password,
    });
  }

  // ── Context / rewind ───────────────────────────────────────

  /// `POST /sessions/:id/context` — inject a [ContextPacket]-shaped map.
  Future<JsonMap> addContextPacket(String sessionId, JsonMap packet) =>
      _post('/sessions/$sessionId/context', packet);

  /// `POST /sessions/:id/rewind`.
  Future<int> rewindSession(String sessionId, {required int keepUserTurns}) async {
    final json = await _post('/sessions/$sessionId/rewind', {
      'keepUserTurns': keepUserTurns,
    });
    return json['remainingMessages'] as int? ??
        json['remaining_messages'] as int? ??
        0;
  }

  // ── Goal extensions ────────────────────────────────────────

  /// `POST /sessions/:id/goal/status`.
  ///
  /// [status]: active | paused | blocked | usage_limited | budget_limited | complete
  Future<JsonMap> updateGoalStatus(String sessionId, String status) =>
      _post('/sessions/$sessionId/goal/status', {'status': status});

  /// `POST /sessions/:id/goal/checklist` — replace checklist tasks.
  Future<JsonMap> setGoalChecklist(
    String sessionId,
    List<JsonMap> tasks,
  ) =>
      _post('/sessions/$sessionId/goal/checklist', {'tasks': tasks});

  /// `POST /sessions/:id/goal/tasks/:taskId`.
  ///
  /// [status]: pending | in_progress | done | verified | skipped
  Future<JsonMap> updateGoalTaskStatus(
    String sessionId,
    int taskId,
    String status,
  ) =>
      _post('/sessions/$sessionId/goal/tasks/$taskId', {'status': status});

  // ── Background poll/cancel ─────────────────────────────────

  /// `GET /sessions/:id/background/:taskId`.
  Future<JsonMap> getBackgroundCommand(String sessionId, String taskId) =>
      _get('/sessions/$sessionId/background/$taskId');

  /// `POST /sessions/:id/background/:taskId/cancel`.
  Future<JsonMap> cancelBackgroundCommand(String sessionId, String taskId) =>
      _post('/sessions/$sessionId/background/$taskId/cancel', {});

  // ── Rename saved session ───────────────────────────────────

  /// `POST /sessions/:id/rename`.
  Future<JsonMap> renameSavedSession(String sessionId, String title) =>
      _post('/sessions/$sessionId/rename', {'title': title});

  // ── Permission mode ────────────────────────────────────────

  /// `GET /permission-mode`.
  Future<PermissionMode> getPermissionMode() async {
    final json = await _get('/permission-mode');
    final raw = (json['mode'] ?? json['value'] ?? 'restricted').toString();
    return PermissionMode.parse(raw);
  }

  /// `POST /permission-mode`.
  Future<PermissionMode> setPermissionMode(PermissionMode mode) async {
    final json = await _post('/permission-mode', {'mode': mode.apiValue});
    final raw = (json['mode'] ?? mode.apiValue).toString();
    return PermissionMode.parse(raw);
  }
}
