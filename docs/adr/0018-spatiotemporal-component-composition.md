# ADR 0018 — Spatiotemporal component composition (Matrix kernel host)

## Status

Accepted (prototype, branch `feat/spatiotemporal-compose`).

## Context

NAVI's composition was implicit and fixed:

- **spatial**: the crate graph (`navi-core` → `navi-sdk` → `navi-tui`/`navi-napi`)
  decides what exists together. Replacing a part means recompiling.
- **temporal**: `NaviEngineBuilder` and `RuntimeComponents` decide the wiring at
  `build()` time; `NaviEngineInner.runtime_components`, `host_tools`, the
  `Arc<ToolExecutor>` and the frontend are frozen for the process lifetime.

The engine already had the right seams (`Arc<dyn Tool>`, `RuntimeComponents`,
`Arc<dyn EngineDriver>`, `broadcast<RuntimeEvent>`) and the TUI was already a
*client* of `EngineDriver` — but the TUI **built and owned** the engine
(`navi-tui/src/app.rs`: `build_engine`), so a frontend could not be replaced while
a session lived.

The WASM plugin system (ADRs 0001–0014) was removed in v0.8.0: it required
maintaining two sandbox stacks inside the process. ADR 0013 left an explicit
note: *"deep TUI patching requires a new UI protocol"*.

## Decision

Adopt the [Matrix](https://github.com/enrell/matrix) kernel as NAVI's
composition authority, with NAVI as its **host**:

- `crates/navi-compose` embeds `matrix-core` (in-process, synchronous, no async
  runtime of its own) and implements `CallForwarder`, `LifecycleHook` and
  `EventSink`.
- Every swappable part is a **component** with a manifest (`provides`/`requires`/
  `calls`/`tier`/`trust`/`restart`): the engine, frontends, tool packs,
  providers, compaction/prompt/security strategies.
- The **frontend is a component**. `navi-cli` builds the engine once, activates
  `navi.engine`, starts the session, then activates the requested frontend
  (`--ui`). The supervisor owns the lifecycle.
- **Third-party components run out of process.** `navi-compose` attaches
  `matrix-host`, which spawns a component whose `execution.entrypoint` is a real
  binary with a launch token and a Unix socket (`matrix.component` v0.1). A tool
  plugin's tool joins the session's tool set (`PluginToolProxy`); `navi components
  reload` swaps it by generation, and `navi components install <manifest>` is the
  install path.
- **Engine parts are components too.** Parts the engine calls as Rust trait
  objects (the compaction strategy, the system-prompt builder) live in a typed
  *slot* keyed by capability: activating a provider installs its value for that
  generation, withdrawing it falls back to the previous provider, and the engine
  holds a proxy that resolves the slot per call. `navi components reload
  navi.compaction.terse` therefore changes the running session's compaction
  policy.
- **A frontend can be another process.** A `kind = ui` component with a real
  entrypoint is spawned by the host with the terminal inherited and a control
  socket: the child owns raw mode, receives engine events and sends commands back
  (admitted by the kernel). `navi components reload navi.plugin.tui` hands the
  terminal to a third-party TUI and `navi components reload navi.ui.tui` hands it
  back, both proven in a real PTY. The TUI's own input is read on a thread and
  delivered over a channel, so a blocking terminal poll can never stall the
  control channel.
- **Engine parts can come from another process.** A `kind = compaction` component
  installs a proxy into the engine's compaction slot; the engine asks the plugin
  through the kernel with the instance grant from activation (`invoke_for_ref`),
  so a stale generation can never be reached by capability name. The plugin
  decides (indices to drop/clear), the host applies, and full compaction stays
  with the built-in strategy. `navi components invoke` lets an operator call any
  live component's capability through the same path.
- **Isolation is opt-in and finite.** A process component can declare a
  `[sandbox]`: `matrix-guard` then runs it under `bwrap` + seccomp with rlimits,
  a private `/tmp` and only its own workspace writable. Declaring one makes the
  host policy *secure* (unlisted components are denied), so the policy is built
  from the installed manifests when the host attaches.
- **The host supervises; the kernel does not.** The Matrix process host spawns
  children and speaks the protocol, but restart policy belongs to the
  composition owner: a frontend that fails (`UiExit::Error`, including a child
  that exits non-zero) and a process component whose child died are restarted
  under the manifest's `restart` policy, with a budget (5 attempts / 30 s by
default) and exponential backoff. A restart is a new generation, so the
  successor gets a fresh process and fresh bindings; `restart = "never"` and an
exhausted budget leave the component explicitly down, and a user quit is never
  restarted.
- The terminal is an **exclusive resource** (`TerminalLease`, one owner,
  released on withdraw). A runtime swap quiesces the current generation, releases
  the lease, and activates the successor with fresh bindings.
- Session state stays in the engine; a swapped-in frontend re-attaches through
  `TuiApp::attach_session`.

Kernel invariants (Matrix `docs/CONTRACT.md`) are the acceptance criteria: I01
(single owner), I02 (no admission outside `Active`), I03 (stale generation
rejected at the effect boundary), I04 (Active ⇒ bindings valid), I05/I07
(partial acquisition rolls back; `Disposed` = no pending resources).

## Consequences

Positive:

- Parts of the app are replaceable **at runtime**: `navi components reload
  navi.ui.plain` swaps the frontend of a live session; non-UI components
  (compaction, providers, tool packs) swap the same way.
- Missing dependencies become a visible `Waiting` state with a reason instead of
  a runtime surprise.
- Resources acquired by a failed component are cleaned up by the kernel
  (`CleanupPending` → `Disposed`), not by convention.
- The composition is auditable: generations, bindings, pending work and the
  journal are inspectable (`navi components status|journal|inventory`).
- The process tier for third-party components is implemented on Linux
  (`matrix-host` + `matrix-guard` launch tokens); a sandboxed profile is
  available in Matrix for operators who want `bwrap` + seccomp isolation instead
  of the explicitly-trusted process.

Negative:

- A reload is not free: the terminal handoff is visible (Matrix v0.1 does not
  promise zero downtime).
- In-process components are trusted code; Rust gives no plugin isolation.
- `matrix-core` is an experimental `0.1.0` kernel: it is pinned by path and the
  coupling is confined to `navi-compose`.
- Matrix v0.1 keeps a *registered definition* continuously desired, so
  "withdraw and stay stopped" means removing the definition (the installed
  manifest stays on disk).
- A second Tokio runtime is avoided: frontend threads run inside the CLI's
  runtime via `Handle::enter`.

## Alternatives considered

- **Native in-process plugins (libloading)** — rejected by ADR 0003/0013: no
  memory isolation, credential theft from process memory.
- **WASM-only plugins** — removed in v0.8.0; a second sandbox stack to maintain,
  and it cannot hand over a terminal anyway.
- **Out-of-process frontends only (navi-server/ACP style)** — kept as a valid
  *transport* for frontends, but it does not give the engine's own parts
  (compaction, prompts, tool packs) a composition model.
- **Reimplementing Matrix's invariants inside NAVI** — rejected: the hard part is
  exactly the lifecycle/admission/cleanup semantics, and duplicating them would
  drift.

## References

- Design study: `docs/compose.md`, `crates/navi-compose/README.md`
- Matrix: `CHARTER.md`, `docs/CONTRACT.md`, `docs/LIFECYCLE.md`, `docs/PROTOCOL.md`
