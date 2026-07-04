# navi-plugin-orchestrator

[![Crates.io](https://img.shields.io/crates/v/navi-plugin-orchestrator)](https://crates.io/crates/navi-plugin-orchestrator)
[![License](https://img.shields.io/crates/l/navi-plugin-orchestrator)](../LICENSE)

Plugin orchestration layer for [NAVI](https://github.com/navi-ai-org/navi).

`navi-plugin-orchestrator` coordinates the full plugin lifecycle — from discovery and installation through runtime loading — bridging the manifest, broker, and runtime crates.

## What it does

- **Discovery** — scans configured plugin directories for installed plugins
- **Installation** — coordinates with `navi-plugin-broker` to fetch and stage plugins
- **Loading** — delegates to `navi-plugin-runtime` for WASM execution
- **Tool adaptation** — wraps WASM plugin tools for the engine's `ToolExecutor`

## Feature flags

| Flag | Default | Description |
|------|---------|-------------|
| `wasm-runtime` | off | Enable WASM plugin runtime support via `navi-plugin-runtime` |

## Part of the NAVI workspace

This crate depends on [`navi-core`](https://crates.io/crates/navi-core), [`navi-plugin-manifest`](https://crates.io/crates/navi-plugin-manifest), and [`navi-plugin-broker`](https://crates.io/crates/navi-plugin-broker).

**Full project:** <https://github.com/navi-ai-org/navi>
**License:** Apache-2.0
