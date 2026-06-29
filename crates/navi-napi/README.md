# @navi-agent/napi

Node.js bindings for the NAVI runtime SDK.

> **Full documentation:** [docs/navi-napi-guide.md](docs/navi-napi-guide.md)

```ts
import { NaviNapiEngineBuilder } from '@navi-agent/napi';

const builder = new NaviNapiEngineBuilder(process.cwd());
builder.configureLearning({ language: 'pt-BR', style: 'socratico' });
builder.hostTool(
  { name: 'consultar_materiais', description: 'Consulta materiais.', kind: 'read' },
  async ({ input }) => ({ ok: true, output: { input } }),
);

const engine = builder.build();

const session = await engine.startSession();
const response = await engine.sendTurn(session.id, 'Ola!');
console.log(response.text);
await engine.closeSession(session.id);
```

Build the local native addon:

```sh
npm run build
```

Run the binding smoke tests:

```sh
npm test
```
