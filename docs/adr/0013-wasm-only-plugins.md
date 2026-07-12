# ADR 0013 — WASM-Only Plugins (Retire Native libloading Path)

## Status
Accepted

## Context
ADR 0012 defined dual execution paths: native in-process libraries (`libloading`)
for `Core` / `LocalDev`, and WASM for `Community` / `Signed`. Native plugins share
the host process address space and cannot be safely offered via marketplace or
blind install. Defense-in-depth (Landlock, brokers) does not prevent credential
theft from process memory.

Product direction: one plugin runtime for all extension packages.

## Decision
**Plugins are WASM-only.**

| Path | Status |
|------|--------|
| WASM (`wasmtime` + host brokers) | **Only** supported plugin runtime |
| Native `.so` / `.dylib` via `libloading` | **Retired** — config ignored with warning |
| `navi-plugin-host` | **Removed** from the workspace (ADR 0013 cleanup) |

### Trust levels (still WASM)

| Level | Signature | Typical source |
|-------|-----------|----------------|
| `Community` | Required (Ed25519) | Marketplace install |
| `Signed` | Required | Curated signed packages |
| `LocalDev` | Skipped | `navi plugin install <path>` during development |
| `Core` | N/A | First-party code in the binary (not a dynamic plugin) |

### Marketplace package kinds

All kinds install as WASM plugin packages under `{data_dir}/plugins/`:

| Kind | Install semantics |
|------|-------------------|
| `plugin` | Tools register with `ToolExecutor` |
| `skill` | WASM package; skill activation remains skill system / session |
| `mcp` | WASM package; optional `mcp.json` merged by user into global MCP config |
| `integration` | WASM package (bots/sidecars may need env secrets) |

Native TUI panels (`TuiComponent` via libloading) are not loaded. Future TUI
extension must use a host-mediated protocol, not in-process widgets.

## Consequences
Positive:
- Single security model for marketplace and local packages
- No process-memory plugin attack surface from third-party code
- Clear LocalDev escape hatch that is still sandboxed WASM

Negative:
- Deep TUI patching requires a new UI protocol (not native frames)
- Plugin authors must target `wasm32` / component toolchain
- The `navi-plugin-host` crate was deleted; do not reintroduce in-process native plugins

## Supersedes
- ADR 0012 dual-path native branch (native path removed from runtime)
- ADR 0011 Landlock-for-native as a product requirement (crate may remain unused)
- ADR 0003 still holds: no native community plugins (now: no native plugins at all)
