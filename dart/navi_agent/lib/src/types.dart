/// Dart types mirroring the NAVI SDK's serializable structs.
///
/// All types implement JSON serialization for FFI data exchange.
import 'dart:convert';

/// Session information returned after starting a session.
class SessionInfo {
  final String id;
  final String projectDir;
  final String model;
  final String provider;

  SessionInfo({
    required this.id,
    required this.projectDir,
    required this.model,
    required this.provider,
  });

  factory SessionInfo.fromJson(Map<String, dynamic> json) => SessionInfo(
        id: json['id'] as String,
        projectDir: json['projectDir'] as String? ?? '',
        model: json['model'] as String? ?? '',
        provider: json['provider'] as String? ?? '',
      );
}

/// Response from sending a turn.
class TurnResponse {
  final String sessionId;
  final String text;

  TurnResponse({required this.sessionId, required this.text});

  factory TurnResponse.fromJson(Map<String, dynamic> json) => TurnResponse(
        sessionId: json['sessionId'] as String? ?? '',
        text: json['text'] as String? ?? '',
      );
}

/// A model available in the current configuration.
class ModelInfo {
  final String id;
  final String name;
  final String providerId;
  final String providerLabel;
  final String taskSize;
  final int? contextWindowTokens;

  ModelInfo({
    required this.id,
    required this.name,
    required this.providerId,
    required this.providerLabel,
    required this.taskSize,
    this.contextWindowTokens,
  });

  factory ModelInfo.fromJson(Map<String, dynamic> json) => ModelInfo(
        id: json['id'] as String? ?? '',
        name: json['name'] as String? ?? '',
        providerId: json['providerId'] as String? ?? '',
        providerLabel: json['providerLabel'] as String? ?? '',
        taskSize: json['taskSize'] as String? ?? '',
        contextWindowTokens: json['contextWindowTokens'] as int?,
      );
}

/// A discovered skill (SKILL.md) available for activation.
class SkillInfo {
  final String id;
  final String name;
  final String? description;
  final List<String> tags;

  SkillInfo({
    required this.id,
    required this.name,
    this.description,
    this.tags = const [],
  });

  factory SkillInfo.fromJson(Map<String, dynamic> json) => SkillInfo(
        id: json['id'] as String? ?? '',
        name: json['name'] as String? ?? '',
        description: json['description'] as String?,
        tags: (json['tags'] as List<dynamic>?)
                ?.map((e) => e as String)
                .toList() ??
            [],
      );
}

/// Provider account information.
class ProviderAccountInfo {
  final String providerId;
  final String providerLabel;
  final String envVar;
  final bool hasStoredKey;

  ProviderAccountInfo({
    required this.providerId,
    required this.providerLabel,
    required this.envVar,
    required this.hasStoredKey,
  });

  factory ProviderAccountInfo.fromJson(Map<String, dynamic> json) =>
      ProviderAccountInfo(
        providerId: json['providerId'] as String? ?? '',
        providerLabel: json['providerLabel'] as String? ?? '',
        envVar: json['envVar'] as String? ?? '',
        hasStoredKey: json['hasStoredKey'] as bool? ?? false,
      );
}

/// Options for sending a turn.
class TurnOptions {
  final String? thinking;

  /// Content parts for multimodal messages.
  final List<Map<String, dynamic>>? contentParts;

  /// Context packets to include with this turn only.
  final List<Map<String, dynamic>>? contextPackets;

  TurnOptions({this.thinking, this.contentParts, this.contextPackets});

  Map<String, dynamic> toJson() {
    final json = <String, dynamic>{};
    if (thinking != null) json['thinking'] = thinking;
    if (contentParts != null && contentParts!.isNotEmpty) {
      json['contentParts'] = contentParts;
    }
    if (contextPackets != null && contextPackets!.isNotEmpty) {
      json['contextPackets'] = contextPackets;
    }
    return json;
  }
}

/// Engine configuration snapshot.
class EngineConfig {
  final String provider;
  final String modelName;
  final String? globalConfigPath;
  final String? projectConfigPath;
  final String dataDir;

  EngineConfig({
    required this.provider,
    required this.modelName,
    this.globalConfigPath,
    this.projectConfigPath,
    required this.dataDir,
  });

  factory EngineConfig.fromJson(Map<String, dynamic> json) => EngineConfig(
        provider: json['model']?['provider'] as String? ?? '',
        modelName: json['model']?['name'] as String? ?? '',
        globalConfigPath: json['globalConfigPath'] as String?,
        projectConfigPath: json['projectConfigPath'] as String?,
        dataDir: json['dataDir'] as String? ?? '',
      );
}

/// Model selection result.
class ModelSelectionResult {
  final String providerId;
  final String model;
  final int? contextWindowTokens;
  final bool providerConfigured;
  final String? savedTo;

  ModelSelectionResult({
    required this.providerId,
    required this.model,
    this.contextWindowTokens,
    required this.providerConfigured,
    this.savedTo,
  });

  factory ModelSelectionResult.fromJson(Map<String, dynamic> json) =>
      ModelSelectionResult(
        providerId: json['providerId'] as String? ?? '',
        model: json['model'] as String? ?? '',
        contextWindowTokens: json['contextWindowTokens'] as int?,
        providerConfigured: json['providerConfigured'] as bool? ?? false,
        savedTo: json['savedTo'] as String?,
      );
}

/// Saved session metadata.
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
    required this.createdAt,
    required this.updatedAt,
  });

  factory SavedSessionInfo.fromJson(Map<String, dynamic> json) =>
      SavedSessionInfo(
        id: json['id'] as String? ?? '',
        title: json['title'] as String?,
        project: json['project'] as String? ?? '',
        createdAt: json['createdAt'] as int? ?? 0,
        updatedAt: json['updatedAt'] as int? ?? 0,
      );
}

/// A runtime event from the engine event stream.
class RuntimeEvent {
  final int version;
  final Map<String, dynamic> kind;

  RuntimeEvent({required this.version, required this.kind});

  factory RuntimeEvent.fromJson(Map<String, dynamic> json) => RuntimeEvent(
        version: json['version'] as int? ?? 1,
        kind: json['kind'] as Map<String, dynamic>? ?? {},
      );

  /// The event kind name (e.g. 'AssistantDelta', 'ToolRequested').
  String get kindName => kind.keys.firstOrNull ?? 'Unknown';
}

/// Exception thrown when a NAVI engine operation fails.
class NaviException implements Exception {
  final String message;
  NaviException(this.message);

  @override
  String toString() => 'NaviException: $message';
}

// Internal helper to parse JSON from a C string pointer.
Map<String, dynamic>? tryParseJson(String jsonStr) {
  if (jsonStr.isEmpty || jsonStr == 'null') return null;
  try {
    final decoded = json.decode(jsonStr);
    if (decoded is Map<String, dynamic>) return decoded;
    return null;
  } catch (_) {
    return null;
  }
}

List<dynamic>? tryParseJsonList(String jsonStr) {
  if (jsonStr.isEmpty || jsonStr == 'null') return null;
  try {
    final decoded = json.decode(jsonStr);
    if (decoded is List) return decoded;
    return null;
  } catch (_) {
    return null;
  }
}
