# @navi/napi

Node.js bindings for the NAVI runtime SDK.

```ts
import { NaviNapiEngineBuilder } from '@navi/napi';

const builder = new NaviNapiEngineBuilder(process.cwd());
builder.configureLearning({ language: 'pt-BR', style: 'socratico' });
builder.hostTool(
  { name: 'consultar_materiais', description: 'Consulta materiais.', kind: 'read' },
  async ({ input }) => ({ ok: true, output: { input } }),
);

const engine = builder.build();
```

Build the local native addon:

```sh
npm run build
```

Run the binding smoke tests:

```sh
npm test
```
