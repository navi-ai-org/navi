# Implementation report — spatiotemporal composition in NAVI

This is the *report* half of the study `~/lab/navi-matrix/DESIGN.md`: what the
design proposed, what was built, where it deviates, and what remains.

* **Where:** git worktree `/home/lain/lab/navi-wt/spatiotemporal`, branch
  `feat/spatiotemporal-compose`, base `2e4c92a`, head `763b2d8` — 12 commits,
  52 files, +11696/−62. `projects/navi` (main) and `projects/matrix` were left
  untouched by this work.
* **Design:** `~/lab/navi-matrix/DESIGN.md` (sections below are referenced as §N).
* **User docs:** `docs/compose.md`, `docs/adr/0018-spatiotemporal-component-composition.md`,
  and one README per plugin crate.

```
78aa0d6 compose: spatiotemporal component composition with runtime frontend swap
14e0471 fix: exclusive frontend role, self-exit reaping and signal shutdown
0dd3d64 compose: process tier — third-party plugins out of process
9694f16 compose: engine parts as components (typed capability slots)
8be3ab8 compose: a third-party TUI as a process-tier frontend
586e02a compose: supervision — restart policy, budget and backoff
8cf63c9 compose: opt-in finite sandbox for process components
67633b7 compose: an engine part served by a plugin process
bf084e0 compose: every engine seam is a component (security, harness, hooks)
7533320 compose: seams served by a process component (security, harness, hooks)
35cca5c docs: prompt is the one seam without a process contract
763b2d8 compose: a plugin process can render the system prompt
```

## 1. Phases (§8)

| Phase | Planned | Delivered |
|---|---|---|
| 0 — contracts | `navi-compose` with manifests, FSM, generations, `Waiting` with reason, JSONL journal, `navi components list\|status\|reload` | ✅ plus `install`, `manifest`, `journal`, `inventory`, `invoke`; the kernel is `matrix-core` itself (decision A) |
| 1 — seams as components | tools, provider, compaction/prompt/security/harness resolved per invocation with generation checks | ✅ all five `RuntimeComponents` (security, harness, prompt, compaction, hooks) + tools; **provider** left as-is (already runtime-swappable through the engine's own `Arc<RwLock<Arc<dyn ModelProvider>>>`) |
| 2 — UI as component (the request) | `TerminalLease`, `UiFrontend`, quiesce, two first-party UIs, `navi --ui <id>`, reload | ✅ `navi.ui.tui`, `navi.ui.plain`, exclusive frontend role, quiesce/`UiExit`, `navi --ui <id>` |
| 3 — process tier | `matrix-host` + `matrix-guard`, protocol, supervision with budget/backoff, third-party UI, sandbox denial | ✅ all of it, plus third-party plugins for tools, compaction, security+hooks and prompt |
| 4 — remote | `matrix-runtime` (mTLS), remote `navi-server` frontend | ❌ not attempted (explicitly optional in the design) |

**Success criterion of the original request** (§8, last paragraph) — *“`navi
components reload navi.ui.<x>` swaps the frontend of a live session without
losing state, without stale handles and with the terminal restored — and a
third-party UI does the same out of process”* — is met: see `pty_compose_swap`
(built-in TUI → line frontend → back, same session) and `pty_plugin_tui_swap`
(built-in TUI → third-party process TUI → built-in TUI generation 2), both in a
real PTY.

## 2. Acceptance tests (§8, N01–N06)

| # | Requirement | Test (crate) |
|---|---|---|
| N01 | swap frontends, same session/history | `runtime_reload_swaps_the_frontend_generation`, `session_binding_is_shared_by_every_generation` (navi-compose), `pty_compose_swap` (navi-cli) |
| N02 | lease is exclusive, never corrupts | `terminal_lease_is_exclusive_and_released_on_swap`, `activating_another_frontend_withdraws_the_previous_one` (navi-compose), `pty_compose_swap` |
| N03 | crash of generation N → terminal restored, N+1 per `restart`, journaled | `a_crashed_frontend_is_restarted_with_a_new_generation`, `restart_budget_is_finite_and_reported`, `restart_policy_never_leaves_the_component_down`, `a_frontend_the_user_quits_is_not_restarted` (navi-compose), `a_killed_plugin_child_is_restarted_by_supervision` (navi-plugin-demo) |
| N04 | command from the retired UI → `stale-generation` | `commands_are_admitted_while_active_and_denied_after_withdraw`, `stale_generation_is_rejected_at_the_effect_boundary` (navi-compose) |
| N05 | withdraw with work in flight: `drain_ms`, late commit rejected and journaled | `stale_generation_is_rejected_at_the_effect_boundary` + the manifests' `on_withdraw = "drain"` policy; the kernel settles tickets (a full in-flight *turn* is drained by the engine, not by a NAVI-specific test) |
| N06 | missing dependency → `Waiting` with visible reason | `missing_provider_leaves_component_waiting_with_reason`, `uninstalling_a_provider_cascades_consumers_to_waiting` (navi-compose) |

Phase-1 acceptance (§8) maps to
`compaction_strategy_swaps_at_runtime_through_the_slots` (swap mid-session),
`a_plugin_process_can_be_the_engines_compaction_strategy` (out of process) and
`stale_generation_is_rejected_at_the_effect_boundary` (retired handle).

Phase-3 acceptance (§8) maps to
`third_party_plugin_runs_out_of_process_and_swaps_by_generation`,
`a_sandboxed_plugin_cannot_see_the_host_filesystem` (denial, A/B against
`a_trusted_plugin_reaches_the_host_filesystem`) and
`in_process_components_are_never_spawned_as_processes`.

## 3. What the design did not anticipate

1. **Process-tier engine parts.** The design stopped at “UI and tools as
   components”; the shipped version lets a plugin own *any* engine seam from its
   own process (compaction, security, harness observations, hooks, prompt) with a
   JSON contract per capability, dispatched by capability prefix.
2. **Supervision.** `matrix-host` spawns and speaks the protocol but has no
   restart policy, so the host had to own it: `restart` policy, attempt budget,
   exponential backoff, and detection of a child that died while the kernel still
   says `Active`.
3. **Sandbox.** `matrix-guard`'s `bwrap` + seccomp profile is now reachable from a
   manifest (`[sandbox]`), with the host policy derived from installed manifests.
4. **`navi components invoke`.** Calling a live component's capability through
   the kernel (request/answer files, like reloads) turned out to be the best
   plugin-authoring diagnostic *and* the most convincing live proof.
5. **Two behaviours the tests forced out of hiding:**
   * `crossterm::event::poll` blocks indefinitely in a bare PTY
     (`crates/navi-tui/examples/poll_probe.rs`), so the TUI reads input on a
     thread and delivers events over a channel — without that, a swap could never
     be honoured while the loop waited for a key;
   * a `Slot::install` that updated in place meant “reload the built-in provider”
     could not take the slot back; re-installing now moves the provider to the top.

## 4. Deviations from the proposed shape (§5.4)

* In-process components are declared to the kernel with the `inproc:<id>`
  sentinel because the kernel's in-process dispatch is a closed set of demo
  reducers (§9 decision D, still open); every NAVI capability is served by the
  host's `NativeForwarder`, and the sentinel is filtered so the process host
  never tries to spawn it.
* The frontend contract is `navi.ui.command.<name>@1` plus the engine driver and
  event capabilities — not a `navi.ui.frontend@1` interface (§11.1).
* Process-tier frontends are spawned by the **host**, not by `matrix-host`:
  `matrix-guard` deliberately runs children with `Stdio::null()` and no terminal.
  The child inherits stdio and gets a control socket (`{control}`/`{session}`),
  and the host keeps the exclusive lease as bookkeeping only.
* `SystemPromptInput` is sent to a prompt plugin as a **projection** (the live
  prompt cache and config stay on the host).

## 5. Risks (§9) — status

| # | Risk | Status |
|---|---|---|
| 1 | synchronous kernel + tokio | the forwarder never awaits; process calls are blocking round-trips with timeouts |
| 2 | reentrancy deadlock | avoided by design: proxies call `invoke_for_ref` with no locks held; the host documents its lock order (`status()` snapshots before locking `running`) |
| 3 | `matrix-core` 0.1 experimental | path dependency; the coupling lives in `navi-compose` (+ the plugin SDK crates) |
| 4 | global terminal hooks | the panic hook is ownership-guarded by the lease; signals are forwarded by the supervisor |
| 5 | crossterm/termios | discovered and worked around (thread + channel); verified in a real PTY (portable-pty) and under `script`; WezTerm/Kitty/Ghostty not exercised here |
| 6 | Windows | Unix-first, documented |
| 7 | “one binary” identity | in-process core is the default; the process tier is opt-in |
| 8 | no zero-downtime | documented (“a reload pauses the terminal”) |
| 9 | third parties without WASM | respected — process + guard + approval, no WASM reintroduced |
| 10 | AGENTS.md #4 (no daemon) | the supervisor is in-process; IPC exists only for process components |

## 6. Open decisions (§9)

| | Decision | Status |
|---|---|---|
| A | adopt `matrix-core` as a path dependency | ✅ adopted (as recommended) |
| B | UI scope `app` vs `session:<id>` | ✅ `app`: the frontend role is exclusive, the session binding is a shared resource |
| C | who performs the swap | ✅ the supervisor (`navi components reload` writes a request file; the supervisor applies it within ~50 ms) |
| D | contribute `ExecutionKind::Native` upstream | ❌ open — the `inproc:` sentinel remains |
| E | promote `EngineDriver` to a versioned stable contract | ❌ open — it is the engine facade compose uses; `RuntimeEvent.version` exists |

## 7. Remaining limits

* The process tier and the terminal lease are Unix/Linux-first.
* The TUI's input reader thread may consume one key after a quiesce (the
  crossterm poll behaviour above).
* The process-host policy is fixed when the host attaches: a sandbox declared
  mid-session logs a warning and applies on the next start.
* Phase 4 (remote hosts) was not attempted.

## 8. Reproducing it

```bash
# tests
cargo test -p navi-compose -p navi-plugin-demo -p navi-plugin-tui \
           -p navi-plugin-compaction -p navi-plugin-security -p navi-plugin-prompt -p navi-cli

# a composed session
cargo build -p navi-cli -p navi-plugin-tui -p navi-plugin-compaction \
            -p navi-plugin-security -p navi-plugin-prompt
navi --ui navi.ui.tui

# swap the frontend to a third-party process TUI, and back
navi components reload navi.plugin.tui
navi components reload navi.ui.tui

# swap engine parts and ask the live plugin what it would do
navi components reload navi.plugin.compaction
navi components invoke navi.plugin.compaction navi.compaction@1 --input '{"messages":[],"timestamps":[],"gap_threshold_minutes":60}'
navi components reload navi.plugin.security
navi components invoke navi.plugin.security navi.security@1 --input '{"op":"validate_tool","tool":{"name":"web_fetch"},"input":{}}'

# kill the third-party frontend: supervision brings it back in a new generation
kill -9 "$(pgrep -x navi-plugin-tui)"
navi components status      # gen 2, restarts 1, "supervision: … restarted (generation 2)"
```
