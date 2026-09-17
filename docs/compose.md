# Spatiotemporal component composition (`navi-compose`)

**Status:** implemented prototype (`feat/spatiotemporal-compose`).
**Kernel:** [Matrix](https://github.com/enrell/matrix) `matrix-core` 0.1 (path dependency).

NAVI's parts are **components**: the engine, each frontend, tool packs, providers
and strategies declare `provides`/`requires`, live in a *context* with a
*generation*, and can be **swapped at runtime** without restarting the process or
losing the session.

```
Compose (host, crates/navi-compose)        Kernel (matrix-core)
├── frontend threads (navi.ui.*)      ◄──  Activated / Withdrawn lifecycle
├── native handlers (navi.*@1)        ◄──  invoke() admission + generation check
├── terminal lease + resources        ◄──  acquire_external / release (I01/I05)
└── async effect drain                ──►  load_manifest / reload / dispose
```

## Why

Before: the composition of NAVI was the crate graph (spatial, fixed at build) and
`NaviEngineBuilder` (temporal, fixed at `build()`). There was no way to replace a
part at runtime, no declared dependency graph, and no cleanup guarantee for
resources a component acquired.

Now: the kernel owns composition (dependency resolution, generations, admission,
resources, lifecycle, journal); the host executes.

## Commands

```bash
navi --ui navi.ui.tui        # default: the ratatui frontend as a component
navi --ui navi.ui.plain      # line-based frontend (no terminal lease)

navi components list         # installed component definitions
navi components status       # live generations/state/TTY ownership of a running session
navi components reload ID    # swap a generation in the running session
navi components journal      # tail of the kernel journal
navi components inventory    # kernel inventory (dependencies + resources)
```

Swapping the frontend of a **live** session:

```bash
# terminal A
navi --ui navi.ui.plain
# terminal B
navi components status
navi components reload navi.ui.tui
```

What happens (Matrix `LIFECYCLE` reload, coordinated replacement):

1. the candidate manifest is validated **without publishing**;
2. the running generation is withdrawn (`Quiescing`): admission is revoked, the
   frontend is asked to quiesce, it restores the terminal and releases the
   exclusive `TerminalLease`;
3. the successor is activated: it acquires the terminal, binds to
   `navi.engine.driver@1` / `navi.engine.events@1` and starts rendering;
4. the session (messages, goal, plan, usage) lives in the engine, so the new
   frontend attaches to the same session via `TuiApp::attach_session`.

Reload v0.1 does **not** promise zero downtime: the terminal handoff is visible.

## Component model

| Concept | NAVI |
|---|---|
| Component | `navi.engine`, `navi.ui.tui`, `navi.ui.plain`, `navi.compaction.default`, … |
| Interface | `navi.engine.driver@1`, `navi.engine.events@1`, `navi.ui.command.<name>@1` |
| Context | one per activation (`app` → engine; the frontend binds to it) |
| Generation | bumped on every activation/reload; stale references are rejected |
| Resource | the terminal (`TerminalLease`, exclusive), plus kernel-tracked handles |
| Effect | commands admitted through the kernel; async execution drained by the supervisor |

Manifests are written to `{data_dir}/compose/manifests/<id>.json` (kernel schema)
and the journal to `{data_dir}/compose/journal.jsonl`.

## Invariants exercised by tests

| Invariant | Where |
|---|---|
| I01 one owner per resource | `terminal_lease_is_exclusive_and_released_on_swap` |
| I02 no admission outside `Active` | `commands_are_admitted_while_active_and_denied_after_withdraw` |
| I03 stale generation rejected | `stale_generation_is_rejected_at_the_effect_boundary` |
| I04 Active ⇒ bindings valid | `missing_provider_leaves_component_waiting_with_reason` |
| I05/I07 partial acquisition rolls back | `failed_frontend_start_rolls_back_the_activation` |
| cascade withdraw | `uninstalling_a_provider_cascades_consumers_to_waiting` |
| reload semantics | `runtime_reload_swaps_the_frontend_generation` |
| TUI honours quiesce | `crates/navi-tui/tests/compose_control.rs` |

## Trust model

In-process components are **trusted first-party code**: Rust provides no plugin
isolation (Matrix CHARTER #6). Untrusted/third-party components belong to the
process tier (`matrix-host` + `matrix-guard`), which is **not** part of this
prototype — the manifest field `execution.entrypoint` already distinguishes
`inproc:<id>` from a real binary path, so the process tier plugs in without
changing the composition model.

## Known limits

- `withdraw` removes the kernel definition (Matrix v0.1 keeps a registered
  definition continuously desired, so "stop and stay stopped" = remove it); the
  installed manifest stays on disk and `activate` restores it.
- The supervisor publishes `status.json` for other processes; the kernel itself
  is in-process only.
- Frontends that never create a session (no credentials yet) leave
  `session_id = None`: a swap then starts a fresh session on the first turn.
- Windows: the terminal lease uses Unix `termios`; the swap path is Unix-first.
- `navi.ui.plain` reads stdin on a helper thread so it can poll the control
  channel. After a swap the retired generation's reader thread may linger until
  the next stdin line arrives, so a *line-based* frontend can lose a line to the
  previous generation. The TUI is unaffected (it owns its input loop and stops
  it on quiesce); a production line frontend should read the TTY directly.
