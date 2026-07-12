part of 'navi_remote_engine.dart';

/// Provider registry catalog + sync.
extension NaviRemoteRegistry on NaviRemoteEngine {
  /// `GET /registry`.
  Future<JsonMap> getRegistry() => _get('/registry');

  /// `POST /registry/sync` — optional force via body and/or query.
  Future<JsonMap> syncRegistry({bool force = false}) {
    final q = force ? '?force=true' : '';
    return _post('/registry/sync$q', {'force': force});
  }
}
