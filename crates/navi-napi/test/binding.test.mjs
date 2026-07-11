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
});
