/// Types for the NAVI remote client.
///
/// Mirrors the types from the navi_agent package but designed for
/// HTTP/WebSocket communication with a navi-server instance.

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
        id: json['id'] as String? ?? '',
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

/// A model available on the server.
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

/// Engine configuration snapshot from the server.
class EngineConfig {
  final String provider;
  final String modelName;
  final String? projectDir;

  EngineConfig({
    required this.provider,
    required this.modelName,
    this.projectDir,
  });

  factory EngineConfig.fromJson(Map<String, dynamic> json) => EngineConfig(
        provider: json['model']?['provider'] as String? ?? '',
        modelName: json['model']?['name'] as String? ?? '',
        projectDir: json['projectDir'] as String?,
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

/// Exception thrown when a NAVI remote operation fails.
class NaviRemoteException implements Exception {
  final String message;
  NaviRemoteException(this.message);

  @override
  String toString() => 'NaviRemoteException: $message';
}
