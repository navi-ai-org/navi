import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:navi_remote/navi_remote.dart';
import 'package:test/test.dart';

void main() {
  group('NaviRemoteEngine HTTP integration (mock client)', () {
    late List<http.BaseRequest> requests;

    NaviRemoteEngine engineWith(MockClientHandler handler) {
      requests = [];
      final client = MockClient((request) async {
        requests.add(request);
        return handler(request);
      });
      return NaviRemoteEngine.forTesting(
        baseUrl: 'http://gateway.test:9800',
        secret: 'secret-xyz',
        client: client,
      );
    }

    test('sends X-Navi-Secret on authenticated calls', () async {
      final engine = engineWith((req) async {
        return http.Response(
          jsonEncode({
            'model': {'provider': 'p', 'name': 'm'},
            'projectDir': '/home',
          }),
          200,
        );
      });
      // /health intentionally has no auth header (public).
      await engine.health();
      expect(requests.first.url.path, '/health');
      expect(requests.first.headers['X-Navi-Secret'], isNull);

      await engine.loadedConfig();
      final configReq = requests.last;
      expect(configReq.url.path, '/config');
      expect(configReq.headers['X-Navi-Secret'], 'secret-xyz');
      expect(configReq.headers['Content-Type'], contains('json'));
      engine.dispose();
    });

    test('startSession sends single camelCase keys (no dual snake)', () async {
      final engine = engineWith((req) async {
        expect(req.method, 'POST');
        expect(req.url.path, '/sessions');
        final body = jsonDecode(req.body) as Map;
        // Dual keys break serde on the server — only camelCase.
        expect(body.containsKey('project_dir'), isFalse);
        expect(body['projectDir'], '/proj');
        expect(body.containsKey('active_skills'), isFalse);
        expect(body['activeSkills'], ['s1']);
        return http.Response(
          jsonEncode({
            'id': 'sess-9',
            'projectDir': '/proj',
            'model': 'm',
            'provider': 'p',
          }),
          201,
        );
      });

      final s = await engine.startSession(
        projectDir: '/proj',
        activeSkills: ['s1'],
      );
      expect(s.id, 'sess-9');
      engine.dispose();
    });

    test('approve uses single requestId key', () async {
      final engine = engineWith((req) async {
        final body = jsonDecode(req.body) as Map;
        expect(body.containsKey('request_id'), isFalse);
        expect(body['requestId'], 'apr-1');
        expect(body['approved'], true);
        return http.Response(jsonEncode({'consumed': true}), 200);
      });
      final ok = await engine.approve('sess', 'apr-1');
      expect(ok, isTrue);
      engine.dispose();
    });

    test('setPermissionMode body', () async {
      final engine = engineWith((req) async {
        expect(req.url.path, '/permission-mode');
        final body = jsonDecode(req.body) as Map;
        expect(body['mode'], 'yolo');
        return http.Response(jsonEncode({'mode': 'yolo'}), 200);
      });
      final mode = await engine.setPermissionMode(PermissionMode.yolo);
      expect(mode, PermissionMode.yolo);
      engine.dispose();
    });

    test('listSavedSessions parses array body', () async {
      final engine = engineWith((req) async {
        return http.Response(
          jsonEncode([
            {
              'id': 'a',
              'title': 'One',
              'project': '/p',
              'createdAt': 1,
              'updatedAt': 2,
            },
          ]),
          200,
        );
      });
      final list = await engine.listSavedSessions();
      expect(list.length, 1);
      expect(list.first.title, 'One');
      engine.dispose();
    });

    test('loadSavedSession parses live resume payload', () async {
      final engine = engineWith((req) async {
        expect(req.url.path, '/sessions/load/abc');
        return http.Response(
          jsonEncode({
            'id': 'abc',
            'projectDir': '/home',
            'model': 'claude',
            'provider': 'anthropic',
            'title': 'Restored',
            'snapshot': {
              'id': 'abc',
              'events': [
                {'role': 'user', 'text': 'hi'},
              ],
            },
          }),
          200,
        );
      });
      final loaded = await engine.loadSavedSession('abc');
      expect(loaded.session.id, 'abc');
      expect(loaded.session.provider, 'anthropic');
      expect(loaded.snapshot?['events'], isNotEmpty);
      engine.dispose();
    });

    test('throws NaviRemoteException on 401', () async {
      final engine = engineWith(
        (req) async => http.Response('{"error":"nope"}', 401),
      );
      expect(
        () => engine.listModels(),
        throwsA(
          isA<NaviRemoteException>().having(
            (e) => e.statusCode,
            'statusCode',
            401,
          ),
        ),
      );
      engine.dispose();
    });

    test('throws with server error message on 400', () async {
      final engine = engineWith(
        (req) async =>
            http.Response(jsonEncode({'error': 'invalid body'}), 400),
      );
      expect(
        () => engine.sendTurn('s', 'hi'),
        throwsA(
          isA<NaviRemoteException>().having(
            (e) => e.message,
            'message',
            'invalid body',
          ),
        ),
      );
      engine.dispose();
    });

    test('memorySearch builds query string', () async {
      final engine = engineWith((req) async {
        expect(req.method, 'GET');
        expect(req.url.path, '/memory/search');
        expect(req.url.queryParameters['q'], 'auth');
        expect(req.url.queryParameters['limit'], '10');
        return http.Response(jsonEncode({'results': []}), 200);
      });
      await engine.memorySearch('auth', limit: 10);
      engine.dispose();
    });

    test('resolvePlanReview single planId key', () async {
      final engine = engineWith((req) async {
        expect(req.url.path, '/sessions/s1/plan/review');
        final body = jsonDecode(req.body) as Map;
        expect(body.containsKey('plan_id'), isFalse);
        expect(body['planId'], 'p1');
        expect(body['decision'], 'approve');
        return http.Response(jsonEncode({'consumed': true}), 200);
      });
      await engine.resolvePlanReview(
        's1',
        id: 'r1',
        planId: 'p1',
        decision: 'approve',
      );
      engine.dispose();
    });

    test('selectModel single camelCase keys', () async {
      final engine = engineWith((req) async {
        final body = jsonDecode(req.body) as Map;
        expect(body.containsKey('provider_id'), isFalse);
        expect(body['providerId'], 'openai');
        expect(body.containsKey('save_target'), isFalse);
        expect(body['saveTarget'], 'auto');
        return http.Response(jsonEncode({'model': 'gpt'}), 200);
      });
      await engine.selectModel(providerId: 'openai', model: 'gpt');
      engine.dispose();
    });

    test('listSkills maps SkillInfo', () async {
      final engine = engineWith((req) async {
        return http.Response(
          jsonEncode([
            {'id': 's', 'name': 'Skill', 'tags': [], 'requires': []},
          ]),
          200,
        );
      });
      final skills = await engine.listSkills();
      expect(skills.single.id, 's');
      engine.dispose();
    });

    test('memoryWrite sends single type key', () async {
      final engine = engineWith((req) async {
        expect(req.method, 'POST');
        expect(req.url.path, '/memory');
        final body = jsonDecode(req.body) as Map;
        expect(body.containsKey('memory_type'), isFalse);
        expect(body.containsKey('memoryType'), isFalse);
        expect(body['type'], 'note');
        return http.Response(jsonEncode({'id': 'm1'}), 201);
      });
      await engine.memoryWrite(
        id: 'm1',
        type: 'note',
        name: 'Note',
        description: 'desc',
        body: 'body',
      );
      engine.dispose();
    });

    test('memoryHistorySearch sends single sessionId query', () async {
      final engine = engineWith((req) async {
        expect(req.method, 'GET');
        expect(req.url.path, '/memory/history');
        final q = req.url.queryParameters;
        expect(q.containsKey('session_id'), isFalse);
        expect(q['sessionId'], 's1');
        return http.Response(jsonEncode({'results': []}), 200);
      });
      await engine.memoryHistorySearch('q', sessionId: 's1');
      engine.dispose();
    });

    test('installPluginFromMarketplace sends single pluginId key', () async {
      final engine = engineWith((req) async {
        expect(req.method, 'POST');
        expect(req.url.path, '/plugins/install/marketplace');
        final body = jsonDecode(req.body) as Map;
        expect(body.containsKey('plugin_id'), isFalse);
        expect(body['pluginId'], 'p1');
        return http.Response(jsonEncode({'id': 'p1'}), 200);
      });
      await engine.installPluginFromMarketplace('p1', confirm: true);
      engine.dispose();
    });

    test('updatePluginFromMarketplace sends single pluginId key', () async {
      final engine = engineWith((req) async {
        expect(req.method, 'POST');
        expect(req.url.path, '/plugins/update/marketplace');
        final body = jsonDecode(req.body) as Map;
        expect(body.containsKey('plugin_id'), isFalse);
        expect(body['pluginId'], 'p2');
        return http.Response(jsonEncode({'id': 'p2'}), 200);
      });
      await engine.updatePluginFromMarketplace('p2', confirm: true);
      engine.dispose();
    });
  });
}
