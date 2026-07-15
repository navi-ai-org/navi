import assert from 'node:assert/strict';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const napi = require('../index.js');

test('exports public NAPI classes', () => {
  assert.equal(typeof napi.NaviNapiEngineBuilder, 'function');
  assert.equal(typeof napi.NaviNapiEngine, 'function');
});

test('builder creates a runtime with TypeScript tools and hooks', () => {
  const workspace = mkdtempSync(join(tmpdir(), 'navi-napi-'));
  const builder = new napi.NaviNapiEngineBuilder(workspace);

  builder.onToolCall(() => {});
  builder.hostTool(
    {
      name: 'lookup_docs',
      description: 'Look up project documentation.',
      kind: 'read',
      inputSchema: { type: 'object' },
    },
    async ({ invocationId, input }) => ({
      ok: true,
      output: { invocationId, input },
    }),
  );

  const engine = builder.build();
  assert.equal(typeof engine.listModels, 'function');
  const models = engine.listModels();
  assert.ok(Array.isArray(models));
  assert.ok(models.length > 0);
  const first = models[0];
  assert.ok(Array.isArray(first.effortOptions), 'listModels should include effortOptions');
  assert.equal(typeof first.effortBinary, 'boolean');
  assert.ok(first.effortOptions.length > 0);
  assert.equal(typeof first.effortOptions[0].value, 'string');
  assert.equal(typeof first.effortOptions[0].label, 'string');

  assert.equal(typeof engine.listTuiExtensions, 'function');
  assert.equal(typeof engine.listTuiExtensionCommands, 'function');
  assert.equal(typeof engine.pluginInstallPathWithMeta, 'function');

  const extensions = engine.listTuiExtensions();
  assert.ok(Array.isArray(extensions), 'listTuiExtensions should return an array');

  const extensionCommands = engine.listTuiExtensionCommands();
  assert.ok(
    Array.isArray(extensionCommands),
    'listTuiExtensionCommands should return an array',
  );

  const config = engine.loadedConfig();
  assert.ok(config && typeof config === 'object');
  assert.ok(config.tui && typeof config.tui === 'object');
  assert.equal(typeof config.tui.desktopNotifications, 'boolean');

  // Invalid path should reject cleanly without crashing the binding.
  assert.throws(
    () => engine.pluginInstallPathWithMeta('/tmp/navi-missing-plugin-path', false),
    /./,
  );
});
