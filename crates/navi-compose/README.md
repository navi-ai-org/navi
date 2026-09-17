# navi-compose

Spatiotemporal component composition for NAVI — the **host** of the
[Matrix](https://github.com/enrell/matrix) kernel.

NAVI's parts (engine, frontends, tools, providers, compaction strategies) are
declared as *components* with `provides`/`requires`, generations, call policies,
resources and a lifecycle. The kernel decides composition; this crate executes:

```
Compose (host)                          Kernel (matrix-core)
├── frontend threads (navi.ui.*)   ◄──  Activated / Withdrawn lifecycle
├── native handlers (navi.*@1)     ◄──  invoke() admission + generation check
├── terminal lease + resources     ◄──  acquire_external / release (I01/I05)
└── async effect drain             ──►  load_manifest / reload / dispose
```

## Runtime frontend swap

```bash
navi --ui navi.ui.plain        # start with the line frontend
# in another terminal:
navi components status         # generations, waiting reasons, TTY ownership
navi components reload navi.ui.tui
```

The running supervisor quiesces the old generation (the frontend releases the
exclusive terminal lease), the kernel withdraws it, and the successor is
activated with fresh bindings — the session, goal and history survive because
they live in the engine.

## Layout

| Module | Role |
|---|---|
| `manifest` | `ComponentManifest` (TOML/JSON) → kernel manifest |
| `native` | `Handler` table + `NativeForwarder` (`CallForwarder`) |
| `host` | `Compose` supervisor: install/activate/withdraw/reload, leases, effects |
| `ui` | `UiFrontend` contract + `navi.ui.tui` / `navi.ui.plain` |

## Trust model

In-process components are **trusted first-party code** — Rust provides no plugin
isolation (Matrix CHARTER #6). Third-party components belong to the process tier
(`matrix-host` + guard), which this prototype leaves for a later phase.

## Status

Prototype (`feat/spatiotemporal-compose` worktree). `matrix-core` is an
experimental `0.1.0` kernel pinned by path dependency.
