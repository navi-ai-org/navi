# ADR 0001 — Use WASM Component Runtime for Community Plugins

## Status
Accepted (partially implemented — see Implementation State)

## Context
NAVI needs community plugins without granting full process trust. Native .so/.dylib
loaded via libloading gives plugins full access to process memory, environment variables,
filesystem, network, and terminal. This is unacceptable for blind installation.

## Decision
Community plugins MUST run as WASM Components via Wasmtime.

## Implementation State

The current runtime uses **raw WASM modules** loaded via `wasmtime::Module`. The full
Component Model integration (WIT interfaces, `wasmtime::component::*`, typed resource
exports) is planned but incomplete.

**What exists today:**
- `WasmtimePlugin` loads `.wasm` files as raw modules
- Host imports are registered via `Linker` (filesystem, network, TUI, etc.)
- Fuel and memory limits are enforced
- WIT IDL files exist under `wit/` as forward-looking interface contracts

**What is deferred:**
- Component Model instantiation (`wasmtime::component::Component`)
- WIT-generated typed host bindings
- Resource handles and `wasi:resource` exports
- Shared-nothing linking across plugin boundaries

The WIT definitions serve as the **authoritative contract** for what the host will
eventually expose. Plugin authors should target the WIT interfaces even though the
current runtime uses raw module imports.

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
- **Dual-mode situation:** raw module runtime is stable but does not enforce the
  component-level type safety that WIT promises. Plugin manifests declare capabilities
  against the WIT contract, but the runtime cannot yet verify that a raw module's
  imports match its declared capabilities at the type level. This gap is acceptable
  because the host broker layer mediates all access regardless, but it means
  capability enforcement is policy-based (host broker) rather than structural (component
  model). The component model migration closes this gap.
