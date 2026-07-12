# navi-lite

Minimal sealed NAVI runtime for edge and embedded Linux prototypes.

This crate intentionally avoids the desktop SDK, TUI, MCP, dynamic plugins,
registry updates, and local embeddings. It builds a mission-specific runtime
with an empty tool executor and only registers tools required by the mission.

## What This Prototype Is

`navi-lite` is a small, headless NAVI runtime for purpose-built edge agents.
It is not a general coding assistant. A lite runtime is created for one mission,
with a sealed tool allowlist and a prompt that tells the model to use only those
tools.

The first prototype mission is a health check:

1. call `lite_health_check`;
2. call `lite_emit_report`;
3. return a structured JSON report.

## What It Excludes

The lite crate does not depend on:

- `navi-sdk`;
- `navi-tui` or `copland`;
- MCP;
- native or WASM plugin hosts;
- registry sync;
- local embeddings (`candle`, `tokenizers`, `hf-hub`);
- `navi-vfs` / tree-sitter code tools.

`navi-core` keeps those capabilities for normal NAVI builds through default
features. `navi-lite` depends on `navi-core` with `default-features = false`.

## Demo

Run against an OpenAI-compatible gateway:

```bash
NAVI_LITE_API_KEY=... \
NAVI_LITE_BASE_URL=https://your-gateway.example/v1 \
NAVI_LITE_MODEL=your-model \
cargo run -p navi-lite --bin navi-lite -- --json
```

The demo also accepts explicit flags:

```bash
cargo run -p navi-lite --bin navi-lite -- \
  --base-url https://your-gateway.example/v1 \
  --model your-model \
  --api-key "$NAVI_LITE_API_KEY" \
  --json
```

## Public Types

- `LiteConfig`: endpoint, model, API key, project directory, and data directory.
- `LiteMission`: mission id, task text, and allowed tool names.
- `LiteRuntime`: constructs a sealed `AgentRuntime` and runs a mission.
- `LiteMissionResult`: session id, raw agent text, report JSON, and tools used.

## Security Model

The runtime starts with `ToolExecutor::empty_with_security_policy`, then manually
registers only the lite tools. `LiteSecurityPolicy` denies any tool not listed in
the mission allowlist.

The prototype avoids hardcoded production credentials. Use device identity,
short-lived credentials, mTLS, or a fleet gateway for real deployments.

## Verification

Focused checks:

```bash
cargo check -p navi-core --no-default-features
cargo test -p navi-lite -- --test-threads=4
```

Confirm excluded dependencies stay out of the lite graph:

```bash
cargo tree -p navi-lite -e normal --no-default-features \
  | rg "navi-vfs|navi-tui|copland|navi-mcp|navi-plugin-runtime|wasmtime|candle|tokenizers|hf-hub"
```

That command should print no matches.

## Current Limits

- Linux edge/SBC prototype only; not MCU or RTOS.
- No LoRa transport yet; the demo writes JSON to stdout.
- No signed patch or rollback flow yet.
- Health checks are intentionally minimal and do not execute shell commands.
