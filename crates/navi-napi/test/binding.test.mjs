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

test('builder creates a learning runtime with TypeScript tools and hooks', () => {
  const workspace = mkdtempSync(join(tmpdir(), 'navi-napi-'));
  const builder = new napi.NaviNapiEngineBuilder(workspace);

  builder.configureLearning({
    language: 'pt-BR',
    style: 'socratico',
    maxConsecutiveErrors: 6,
    keepAllAssessments: true,
    exemptToolNames: ['questionario'],
  });
  builder.onToolCall(() => {});
  builder.hostTool(
    {
      name: 'consultar_materiais',
      description: 'Consulta materiais didaticos.',
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
  assert.ok(Array.isArray(engine.listModels()));
});
