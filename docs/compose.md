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

The **frontend role is exclusive**: activating a frontend withdraws any other
live frontend first (one terminal, one owner), so `navi.ui.plain` replaces
`navi.ui.tui` instead of running beside it.

Shutdown is signal-driven: SIGTERM/SIGINT/SIGHUP are handled by the supervisor,
which asks the running frontend to quit and exits once it has (each frontend
restores the terminal on its way out).

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
| out-of-process plugin | `crates/navi-plugin-demo/tests/process_tier.rs` |

## Engine parts as components (typed slots)

Some parts of NAVI are not JSON capabilities but Rust trait objects the engine
calls directly. Those go through a **typed slot** keyed by the capability:

* activating a provider installs its value for that generation;
* withdrawing it pops the slot back to the previous provider;
* the engine holds a *proxy* (`ComposeCompaction`, `ComposePrompt`) that reads
the slot on every call — so a swap changes behavior on the next use, with no
restart and no rebuild.

| Capability | Component | Effect when active |
|---|---|---|
| `navi.compaction@1` | `navi.compaction.default` | NAVI's standard micro-compaction (read-only tool results) |
| `navi.compaction@1` | `navi.compaction.terse` | clears **every** old tool result |
| `navi.prompt@1` | `navi.prompt.branded` | stamps a marker into the base instructions |

```bash
navi components reload navi.compaction.terse   # engine compaction changes mid-session
navi components reload navi.prompt.branded     # next turn uses the other prompt builder
```

The kernel still decides *who* is active (generations, admission, withdraw
cascade); the host owns the typed handle. `navi components status` shows both
providers with their generations, and the log records every slot install:

```
installing component into the compaction slot component="navi.compaction.terse" generation=1 role="navi.compaction@1"
```

## Process tier: third-party components

A component whose `execution.entrypoint` is a real binary runs **out of process**
(Linux): the supervisor attaches `matrix-host`, which spawns the plugin with a
launch token and a Unix socket, and dispatch flows
`kernel → host forwarder → child session → plugin`.

### Capability plugins (tools, …)

```toml
id = "navi.plugin.roll_dice"
kind = "tool"
tier = "local"
capabilities = ["navi.plugin.info@1", "navi.tool.roll_dice.definition@1", "navi.tool.roll_dice@1"]
execution = { kind = "process", entrypoint = "/path/to/plugin", args = ["--matrix-sock", "{sock}", "--id", "{id}"], timeout_ms = 15000 }
```

### Frontend plugins (another TUI)

A `kind = ui` component with a real entrypoint is spawned by the **host** (not by
`matrix-host`, which runs children without a terminal on purpose). The child owns
the terminal and talks to the host over a control socket:

```toml
id = "navi.plugin.tui"
kind = "ui"
capabilities = ["navi.plugin.tui@1"]
execution = { kind = "process", entrypoint = "/path/to/navi-plugin-tui", args = ["--control", "{control}", "--session", "{session}"], timeout_ms = 15000 }
```

* host → plugin: `{"type":"event", …}`, `{"type":"quiesce"}`, `{"type":"shutdown"}`;
* plugin → host: `{"type":"command","text":…}` (admitted by the kernel), `{"type":"quit"}`.

The host keeps the exclusive terminal lease as *bookkeeping* (the child performs
the OS effect), so the successor only starts after the child exits.
`crates/navi-plugin-tui` is a working example (~300 lines, crossterm only).

```bash
navi components install plugin-tui.toml
navi components reload navi.plugin.tui   # the third-party TUI takes over the terminal
navi components reload navi.ui.tui       # and hands it back
```

`crates/navi-cli/tests/pty_plugin_tui_swap.rs` proves the round trip in a real
PTY: built-in TUI → third-party process TUI → built-in TUI (generation 2) → clean
exit, with both directions recorded in the kernel journal.

### Rules that keep the tiers apart

- a tool plugin declares `navi.tool.<tool>@1` plus `navi.tool.<tool>.definition@1`
  (one definition capability per tool, so several plugins coexist without
  ambiguity); NAVI reads the definition and registers the tool for the session;
- the `inproc:<id>` sentinel never reaches the process host (the composite hook
  filters it), and frontends are never spawned by it either — the host runs them;
- the plugin inherits an empty environment and no terminal: it talks to NAVI only
  through the socket.

## Trust model

In-process components are **trusted first-party code**: Rust provides no plugin
isolation (Matrix CHARTER #6). Third-party components belong to the **process
tier**: their own process, a launch token, an empty environment, no terminal.
Operators who want stronger isolation can use Matrix's sandbox profile (`bwrap` +
seccomp via `matrix-guard`) by declaring the component in `HostPolicy` with a
`Sandbox` instead of `None` (explicitly trusted process).

## Supervision (restart per policy)

The kernel decides composition; **the host owns restart policy** (the Matrix
process host spawns and speaks the protocol, but has no restart policy of its
own). NAVI supervises two kinds of failure:

* a **frontend** that ends with `UiExit::Error` (an in-process frontend that
  panics or returns an error; a process frontend whose child exits non-zero);
* a **process component whose child died** while the kernel still reports it
  `Active` (detected with `Host::child_running` after a 2 s startup grace).

Each failure is scheduled under the component's manifest policy:

| `restart` | Behaviour |
|---|---|
| `on-failure` (default) | restart with exponential backoff |
| `permanent` | same as `on-failure` (the component is meant to be up) |
| `never` | stay down; the last decision says `failed (policy: never)` |

Budget and backoff are configurable (`Compose::set_restart_policy`): by default
at most 5 attempts per 30 s, backoff `250 ms · 2ⁿ` capped at 4 s. When the budget
is exhausted the component is a *supervised failure*: it stays down and the
decision is visible — no restart storm.

A restart is a **new generation** (the kernel withdraws the dead instance and
activates a successor), so a restarted plugin gets a fresh process, fresh
bindings and a fresh journal entry. A frontend the *user* quits (`UiExit::Quit`)
is never restarted, and a restart that is already waiting for its backoff does
not consume another attempt.

```bash
navi components status
# id                       state          gen    instance tty    restarts  waiting
# navi.plugin.tui          Active         2      6        yes    1
# supervision: navi.plugin.tui → restarted (generation 2)
```

Proven by:

| Behaviour | Evidence |
|---|---|
| crashed frontend restarts in a new generation | `a_crashed_frontend_is_restarted_with_a_new_generation` |
| budget is finite and reported | `restart_budget_is_finite_and_reported` |
| `restart = "never"` stays down | `restart_policy_never_leaves_the_component_down` |
| a clean quit is not a failure | `a_frontend_the_user_quits_is_not_restarted` |
| killed plugin child comes back | `a_killed_plugin_child_is_restarted_by_supervision` |

## Known limits

- The TUI reads terminal input on a dedicated thread and delivers events over a
  channel. `crossterm::event::poll` was observed to block indefinitely in a bare
  PTY (see `crates/navi-tui/examples/poll_probe.rs`), and a frontend that polls
  the terminal inline can never honour a swap while it waits for a key. The
  reader thread may stay blocked in `event::read()` after a quiesce and consume
  one key from a successor; a production line frontend should read the TTY
  directly.
- `withdraw` removes the kernel definition (Matrix v0.1 keeps a registered
  definition continuously desired, so "stop and stay stopped" = remove it); the
  installed manifest stays on disk and `activate` restores it.
- The supervisor publishes `status.json` for other processes; the kernel itself
  is in-process only.
- Frontends that never create a session (no credentials yet) leave
  `session_id = None`: a swap then starts a fresh session on the first turn.
- Windows: the terminal lease uses Unix `termios`; the swap path is Unix-first.
- `navi.ui.plain` reads stdin on a helper thread so it can poll the control
  channel. crossterm (used by the TUI) keeps an internal stdin reader alive for
  the process lifetime, so a line-based successor may not receive typed lines
  after a TUI generation; end such a session with a signal (handled cleanly) or
  start with the line frontend. The TUI is unaffected — it owns its input loop
  and stops it on quiesce.
