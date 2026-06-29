# @navi-agent/napi

Node.js bindings for the NAVI runtime SDK.

[![npm](https://img.shields.io/npm/v/@navi-agent/napi)](https://www.npmjs.com/package/@navi-agent/napi)

> **Full documentation:** [docs/navi-napi-guide.md](docs/navi-napi-guide.md)

## Install

```sh
npm install @navi-agent/napi
```

Prebuilt native binaries are included automatically for Linux (x64/arm64),
macOS (x64/arm64), and Windows (x64). No Rust toolchain required.

## Usage

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

## Development

To build the native addon from source (requires [Rust](https://rustup.rs)):

```sh
cd crates/navi-napi
npm run build   # debug build
npm test        # build + smoke tests
```
