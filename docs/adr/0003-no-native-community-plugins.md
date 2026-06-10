# ADR 0003 — No Native In-Process Community Plugins

## Status
Accepted

## Context
Native .so/.dylib plugins loaded via libloading run in the same process with full
access to memory, env vars, filesystem, network, and terminal. A malicious plugin
can bypass any userspace security policy by patching function pointers, reading
credentials from memory, or spawning background threads.

## Decision
Native in-process plugins are restricted to core plugins or explicit unsafe local-dev
mode. Community plugins MUST NOT run native code in-process.

The `TrustLevel` enum defines four levels:

| Level | Native allowed | Signature required | Sandbox |
|-------|---------------|-------------------|---------|
| `Core` | Yes | No (bundled) | N/A (host code) |
| `Signed` | No | Ed25519 | WASM runtime |
| `Community` | No | Ed25519 | WASM runtime |
| `LocalDev` | Yes | No (skipped) | Landlock filesystem |

- **Core** plugins are built into NAVI or loaded as trusted native libraries for
  first-party functionality. They run in the host process with full trust.
- **LocalDev** plugins skip cryptographic signature verification and may load native
  `.so`/`.dylib` files for local development and testing. This is explicitly an
  unsafe mode — developers opt in knowing the security implications.
- **Community** and **Signed** plugins MUST be WASM Components. No native code
  is permitted.

## Consequences
Positive:
- Eliminates the largest attack surface (arbitrary native code in host process)
- Forces plugin authors to work within the WASM sandbox
- Security policy cannot be bypassed by plugin code
- Clear trust model: Core = trusted native, LocalDev = dev-only native, Community = sandboxed WASM

Negative:
- Some plugins that need native performance require subprocess model
- Plugin development requires WASM toolchain
- Cannot load existing .so plugins as community plugins
- LocalDev mode exists as an escape hatch but is clearly marked unsafe
