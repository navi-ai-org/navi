part of 'navi_remote_engine.dart';

/// Credentials + device OAuth.
extension NaviRemoteAuth on NaviRemoteEngine {
  /// `GET /credentials`.
  Future<List<JsonMap>> listCredentials() async {
    final json = await _get('/credentials');
    return asJsonMapList(json['value'] ?? json);
  }

  /// `GET /credentials/:providerId` — status + accounts.
  Future<JsonMap> getCredential(String providerId) =>
      _get('/credentials/${Uri.encodeComponent(providerId)}');

  /// `PUT /credentials/:providerId`.
  Future<JsonMap> setProviderApiKey(String providerId, String apiKey) {
    return _put('/credentials/${Uri.encodeComponent(providerId)}', {
      'apiKey': apiKey,
    });
  }

  /// `DELETE /credentials/:providerId`.
  Future<JsonMap> deleteProviderApiKey(String providerId) =>
      _delete('/credentials/${Uri.encodeComponent(providerId)}');

  /// `GET /credentials/:providerId/accounts`.
  Future<List<JsonMap>> listCredentialAccounts(String providerId) async {
    final json =
        await _get('/credentials/${Uri.encodeComponent(providerId)}/accounts');
    return asJsonMapList(json['value'] ?? json);
  }

  /// `POST /credentials/:providerId/accounts`.
  Future<JsonMap> addProviderAccount(
    String providerId, {
    required String apiKey,
    String? label,
  }) {
    return _post(
      '/credentials/${Uri.encodeComponent(providerId)}/accounts',
      {
        'apiKey': apiKey,
        if (label != null) 'label': label,
      },
    );
  }

  /// `POST /credentials/:providerId/accounts/:accountId/select`.
  Future<JsonMap> selectProviderAccount(
    String providerId,
    String accountId,
  ) {
    return _post(
      '/credentials/${Uri.encodeComponent(providerId)}/accounts/'
      '${Uri.encodeComponent(accountId)}/select',
      {},
    );
  }

  /// `DELETE /credentials/:providerId/accounts/:accountId`.
  Future<JsonMap> deleteProviderAccount(
    String providerId,
    String accountId,
  ) {
    return _delete(
      '/credentials/${Uri.encodeComponent(providerId)}/accounts/'
      '${Uri.encodeComponent(accountId)}',
    );
  }

  /// `GET /oauth/:providerId/supports`.
  Future<bool> providerSupportsDeviceOauth(String providerId) async {
    final json =
        await _get('/oauth/${Uri.encodeComponent(providerId)}/supports');
    return json['supports'] as bool? ??
        json['supported'] as bool? ??
        json['value'] as bool? ??
        false;
  }

  /// `POST /oauth/:providerId` — blocking device OAuth flow.
  Future<JsonMap> startDeviceOauth(String providerId, [JsonMap? body]) {
    return _post(
      '/oauth/${Uri.encodeComponent(providerId)}',
      body ?? {},
    );
  }
}
