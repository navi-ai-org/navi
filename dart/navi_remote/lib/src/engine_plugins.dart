part of 'navi_remote_engine.dart';

/// Plugin lifecycle: list / search / install / update / remove / reload.
extension NaviRemotePlugins on NaviRemoteEngine {
  /// `GET /plugins`.
  Future<List<JsonMap>> listPlugins() async {
    final json = await _get('/plugins');
    return asJsonMapList(json['value'] ?? json);
  }

  /// `GET /plugins/search?q=`.
  Future<List<JsonMap>> searchPlugins({String? query}) async {
    final q = _query({'q': query});
    final json = await _get('/plugins/search$q');
    return asJsonMapList(json['value'] ?? json);
  }

  /// `GET /plugins/:id`.
  Future<JsonMap> getPlugin(String id) =>
      _get('/plugins/${Uri.encodeComponent(id)}');

  /// `POST /plugins/install/path` — [confirm] must be true to apply.
  Future<JsonMap> installPluginFromPath(String path, {bool confirm = false}) {
    return _post('/plugins/install/path', {
      'path': path,
      'confirm': confirm,
    });
  }

  /// `POST /plugins/install/marketplace`.
  Future<JsonMap> installPluginFromMarketplace(
    String pluginId, {
    bool confirm = false,
  }) {
    return _post('/plugins/install/marketplace', {
      'plugin_id': pluginId,
      'pluginId': pluginId,
      'confirm': confirm,
    });
  }

  /// `POST /plugins/update/path`.
  Future<JsonMap> updatePluginFromPath(
    String path, {
    bool force = false,
    bool confirm = false,
  }) {
    return _post('/plugins/update/path', {
      'path': path,
      'force': force,
      'confirm': confirm,
    });
  }

  /// `POST /plugins/update/marketplace`.
  Future<JsonMap> updatePluginFromMarketplace(
    String pluginId, {
    bool force = false,
    bool confirm = false,
  }) {
    return _post('/plugins/update/marketplace', {
      'plugin_id': pluginId,
      'pluginId': pluginId,
      'force': force,
      'confirm': confirm,
    });
  }

  /// `DELETE /plugins/:id`.
  Future<JsonMap> removePlugin(String id) =>
      _delete('/plugins/${Uri.encodeComponent(id)}');

  /// `POST /plugins/reload-wasm`.
  Future<JsonMap> reloadWasmPlugins() => _post('/plugins/reload-wasm', {});
}
