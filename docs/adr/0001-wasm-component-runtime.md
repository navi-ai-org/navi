# ADR 0001 — Use WASM Component Runtime for Community Plugins

## Status
Accepted

## Context
NAVI needs community plugins without granting full process trust. Native .so/.dylib
loaded via libloading gives plugins full access to process memory, environment variables,
filesystem, network, and terminal. This is unacceptable for blind installation.

## Decision
Community plugins MUST run as WASM Components via Wasmtime.

## Consequences
Positive:
- Strong runtime boundary (separate linear memory)
- Explicit host imports (plugin can only access what host exposes)
- Portable plugin ABI (same .wasm runs on Linux, macOS, Windows)
- Capability-oriented design (WASI model)
- Built-in resource limits (fuel, memory, timeout)

Negative:
- More complex tooling (WIT, component model)
- Native libraries require subprocess or separate integration
- Performance overhead for compute-heavy tasks (~1.5-3x)
- Plugin authors must target wasm32-wasip2
