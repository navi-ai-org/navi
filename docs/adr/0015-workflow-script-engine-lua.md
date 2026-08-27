# ADR 0015 — Workflow Script Engine: Lua (`mlua`), Not QuickJS

## Status

Accepted

## Context

NAVI needs a **workflow tool** for multi-agent orchestration:

- the **model authors a script** that coordinates work
- the script calls a **host-only API** (`agent`, `parallel`, `pipeline`, `phase`, `log`, `budget`, `args`)
- workers are **subagents** with per-agent policy (tools, path allow/deny, create files/dirs)
- the host enforces caps (default **16** concurrent, **1000** total agents per run, configurable)

This is **not** a revival of the removed Starlark `tool_workflow` tool, which batched **read-only tools** inside an interpreter. Workflow scripts must only **orchestrate subagents**; filesystem and shell stay inside workers under `SecurityPolicy`.

Candidate script engines for the orchestration VM:

| Engine | Binding | Notes |
|--------|---------|--------|
| **Lua 5.4** | `mlua` | Small language, mature embed story |
| **QuickJS** | `rquickjs` | JS subset; familiar `async/await` for models |
| Starlark | `starlark` | Previously used for `tool_workflow`; removed |
| Rhai | `rhai` | Rust-native; good sandbox, weaker model familiarity than JS/Lua |

Some external agent products implement dynamic workflows as **deterministic JavaScript** in a restricted VM. That pattern is useful as a reference for **host API shape** and lifecycle (journal, resume, caps), not a requirement to ship JS inside NAVI.

### Decision drivers

1. **Author is the model** — success rate of *generated* scripts matters as much as human DX.
2. **NAVI is multi-provider** — daily models include DeepSeek, Kimi, Grok, GLM, and others.
3. **Security defaults are mandatory** (ADR 0006) — smaller language surface is easier to lock down.
4. **KISS / Rust embed** — one clear runtime, no Node, no full browser JS semantics.
5. **Portable host API** — if the engine is wrong later, the *API* (`agent` / `parallel` / `pipeline`) should still be swappable.

### Evidence: model script bake-off

We ran a fixed dual-language task (auth-route audit workflow: enumerate → audit fan-out → adversarial verify → optional fix → synthesize) asking each model for **both** a JS (QuickJS-like) and a Lua (5.4-like) script against the **same** host builtins. Results are recorded in full in [Workflow Script Engine Benchmarks](../workflow-script-engine-benchmarks.md).

Headline:

| Cohort | Models | JS vs Lua (same model) |
|--------|--------|-------------------------|
| Small / flash | Laguna, Hy3, DeepSeek-flash, MiniMax, gpt-oss-120b, Kimi-k2.5 | Lua **won or tied** more often than JS; worst bugs were **host-contract** errors, not “Lua syntax” |
| Daily drivers | DeepSeek-v4-pro, Kimi-k2.7, Grok-4.5, GLM-5.2 | **All tied** JS/Lua at high quality (18–20/20) |

Top scripts (Kimi-k2.7, GLM-5.2, Hy3, Grok) were **language-agnostic** in quality. Failures that hurt most (missing `file` in prompts, inventing `JSON.parse`, wrong `parallel` arity, optimistic `ok` after fix) appeared in **both** languages depending on the model—not systematically in Lua.

Therefore “models only write good JS” is **not** supported for NAVI’s actual model mix. QuickJS’s remaining advantages are **JS ecosystem familiarity** and natural `async/await`, not measured authoring quality on our stack.

## Decision

**Ship the workflow orchestration script engine as Lua 5.4 via `mlua`.**

### Host API (language-agnostic; Lua is the first implementation)

Scripts may only call:

| Builtin | Role |
|---------|------|
| `agent(prompt, opts?)` | Spawn one subagent; returns structured result |
| `parallel(thunks)` | Run zero-arg thunks concurrently (host semaphore) |
| `pipeline(items, fn)` | Map items through `fn` (host may parallelize) |
| `phase(title)` | Progress boundary |
| `log(message)` | Progress log line |
| `args` | Read-only run inputs |
| `budget` | `{ total, spent, remaining }` (host-maintained) |

Optional later: `workflow(name_or_path, args)` with **nesting depth ≤ 1**.

### Hard sandbox rules

- No `require` / `dofile` / `load` / `loadfile` / `package` (or only a frozen empty package table).
- No `io.*`, `os.*` (except possibly a ban list that removes the whole library), no `debug.*`.
- No `math.random` / time sources (determinism for journal/resume).
- No raw filesystem, network, or shell from the script; **only** host builtins.
- Instruction/memory/time limits on the VM (mandatory, non-weakable by the script).
- `agent` results are **native tables/objects**, not opaque JSON strings (do not require a global `JSON` library in-script).

### Runtime defaults (configurable in settings; not project-enablable for “power” features if policy says so)

| Setting | Default | Notes |
|---------|---------|--------|
| `max_parallel` | 16 | Clamp to a safe max (e.g. 64) |
| `max_agents` | 1000 | Hard cap per run |
| Default worker policy | read-only explorer | tools + path allow; `create_files` / `create_dirs` false |
| Nested `subagent` / `workflow` inside workers | denied | Same spirit as stripping recursive orchestration tools |

### Explicit non-goals (this ADR)

- Reintroducing Starlark `tool_workflow` as a generic tool-batch language.
- Exposing arbitrary tools as globals inside the script VM.
- Matching third-party JS workflow script syntax 1:1.
- Running workflow state or journals under the project tree (use `{data_dir}/workflows/…` only).

## Alternatives considered

### QuickJS (`rquickjs`)

**Pros:** Models generate idiomatic `async/await`; large training prior on JS; easy to reuse generic public JS orchestration examples.

**Cons for NAVI:** Larger language/attack surface (eval/Function/prototype/timers if not stripped carefully); async interleaving complexity; C embed + ongoing hardening checklist. Bake-off did **not** show better scripts than Lua on daily models. **Rejected as default engine.**

May be revisited only if: (1) host API is stable, (2) a measured JS-preferring authoring workload fails on Lua, and (3) security review of a frozen QuickJS realm is staffed. The host API must stay engine-agnostic so a second backend is possible without rewriting product semantics.

### Starlark

**Pros:** Prior NAVI code; determinism culture; Python-like.

**Cons:** Removed once for cost/value; product job now is **agent orchestration**, not tool batching; reopening the dep without new evidence is churn. **Rejected.**

### Rhai

**Pros:** Pure Rust, easy host functions, good sandbox story.

**Cons:** Less training data than Lua/JS for models; bake-off was JS vs Lua only. **Deferred** as a possible third backend, not the default.

### Typed JSON-only workflow (no script)

**Pros:** Easiest to validate; no VM.

**Cons:** Loses mid-run branching/loops that motivated script mode. May still ship as an alternate input shape later; **not** a substitute for the accepted script decision.

## Consequences

### Positive

- Small, well-understood embed surface aligned with ADR 0006 mandatory limits.
- Documented bake-off shows **Lua is good enough (and often equal)** for NAVI’s daily models.
- Sync-looking `local r = agent(...)` maps cleanly to a host that `block_on`s / awaits subagents (same pattern as the old Starlark bridge).
- Host API can stay shared if QuickJS is ever added as an opt-in experimental backend.

### Negative

- Not syntax-compatible with common external JS workflow examples; authors (and models) need Lua examples in the tool description and docs.
- Arrays are 1-based; host↔JSON boundary must normalize sequential tables carefully.
- Async is host-mediated (no true JS `Promise` scheduler); `parallel` / `pipeline` must be the only concurrency primitives.

### Follow-ups (implementation; not part of this decision text)

1. `workflow` tool + `mlua` sandbox + journal under `{data_dir}/workflows/`.
2. Per-agent policy narrowing on top of existing `subagent` profiles.
3. Settings for `max_parallel` / `max_agents` / opt-in mode (keyword / session).
4. Eval fixtures from top bake-off scripts (Kimi-k2.7, GLM-5.2, Hy3) against a mock host.
5. SDK/TUI progress events for `local_workflow` tasks when the tool lands.

## References

- [Workflow Script Engine Benchmarks](../workflow-script-engine-benchmarks.md) — full scores, cohorts, failure modes
- [Workflow Tool Lua Spec](../workflow-tool-lua-spec.md) — implementable requirements and acceptance matrix
- ADR 0006 — Security defaults are mandatory
- ADR 0013 — WASM-only plugins (orthogonal; workflow VM is **not** a marketplace plugin)
- Removed `tool_workflow` (Starlark) — historical tool-batch approach; do not conflate with this ADR
