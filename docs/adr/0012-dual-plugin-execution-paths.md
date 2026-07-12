# ADR 0012 — Dual Plugin Execution Paths

## Status
Superseded by [ADR 0013](0013-wasm-only-plugins.md) — native path retired; WASM-only

## Context
NAVI supports two fundamentally different plugin execution models: native in-process
libraries and sandboxed WASM modules. Each has different trust assumptions, security
properties, and capability profiles. The system must manage both paths consistently
while maintaining clear trust boundaries.

## Decision
Plugins execute through one of two paths based on their trust level:

### Path 1: Native In-Process (`Core`, `LocalDev`)

```
Plugin .so/.dylib → libloading → host process memory
```

- Runs in the host process with full memory access
- Capabilities are mediated by host brokers (policy-based enforcement)
- Landlock sandbox provides filesystem restriction (Linux only, post-load)
- No memory isolation — a compromised plugin can read host memory, env vars, secrets
- `Core` plugins are first-party and fully trusted
- `LocalDev` plugins skip signature verification; used for local development only

### Path 2: WASM Module (`Community`, `Signed`)

```
Plugin .wasm → wasmtime::Module → sandboxed linear memory
```

- Runs in a separate linear memory (WASM sandbox)
- Can only access host through explicitly registered imports
- Fuel and memory limits enforced by the runtime
- Capabilities are declared in the manifest and enforced by host brokers
- Signature verification required (Ed25519 over hash bundle)
- Cannot access host memory, env vars, or spawn native threads

### Trust Boundary Summary

| Property | Native | WASM |
|----------|--------|------|
| Memory isolation | None | Full (separate linear memory) |
| Host memory access | Yes | No |
| Env var access | Yes | No (broker-mediated only) |
| Filesystem access | Broker + Landlock | Broker only |
| Network access | Direct (process-level) | Broker only |
| Signature required | No (`Core`/`LocalDev`) | Yes |
| Capability enforcement | Policy-based | Structural + policy-based |
| Performance | Native | ~1.5-3x overhead |

### Security Model Differences

**Native security model:** Trust is placed in the plugin code and the host broker
layer. Landlock adds defense-in-depth for filesystem access. The security boundary
is the process boundary — a native plugin is as trusted as the host process itself
(for `Core`) or as trusted as the developer (for `LocalDev`).

**WASM security model:** Trust is minimized. The plugin runs in a sandbox and can
only interact with the host through declared imports. The security boundary is the
WASM runtime — even a malicious plugin cannot escape the linear memory or access
host resources not exposed through imports. Capability enforcement is both structural
(WASM cannot call what is not imported) and policy-based (host brokers validate
requests).

### Decision Flow

```
Plugin load:
  trust_level = determine_trust_level(manifest)
  match trust_level:
    Core → load_native() → no signature check → broker-mediated access
    LocalDev → load_native() → no signature check → broker + Landlock
    Signed → load_wasm() → verify_signature() → broker-mediated access
    Community → load_wasm() → verify_signature() → validate() → broker-mediated access
```

## Consequences
Positive:
- Clear trust model: native = high trust, WASM = minimal trust
- Both paths share the same host broker infrastructure
- Plugin authors can choose their trust level (and accept the corresponding constraints)
- Security properties are explicit and auditable per trust level

Negative:
- Two code paths to maintain (native loader, WASM runtime)
- Native plugins are a persistent security risk — any vulnerability in a `Core` plugin
  compromises the entire host process
- `LocalDev` is an intentional security escape hatch — must be clearly documented
- Behavioral differences between native and WASM may cause plugin compatibility issues
  (a plugin tested in `LocalDev` native mode may behave differently as a `Community`
  WASM plugin)
- Landlock limitations (post-load, Linux-only) mean native sandboxing is incomplete
