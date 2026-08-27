// Unit tests for the NAVI Agent Dart package.
//
// Tests the Dart type system and JSON parsing without requiring
// the native .so/.dylib to be loaded (pure Dart tests).

import 'dart:convert';

import 'package:navi_agent/src/types.dart';
import 'package:test/test.dart';

void main() {
  group('SessionInfo', () {
    test('parses from JSON', () {
      final info = SessionInfo.fromJson({
        'id': 'session-1',
        'projectDir': '/tmp/project',
        'model': 'gpt-5.5',
        'provider': 'openai',
      });
      expect(info.id, equals('session-1'));
      expect(info.projectDir, equals('/tmp/project'));
      expect(info.model, equals('gpt-5.5'));
      expect(info.provider, equals('openai'));
    });

    test('handles missing fields gracefully', () {
      final info = SessionInfo.fromJson({});
      expect(info.id, equals(''));
      expect(info.projectDir, equals(''));
      expect(info.model, equals(''));
      expect(info.provider, equals(''));
    });
  });

  group('TurnResponse', () {
    test('parses from JSON', () {
      final response = TurnResponse.fromJson({
        'sessionId': 'session-1',
        'text': 'Hello, world!',
      });
      expect(response.sessionId, equals('session-1'));
      expect(response.text, equals('Hello, world!'));
    });
  });

  group('ModelInfo', () {
    test('parses from JSON with all fields', () {
      final model = ModelInfo.fromJson({
        'id': 'openai:gpt-5.5',
        'name': 'gpt-5.5',
        'providerId': 'openai',
        'providerLabel': 'OpenAI',
        'taskSize': 'medium',
        'contextWindowTokens': 128000,
      });
      expect(model.id, equals('openai:gpt-5.5'));
      expect(model.contextWindowTokens, equals(128000));
    });

    test('handles null contextWindowTokens', () {
      final model = ModelInfo.fromJson({
        'id': 'test:model',
        'name': 'model',
        'providerId': 'test',
        'providerLabel': 'Test',
        'taskSize': 'small',
      });
      expect(model.contextWindowTokens, isNull);
    });
  });

  group('SkillInfo', () {
    test('parses from JSON', () {
      final skill = SkillInfo.fromJson({
        'id': 'skill-1',
        'name': 'Test Skill',
        'description': 'A test skill',
        'tags': ['test', 'example'],
      });
      expect(skill.id, equals('skill-1'));
      expect(skill.tags, equals(['test', 'example']));
    });
  });

  group('ProviderAccountInfo', () {
    test('parses from JSON', () {
      final info = ProviderAccountInfo.fromJson({
        'providerId': 'openai',
        'providerLabel': 'OpenAI',
        'envVar': 'OPENAI_API_KEY',
        'hasStoredKey': true,
      });
      expect(info.providerId, equals('openai'));
      expect(info.hasStoredKey, isTrue);
    });
  });

  group('TurnOptions', () {
    test('serializes to JSON', () {
      final opts = TurnOptions(thinking: 'high');
      final json = opts.toJson();
      expect(json['thinking'], equals('high'));
      expect(json.containsKey('contentParts'), isFalse);
    });

    test('includes contentParts when non-empty', () {
      final opts = TurnOptions(
        contentParts: [
          {'type': 'text', 'text': 'hello'}
        ],
      );
      final json = opts.toJson();
      expect(json['contentParts'], hasLength(1));
    });
  });

  group('EngineConfig', () {
    test('parses from JSON', () {
      final config = EngineConfig.fromJson({
        'model': {'provider': 'openai', 'name': 'gpt-5.5'},
        'globalConfigPath': '/home/user/.config/navi/config.toml',
        'dataDir': '/home/user/.local/share/navi',
      });
      expect(config.provider, equals('openai'));
      expect(config.modelName, equals('gpt-5.5'));
      expect(config.dataDir, equals('/home/user/.local/share/navi'));
    });
  });

  group('ModelSelectionResult', () {
    test('parses from JSON', () {
      final result = ModelSelectionResult.fromJson({
        'providerId': 'openai',
        'model': 'gpt-5.5',
        'contextWindowTokens': 128000,
        'providerConfigured': true,
        'savedTo': '/home/user/.config/navi/config.toml',
      });
      expect(result.providerId, equals('openai'));
      expect(result.providerConfigured, isTrue);
      expect(result.contextWindowTokens, equals(128000));
    });
  });

  group('SavedSessionInfo', () {
    test('parses from JSON', () {
      final info = SavedSessionInfo.fromJson({
        'id': 'session-1',
        'title': 'My Session',
        'project': '/tmp/project',
        'createdAt': 1700000000,
        'updatedAt': 1700001000,
      });
      expect(info.id, equals('session-1'));
      expect(info.title, equals('My Session'));
      expect(info.createdAt, equals(1700000000));
    });
  });

  group('RuntimeEvent', () {
    test('parses from JSON', () {
      final event = RuntimeEvent.fromJson({
        'version': 1,
        'kind': {
          'AssistantDelta': {'text': 'hello'}
        },
      });
      expect(event.version, equals(1));
      expect(event.kindName, equals('AssistantDelta'));
    });

    test('handles empty kind', () {
      final event = RuntimeEvent.fromJson({'version': 1, 'kind': {}});
      expect(event.kindName, equals('Unknown'));
    });
  });

  group('NaviException', () {
    test('has string representation', () {
      final e = NaviException('test error');
      expect(e.toString(), equals('NaviException: test error'));
    });
  });

  group('JSON helpers', () {
    test('tryParseJson parses valid JSON map', () {
      final result = tryParseJson('{"key": "value"}');
      expect(result, isNotNull);
      expect(result!['key'], equals('value'));
    });

    test('tryParseJson returns null for "null"', () {
      expect(tryParseJson('null'), isNull);
    });

    test('tryParseJson returns null for empty string', () {
      expect(tryParseJson(''), isNull);
    });

    test('tryParseJson returns null for invalid JSON', () {
      expect(tryParseJson('{invalid'), isNull);
    });

    test('tryParseJsonList parses valid JSON array', () {
      final result = tryParseJsonList('["a", "b", "c"]');
      expect(result, isNotNull);
      expect(result, hasLength(3));
      expect(result![0], equals('a'));
    });

    test('tryParseJsonList returns null for non-array', () {
      expect(tryParseJsonList('{"key": "value"}'), isNull);
    });
  });

  group('JSON roundtrip', () {
    test('TurnOptions serializes and roundtrips', () {
      final opts = TurnOptions(
        thinking: 'max',
        contextPackets: [
          {'source': 'File', 'content': 'test.py'}
        ],
      );
      final jsonStr = jsonEncode(opts.toJson());
      final decoded = jsonDecode(jsonStr) as Map<String, dynamic>;
      expect(decoded['thinking'], equals('max'));
      expect(decoded['contextPackets'], hasLength(1));
    });
  });
}
