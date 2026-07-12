part of 'navi_remote_engine.dart';

/// Voice / dictation gateway surface.
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

  /// `GET /voice/installed?engine=`.
  Future<JsonMap> voiceEngineInstalled({String? engine}) {
    final q = _query({'engine': engine});
    return _get('/voice/installed$q');
  }

  /// `POST /voice/init` — download engine package.
  Future<JsonMap> voiceInit({String? engine, bool? force}) {
    final body = <String, dynamic>{};
    if (engine != null) body['engine'] = engine;
    if (force != null) body['force'] = force;
    return _post('/voice/init', body);
  }

  /// `POST /voice/transcribe` — transcribe a WAV path on the server host.
  Future<JsonMap> voiceTranscribe(String path, {String? language}) {
    return _post('/voice/transcribe', {
      'path': path,
      if (language != null) 'language': language,
    });
  }

  /// `POST /voice/stream/start`.
  Future<JsonMap> voiceStreamStart({String? language}) {
    return _post('/voice/stream/start', {
      if (language != null) 'language': language,
    });
  }

  /// `POST /voice/stream/pcm` — 16 kHz mono f32 samples.
  Future<JsonMap> voiceStreamPcm(List<double> samples) {
    return _post('/voice/stream/pcm', {'samples': samples});
  }

  /// `POST /voice/stream/end` → final text.
  Future<JsonMap> voiceStreamEnd() => _post('/voice/stream/end', {});

  /// `POST /voice/stream/cancel`.
  Future<JsonMap> voiceStreamCancel() => _post('/voice/stream/cancel', {});

  /// `WS /voice/events?secret=`.
  Stream<VoiceEvent> subscribeVoiceEvents() {
    final channel = _connectWs('/voice/events');
    return channel.stream.map((data) {
      try {
        final parsed = json.decode(data as String);
        if (parsed is Map) {
          return VoiceEvent.fromJson(JsonMap.from(parsed));
        }
      } catch (_) {}
      return VoiceEvent({});
    });
  }
}
