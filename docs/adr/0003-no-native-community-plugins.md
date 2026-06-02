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

## Consequences
Positive:
- Eliminates the largest attack surface (arbitrary native code in host process)
- Forces plugin authors to work within the WASM sandbox
- Security policy cannot be bypassed by plugin code

Negative:
- Some plugins that need native performance require subprocess model
- Plugin development requires WASM toolchain
- Cannot load existing .so plugins as community plugins
