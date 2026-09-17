part of 'navi_remote_engine.dart';

/// Voice / transcription gateway surface (remote providers).
extension NaviRemoteVoice on NaviRemoteEngine {
  /// `GET /voice/status`.
  Future<JsonMap> voiceStatus() => _get('/voice/status');

  /// `GET /voice/doctor`.
  Future<JsonMap> voiceDoctor() => _get('/voice/doctor');

  /// `GET /voice/providers`.
  Future<List<JsonMap>> voiceProviders() async {
    final json = await _get('/voice/providers');
    return asJsonMapList(json['value'] ?? json);
  }

  /// `POST /voice/transcribe` — transcribe a WAV path on the server host
  /// through the configured remote provider.
  Future<JsonMap> voiceTranscribe(String path, {String? language}) {
    return _post('/voice/transcribe', {
      'path': path,
      if (language != null) 'language': language,
    });
  }
}
