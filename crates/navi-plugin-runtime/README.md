# navi-plugin-runtime

[![Crates.io](https://img.shields.io/crates/v/navi-plugin-runtime)](https://crates.io/crates/navi-plugin-runtime)
[![License](https://img.shields.io/crates/l/navi-plugin-runtime)](../LICENSE)

WASM plugin runtime for [NAVI](https://github.com/navi-ai-org/navi), powered by [Wasmtime](https://crates.io/crates/wasmtime).

`navi-plugin-runtime` executes WASM plugin components in a sandboxed environment with host callbacks for tool invocation, filesystem access, and HTTP requests.

## What's inside

| Module | Purpose |
|--------|---------|
| `runtime` | `PluginRuntime` — loads and executes WASM components with host callbacks |
| `component` | Component kind detection (command, reactor, etc.) |
| `wit` | WIT (WebAssembly Interface Types) integration |
| `error` | `RuntimeError` types for WASM execution failures |

## Host callbacks

WASM plugins communicate with the host through a `HostCallbacks` interface:

```rust
pub struct HostCallbacks {
    pub log: Box<dyn Fn(&str) + Send + Sync>,
    pub tool_call: Box<dyn Fn(&str, Value) -> Result<Value, String> + Send + Sync>,
}
```

## Sandbox

WASM plugins run in Wasmtime's built-in sandbox:

- No direct filesystem access (must go through host callbacks)
- No network access (must go through host callbacks)
- Memory limits enforced by the WASM runtime
- CPU limits via fuel consumption metering

## Part of the NAVI workspace

This crate depends on [`wasmtime`](https://crates.io/crates/wasmtime).

**Full project:** <https://github.com/navi-ai-org/navi>
**License:** Apache-2.0
