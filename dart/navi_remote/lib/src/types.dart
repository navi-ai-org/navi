/// Types for the NAVI remote client.
///
/// Mirrors JSON shapes returned by `navi-server` (gateway over `navi-sdk`).
/// Complex / evolving payloads stay as [JsonMap] so the binding stays complete
/// without lagging behind every SDK struct field.

/// Loose JSON object from the server.
typedef JsonMap = Map<String, dynamic>;

/// Session information returned after starting or resuming a session.
class SessionInfo {
  final String id;
  final String projectDir;
  final String model;
  final String provider;
  final String? title;

  SessionInfo({
    required this.id,
    required this.projectDir,
    required this.model,
    required this.provider,
    this.title,
  });

  factory SessionInfo.fromJson(JsonMap json) => SessionInfo(
        id: json['id'] as String? ?? '',
        projectDir:
            (json['projectDir'] ?? json['project_dir'] ?? '').toString(),
        model: json['model'] as String? ?? '',
        provider: json['provider'] as String? ?? '',
        title: json['title'] as String?,
      );
}

/// Response from sending a turn (HTTP ack; real content streams over WS).
class TurnResponse {
  final String sessionId;
  final String text;

  TurnResponse({required this.sessionId, required this.text});

  factory TurnResponse.fromJson(JsonMap json) => TurnResponse(
        sessionId: json['sessionId'] as String? ??
            json['session_id'] as String? ??
            '',
        text: json['text'] as String? ?? '',
      );
}

/// A model available on the server (`GET /models` → `NaviModelInfo`).
class ModelInfo {
  final String id;
  final String name;
  final String providerId;
  final String providerLabel;
  final String taskSize;
  final int? contextWindowTokens;
  final bool? supportsThinking;
  final List<String> reasoningLevels;
  final String? defaultReasoningEffort;
  final bool effortBinary;
  final List<JsonMap> effortOptions;

  ModelInfo({
    required this.id,
    required this.name,
    required this.providerId,
    required this.providerLabel,
    required this.taskSize,
    this.contextWindowTokens,
    this.supportsThinking,
    this.reasoningLevels = const [],
    this.defaultReasoningEffort,
    this.effortBinary = false,
    this.effortOptions = const [],
  });

  factory ModelInfo.fromJson(JsonMap json) => ModelInfo(
        id: json['id'] as String? ?? '',
        name: json['name'] as String? ?? '',
        providerId: json['providerId'] as String? ??
            json['provider_id'] as String? ??
            '',
        providerLabel: json['providerLabel'] as String? ??
            json['provider_label'] as String? ??
            '',
        taskSize:
            json['taskSize'] as String? ?? json['task_size'] as String? ?? '',
        contextWindowTokens: json['contextWindowTokens'] as int? ??
            json['context_window_tokens'] as int?,
        supportsThinking: json['supportsThinking'] as bool? ??
            json['supports_thinking'] as bool?,
        reasoningLevels: ((json['reasoningLevels'] ?? json['reasoning_levels'])
                    as List<dynamic>? ??
                [])
            .map((e) => e.toString())
            .toList(),
        defaultReasoningEffort: json['defaultReasoningEffort'] as String? ??
            json['default_reasoning_effort'] as String?,
        effortBinary: json['effortBinary'] as bool? ??
            json['effort_binary'] as bool? ??
            false,
        effortOptions: ((json['effortOptions'] ?? json['effort_options'])
                    as List<dynamic>? ??
                [])
            .whereType<Map>()
            .map((e) => JsonMap.from(e))
            .toList(),
      );

  String get displayKey => providerId.isEmpty ? name : '$providerId:$name';
}

/// Engine configuration snapshot from `GET /config`.
class EngineConfig {
  final String provider;
  final String modelName;

  /// Home / default workspace (`--project` on the server).
  final String? projectDir;
  final String? dataDir;
  final String? projectConfigPath;

  /// Full raw config payload for forward-compat.
  final JsonMap raw;

  EngineConfig({
    required this.provider,
    required this.modelName,
    this.projectDir,
    this.dataDir,
    this.projectConfigPath,
    this.raw = const {},
  });

  factory EngineConfig.fromJson(JsonMap json) {
    final model = json['model'];
    String provider = '';
    String name = '';
    if (model is Map) {
      provider = model['provider'] as String? ?? '';
      name = model['name'] as String? ?? '';
    }
    return EngineConfig(
      provider: provider,
      modelName: name,
      projectDir: _asPath(json['projectDir'] ?? json['project_dir']),
      dataDir: _asPath(json['dataDir'] ?? json['data_dir']),
      projectConfigPath:
          _asPath(json['projectConfigPath'] ?? json['project_config_path']),
      raw: json,
    );
  }

  static String? _asPath(dynamic v) {
    if (v == null) return null;
    final s = v.toString();
    return s.isEmpty ? null : s;
  }
}

/// Metadata for a saved session (`GET /sessions/saved`).
class SavedSessionInfo {
  final String id;
  final String? title;
  final String project;
  final int createdAt;
  final int updatedAt;

  SavedSessionInfo({
    required this.id,
    this.title,
    required this.project,
    this.createdAt = 0,
    this.updatedAt = 0,
  });

  factory SavedSessionInfo.fromJson(JsonMap json) {
    final idRaw = json['id'];
    final id = idRaw is String
        ? idRaw
        : (idRaw is Map
            ? (idRaw.values.isNotEmpty
                ? idRaw.values.first.toString()
                : idRaw.toString())
            : idRaw?.toString() ?? '');
    return SavedSessionInfo(
      id: id,
      title: json['title'] as String?,
      project: (json['project'] ?? '').toString(),
      createdAt: _asUnix(json['createdAt'] ?? json['created_at']),
      updatedAt: _asUnix(json['updatedAt'] ?? json['updated_at']),
    );
  }

  static int _asUnix(dynamic v) {
    if (v is int) return v;
    if (v is num) return v.toInt();
    return int.tryParse(v?.toString() ?? '') ?? 0;
  }

  String get displayTitle {
    if (title != null && title!.trim().isNotEmpty) return title!.trim();
    return id;
  }
}

/// Skill available on the server (`GET /skills` → `NaviSkillInfo`).
class SkillInfo {
  final String id;
  final String name;
  final String? description;
  final String? version;
  final String? author;
  final List<String> tags;
  final List<String> requires;
  final String? path;
  final String? instructions;
  final bool editable;
  final String? scope;
  final String? source;
  final List<String> allowTools;
  final List<String> denyTools;

  SkillInfo({
    required this.id,
    required this.name,
    this.description,
    this.version,
    this.author,
    this.tags = const [],
    this.requires = const [],
    this.path,
    this.instructions,
    this.editable = false,
    this.scope,
    this.source,
    this.allowTools = const [],
    this.denyTools = const [],
  });

  factory SkillInfo.fromJson(JsonMap json) => SkillInfo(
        id: json['id'] as String? ?? '',
        name: json['name'] as String? ?? json['id'] as String? ?? '',
        description: json['description'] as String?,
        version: json['version'] as String?,
        author: json['author'] as String?,
        tags: _strList(json['tags']),
        requires: _strList(json['requires']),
        path: json['path'] as String?,
        instructions: json['instructions'] as String?,
        editable: json['editable'] as bool? ?? false,
        scope: json['scope'] as String?,
        source: json['source'] as String?,
        allowTools: _strList(json['allowTools'] ?? json['allow_tools']),
        denyTools: _strList(json['denyTools'] ?? json['deny_tools']),
      );

  static List<String> _strList(dynamic v) =>
      ((v as List<dynamic>?) ?? []).map((e) => e.toString()).toList();
}

/// Request body for `POST /skills` (create/update).
class SkillWriteRequest {
  final String? id;
  final String name;
  final String? description;
  final String? version;
  final String? author;
  final List<String> tags;
  final List<String> requires;
  final List<String> allowTools;
  final String instructions;

  /// `"user"` | `"project"`.
  final String scope;

  SkillWriteRequest({
    this.id,
    required this.name,
    this.description,
    this.version,
    this.author,
    this.tags = const [],
    this.requires = const [],
    this.allowTools = const [],
    required this.instructions,
    this.scope = 'user',
  });

  JsonMap toJson() => {
        if (id != null) 'id': id,
        'name': name,
        if (description != null) 'description': description,
        if (version != null) 'version': version,
        if (author != null) 'author': author,
        'tags': tags,
        'requires': requires,
        'allow_tools': allowTools,
        'allowTools': allowTools,
        'instructions': instructions,
        'scope': scope,
      };
}

/// Result of loading+resuming a saved session.
class LoadedSession {
  final SessionInfo session;
  final JsonMap? snapshot;

  LoadedSession({required this.session, this.snapshot});

  factory LoadedSession.fromJson(JsonMap json) {
    final hasLive = json.containsKey('provider') || json.containsKey('model');
    if (hasLive && json['id'] != null) {
      return LoadedSession(
        session: SessionInfo.fromJson(json),
        snapshot: json['snapshot'] is Map
            ? JsonMap.from(json['snapshot'] as Map)
            : json,
      );
    }
    final id = (json['id'] is String)
        ? json['id'] as String
        : json['id']?.toString() ?? '';
    return LoadedSession(
      session: SessionInfo(
        id: id,
        projectDir: (json['project'] ?? '').toString(),
        model: '',
        provider: '',
        title: json['title'] as String?,
      ),
      snapshot: json,
    );
  }
}

/// Permission modes for tool execution (`GET|POST /permission-mode`).
enum PermissionMode {
  restricted,
  acceptEdits,
  auto,
  yolo;

  String get apiValue {
    switch (this) {
      case PermissionMode.restricted:
        return 'restricted';
      case PermissionMode.acceptEdits:
        return 'accept_edits';
      case PermissionMode.auto:
        return 'auto';
      case PermissionMode.yolo:
        return 'yolo';
    }
  }

  static PermissionMode parse(String raw) {
    switch (raw.trim().toLowerCase().replaceAll('-', '_')) {
      case 'accept_edits':
      case 'acceptedits':
        return PermissionMode.acceptEdits;
      case 'auto':
        return PermissionMode.auto;
      case 'yolo':
        return PermissionMode.yolo;
      case 'restricted':
      default:
        return PermissionMode.restricted;
    }
  }
}

/// Agent mode (`GET /sessions/:id/mode`).
enum AgentMode {
  defaultMode,
  plan;

  String get apiValue => this == AgentMode.plan ? 'plan' : 'default';

  static AgentMode parse(String raw) {
    final v = raw.trim().toLowerCase();
    if (v == 'plan') return AgentMode.plan;
    return AgentMode.defaultMode;
  }
}

/// A runtime event from the session event stream.
class RuntimeEvent {
  final int version;
  final JsonMap kind;
  final JsonMap raw;

  RuntimeEvent({required this.version, required this.kind, JsonMap? raw})
      : raw = raw ?? {'version': version, 'kind': kind};

  factory RuntimeEvent.fromJson(JsonMap json) {
    final kind = json['kind'] is Map
        ? JsonMap.from(json['kind'] as Map)
        : <String, dynamic>{};
    return RuntimeEvent(
      version: json['version'] as int? ?? 1,
      kind: kind,
      raw: json,
    );
  }

  /// The event kind name (e.g. `AssistantDelta`, `ToolRequested`).
  String get kindName => kind.keys.firstOrNull ?? 'Unknown';

  dynamic get kindData => kind[kindName];
}

/// Voice event from `WS /voice/events`.
class VoiceEvent {
  final JsonMap raw;

  VoiceEvent(this.raw);

  factory VoiceEvent.fromJson(JsonMap json) => VoiceEvent(json);

  String get kindName {
    if (raw.containsKey('kind') && raw['kind'] is Map) {
      return (raw['kind'] as Map).keys.firstOrNull?.toString() ?? 'Unknown';
    }
    return raw.keys.firstOrNull ?? 'Unknown';
  }
}

/// Exception thrown when a NAVI remote operation fails.
class NaviRemoteException implements Exception {
  final String message;
  final int? statusCode;

  NaviRemoteException(this.message, {this.statusCode});

  @override
  String toString() => 'NaviRemoteException: $message';
}

// ── JSON helpers ─────────────────────────────────────────────────────────

JsonMap asJsonMap(dynamic value) {
  if (value is Map<String, dynamic>) return value;
  if (value is Map) return JsonMap.from(value);
  return {'value': value};
}

List<JsonMap> asJsonMapList(dynamic value) {
  final list = value is List
      ? value
      : (value is Map && value['value'] is List)
          ? value['value'] as List
          : const [];
  return list.whereType<Map>().map((e) => JsonMap.from(e)).toList();
}

List<String> asStringList(dynamic value) {
  final list = value is List
      ? value
      : (value is Map && value['value'] is List)
          ? value['value'] as List
          : const [];
  return list.map((e) => e.toString()).toList();
}
