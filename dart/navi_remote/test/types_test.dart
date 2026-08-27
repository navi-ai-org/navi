import 'package:navi_remote/navi_remote.dart';
import 'package:test/test.dart';

void main() {
  group('SessionInfo.fromJson', () {
    test('parses camelCase', () {
      final s = SessionInfo.fromJson({
        'id': 'sess-1',
        'projectDir': '/home/proj',
        'model': 'gpt-4',
        'provider': 'openai',
        'title': 'Hello',
      });
      expect(s.id, 'sess-1');
      expect(s.projectDir, '/home/proj');
      expect(s.model, 'gpt-4');
      expect(s.provider, 'openai');
      expect(s.title, 'Hello');
    });

    test('parses snake_case project_dir', () {
      final s = SessionInfo.fromJson({
        'id': 'x',
        'project_dir': '/tmp/a',
        'model': 'm',
        'provider': 'p',
      });
      expect(s.projectDir, '/tmp/a');
    });
  });

  group('ModelInfo.fromJson', () {
    test('parses dual naming and effort options', () {
      final m = ModelInfo.fromJson({
        'id': 'openai:gpt',
        'name': 'gpt',
        'provider_id': 'openai',
        'provider_label': 'OpenAI',
        'task_size': 'large',
        'context_window_tokens': 128000,
        'supports_thinking': true,
        'reasoning_levels': ['low', 'high'],
        'effort_binary': false,
        'effort_options': [
          {'id': 'high', 'label': 'High'},
        ],
      });
      expect(m.providerId, 'openai');
      expect(m.providerLabel, 'OpenAI');
      expect(m.contextWindowTokens, 128000);
      expect(m.reasoningLevels, ['low', 'high']);
      expect(m.effortOptions, isNotEmpty);
      expect(m.displayKey, 'openai:gpt');
    });
  });

  group('EngineConfig.fromJson', () {
    test('reads model + projectDir dual keys', () {
      final c = EngineConfig.fromJson({
        'model': {'provider': 'anthropic', 'name': 'claude'},
        'project_dir': '/home/navi',
        'dataDir': '/data',
      });
      expect(c.provider, 'anthropic');
      expect(c.modelName, 'claude');
      expect(c.projectDir, '/home/navi');
      expect(c.dataDir, '/data');
    });
  });

  group('SavedSessionInfo.fromJson', () {
    test('parses timestamps in seconds', () {
      final s = SavedSessionInfo.fromJson({
        'id': 'abc',
        'title': 'My chat',
        'project': '/p',
        'createdAt': 1700000000,
        'updatedAt': 1700001000,
      });
      expect(s.displayTitle, 'My chat');
      expect(s.createdAt, 1700000000);
    });

    test('falls back to id for empty title', () {
      final s = SavedSessionInfo.fromJson({
        'id': 'only-id',
        'project': '',
      });
      expect(s.displayTitle, 'only-id');
    });
  });

  group('SkillInfo / SkillWriteRequest', () {
    test('parses skill', () {
      final s = SkillInfo.fromJson({
        'id': 'sk',
        'name': 'Skill',
        'allow_tools': ['bash'],
        'tags': ['a'],
        'editable': true,
        'scope': 'user',
      });
      expect(s.allowTools, ['bash']);
      expect(s.editable, isTrue);
    });

    test('write request dual-keys allow_tools', () {
      final json = SkillWriteRequest(
        name: 'n',
        instructions: 'do x',
        allowTools: ['bash'],
      ).toJson();
      expect(json['allow_tools'], ['bash']);
      expect(json['allowTools'], ['bash']);
      expect(json['scope'], 'user');
    });
  });

  group('LoadedSession.fromJson', () {
    test('live resume shape', () {
      final loaded = LoadedSession.fromJson({
        'id': 's1',
        'projectDir': '/proj',
        'model': 'm',
        'provider': 'p',
        'title': 'T',
        'snapshot': {
          'id': 's1',
          'events': [],
          'project': '/proj',
        },
      });
      expect(loaded.session.id, 's1');
      expect(loaded.session.model, 'm');
      expect(loaded.snapshot?['events'], isEmpty);
    });

    test('legacy bare snapshot shape', () {
      final loaded = LoadedSession.fromJson({
        'id': 'old',
        'project': '/x',
        'title': 'Old',
        'events': [
          {'role': 'user', 'text': 'hi'},
        ],
      });
      expect(loaded.session.id, 'old');
      expect(loaded.session.projectDir, '/x');
      expect(loaded.snapshot?['events'], isNotEmpty);
    });
  });

  group('PermissionMode / AgentMode', () {
    test('permission parse accepts aliases', () {
      expect(PermissionMode.parse('yolo'), PermissionMode.yolo);
      expect(PermissionMode.parse('accept-edits'), PermissionMode.acceptEdits);
      expect(PermissionMode.parse('accept_edits'), PermissionMode.acceptEdits);
      expect(PermissionMode.parse('AUTO'), PermissionMode.auto);
      expect(PermissionMode.parse('nope'), PermissionMode.restricted);
      expect(PermissionMode.acceptEdits.apiValue, 'accept_edits');
    });

    test('agent mode parse', () {
      expect(AgentMode.parse('plan'), AgentMode.plan);
      expect(AgentMode.parse('default').apiValue, 'default');
    });
  });

  group('RuntimeEvent / helpers', () {
    test('kindName from kind map', () {
      final e = RuntimeEvent.fromJson({
        'version': 1,
        'kind': {
          'AssistantDelta': {'text': 'hi'},
        },
      });
      expect(e.kindName, 'AssistantDelta');
      expect(e.kindData['text'], 'hi');
    });

    test('asJsonMapList wraps arrays', () {
      final list = asJsonMapList([
        {'a': 1},
        {'b': 2},
      ]);
      expect(list.length, 2);
      expect(asJsonMapList({'value': [{'x': 1}]}).length, 1);
    });

    test('asStringList', () {
      expect(asStringList(['a', 'b']), ['a', 'b']);
      expect(asStringList({'value': [1, 2]}), ['1', '2']);
    });
  });

  group('VoiceEvent', () {
    test('kind from nested kind', () {
      final v = VoiceEvent.fromJson({
        'kind': {'Partial': {'text': 'he'}},
      });
      expect(v.kindName, 'Partial');
    });
  });
}
