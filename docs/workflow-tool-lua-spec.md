# Spec: `workflow` Tool — Lua Multi-Agent Orchestration

| Field | Value |
|-------|--------|
| **Status** | Draft for implementation |
| **ADR** | [0015 — Workflow Script Engine: Lua](adr/0015-workflow-script-engine-lua.md) |
| **Evidence** | [Workflow Script Engine Benchmarks](workflow-script-engine-benchmarks.md) |
| **Owner crate** | `navi-core` (tool + runtime); surfaces: SDK / NAPI / TUI / CLI as listed below |
| **Goal** | A built-in **`workflow` tool** that runs a **sandboxed Lua 5.4 script** authored by the model to **orchestrate large numbers of subagents**, with **hard caps**, **per-agent permission control**, and **no in-script filesystem/shell**. |

---

## 1. Objective (definition of done)

The implementation is **done** when all of the following are true:

1. A model can call tool **`workflow`** with a Lua script (and optional `args` / run policy) and get a structured result.
2. The script can fan out **many** subagents via `agent` / `parallel` / `pipeline` without the parent conversation holding intermediate tool noise.
3. Default concurrency is **16**, default max agents per run is **1000**, both **configurable** and **hard-clamped**.
4. Every subagent runs under an **explicit, non-widening permission policy** (tools, paths, create files/dirs, optional writes).
5. The Lua VM **cannot** escape to host IO; only host builtins work.
6. Nested orchestration tools (`subagent`, `workflow`) are **denied inside workers**.
7. Progress is observable (events / task registry); runs are **cancellable**.
8. Journals and run state live under **`{data_dir}/workflows/`**, never auto-created under the project tree.
9. Automated tests cover the **acceptance matrix** in §11; surfaces in §10 are wired if the feature is user-visible.

**Out of scope for v1 (explicit):**

- QuickJS / Starlark / Rhai backends  
- Nested `workflow()` calls (depth ≥ 1) — optional later  
- Resume-from-journal after process restart (nice-to-have; design for journal format, implement if cheap)  
- Worktree isolation per agent  
- Reintroducing tool-batch Starlark `tool_workflow`

---

## 2. Glossary

| Term | Meaning |
|------|---------|
| **Script** | Lua 5.4 source executed in the workflow VM |
| **Host API** | Builtins injected into the VM (`agent`, `parallel`, …) |
| **Worker** | A subagent spawned by `agent()` |
| **Run** | One invocation of the `workflow` tool |
| **Run policy** | Default permission envelope for the run |
| **Agent policy** | Per-`agent()` override, **intersected** with run policy (never widens) |
| **Journal** | Append-only record of agent starts/results for the run |

---

## 3. Tool surface

### 3.1 Tool identity

| Field | Requirement | Verify |
|-------|-------------|--------|
| Name | `workflow` | Tool registry / schema |
| Kind | `Command` or `Custom` (not a free shell); **Write** risk if workers may write; default run is read-oriented | Metadata |
| Exposure | `Direct` or `Deferred` (product choice); if Deferred, discoverable via `tool_search` | Metadata + prompt |
| Implementation language of scripts | Lua 5.4 via `mlua` only (ADR 0015) | Cargo dep + tests |

### 3.2 Tool input schema

**Required:**

| Field | Type | Rules |
|-------|------|--------|
| `script` | string | Non-empty Lua source. Max size **≤ 64 KiB** (configurable constant; document value). |

**Optional:**

| Field | Type | Default | Rules |
|-------|------|---------|--------|
| `args` | object | `{}` | JSON object → Lua table `args` (read-only in script) |
| `max_parallel` | integer | settings / **16** | Clamped to `[1, MAX_PARALLEL_CEILING]` (ceiling **64** unless settings say lower) |
| `max_agents` | integer | settings / **1000** | Clamped to `[1, MAX_AGENTS_CEILING]` (ceiling **5000**) |
| `policy` | object | see §5 | Run-level default agent policy |
| `timeout_ms` | integer | settings / e.g. **30 min** | Wall clock for entire run; abort on exceed |
| `name` | string | — | Optional label for UI / journal |
| `resume_from_run_id` | string | — | **v1 optional**; if unimplemented, reject with clear error |

**Forbidden input:**

- Arbitrary host paths that load scripts from outside an allowlisted user/data location without explicit design (v1: **inline `script` only** is enough; if `script_path` is added later, must be project-relative + policy-checked).

### 3.3 Tool output (success)

Structured JSON (example shape; field names stable):

```json
{
  "ok": true,
  "run_id": "wf_…",
  "status": "completed",
  "result": { },
  "stats": {
    "agents_started": 0,
    "agents_completed": 0,
    "agents_failed": 0,
    "agents_cached": 0,
    "max_parallel_used": 0,
    "phases": ["A", "B"],
    "elapsed_ms": 0,
    "tokens_estimate": null
  },
  "journal_path": "…relative or data-dir path for host…",
  "error": null
}
```

| Field | Rules |
|-------|--------|
| `result` | Value returned by the Lua entrypoint (JSON-serializable). `null` if script returns nothing. |
| `ok` | `true` iff status is `completed` and script did not throw / host did not hard-fail |
| `status` | `completed` \| `failed` \| `cancelled` \| `timed_out` |

### 3.4 Tool output (failure)

- VM error, host error, cap exceeded, cancel, timeout → `ok: false`, `status` set, `error: { code, message, hint? }`.
- Partial stats still reported when possible.

**Error codes (minimum set):**

| Code | When |
|------|------|
| `script_too_large` | Over max bytes |
| `script_parse_error` | Lua syntax error |
| `script_runtime_error` | Uncaught Lua error |
| `sandbox_violation` | Forbidden global / library use |
| `invalid_host_call` | Bad args to builtin (e.g. `parallel` arity) |
| `agent_cap_exceeded` | `max_agents` hit |
| `budget_exceeded` | Optional token budget |
| `timeout` | Run wall clock |
| `cancelled` | User/parent cancel |
| `policy_denied` | Agent policy rejects spawn or tool |
| `not_implemented` | e.g. resume when not shipped |

---

## 4. Lua runtime contract

### 4.1 Entrypoint

**MUST** support at least one of (document which is primary):

1. Global function `workflow()` that returns one value, **or**
2. Script body as chunk that returns one value (last expression / `return`).

**Acceptance:** fixture scripts from benchmarks that define `function workflow() … end` run when host calls `workflow()`.

### 4.2 Host builtins (complete for v1)

| Builtin | Signature (Lua) | Semantics | Errors |
|---------|-----------------|-----------|--------|
| `agent` | `agent(prompt: string, opts?: table) -> any` | Spawns worker; **blocks** until completion (from script POV); returns structured table | Invalid prompt; policy; caps |
| `parallel` | `parallel(thunks: { function... }) -> { any... }` | Runs thunks concurrently up to `max_parallel`; each thunk is **zero-arg** and should call `agent` (or pure work); returns array of results **in input order** | Non-table; non-function element; nested unbounded spawn still counts toward caps |
| `pipeline` | `pipeline(items: table, fn: function) -> { any... }` | For each item (ipairs), `fn(item)`; host **may** parallelize with same semaphore; order-preserving results | `fn` not function |
| `phase` | `phase(title: string)` | Records phase; emits progress | — |
| `log` | `log(message: string)` | Records log line; emits progress | — |
| `args` | global table | Read-only copy of tool `args` | Writes ignored or error (prefer **error** on newindex) |
| `budget` | global table | Fields `total`, `spent`, `remaining` (numbers; `remaining` may be huge if unlimited) | Read-only |

**MUST NOT** expose: `subagent`, raw tools (`read_file`, `bash`, …), `require`, filesystem, network.

### 4.3 `agent` options (`opts` table)

All optional; missing → run policy defaults.

| Key | Type | Meaning |
|-----|------|---------|
| `profile` | string | Maps to existing `AgentProfile`: `planner`, `explorer`, `implementer`, `reviewer`, `security_reviewer`, `verifier`, `summarizer` |
| `model` | string | Optional model override for worker |
| `tools` | `{string...}` | Allowlist of tool names (intersected with profile + run policy) |
| `approval` | string | `inherit` \| `escalate` \| `read_only` \| `deny_write` |
| `path_allow` | `{string...}` | Glob/path prefixes worker may touch (read/write subject to other flags) |
| `path_deny` | `{string...}` | Always denied (wins over allow) |
| `create_files` | bool | Default **false** |
| `create_dirs` | bool | Default **false** |
| `write_allow` | `{string...}` | Explicit paths/globs allowed for edit/write; empty ⇒ no writes even if profile is implementer |
| `max_tokens` | integer | Optional completion budget for worker |
| `label` | string | UI / journal label |
| `schema` | table | Optional JSON-schema-like hint for structured return (best-effort validation) |

**Intersection rule (normative):**

```text
effective_tools = opts.tools ∩ run.policy.tools ∩ profile_allowlist
effective_paths = opts.path_allow ∩ run.policy.path_allow  (− path_deny)
effective_create_files = opts.create_files AND run.policy.create_files
effective_write_allow = opts.write_allow ∩ run.policy.write_allow
```

A worker **MUST NOT** receive a permission the run policy does not grant.

### 4.4 Sandbox (hard requirements)

| # | Requirement | Test idea |
|---|-------------|-----------|
| S1 | No `require`, `dofile`, `loadfile`, `load` (or `load` only if disabled) | Script calling them errors `sandbox_violation` |
| S2 | No `io`, `os`, `debug` libraries (or empty stubs that error) | Same |
| S3 | No `math.random` / time (`os.time`, etc.) | Same |
| S4 | No setting arbitrary globals that open host capabilities | Metatable lockdown |
| S5 | Instruction limit and/or wall timeout for pure Lua loops | `while true do end` → fail, not hang forever |
| S6 | Memory limit on VM | Document limit; stress test optional |
| S7 | `agent` return values are Lua tables/primitives, **not** requiring in-script `JSON.parse` | Fixture without JSON lib |

### 4.5 Determinism (v1)

- Script-side RNG/time forbidden (S3).
- Host **should** assign stable agent indices for journal keys.
- Full resume-after-edit is **optional v1**; if not shipped, still write journal for debugging.

---

## 5. Run policy defaults

### 5.1 Default run policy (when `policy` omitted)

| Field | Default |
|-------|---------|
| `profile` | `explorer` |
| `approval` | `read_only` |
| `tools` | read-oriented set: at least `read_file`, `search` (and hidden aliases as today if registered) — **no** `write_file`, `edit`, `bash` unless explicitly allowed |
| `path_allow` | project root only (same as `restrict_paths_to_project`) |
| `path_deny` | `.git/**`, NAVI private storage paths, secrets patterns as security layer already does |
| `create_files` | `false` |
| `create_dirs` | `false` |
| `write_allow` | `[]` |

### 5.2 Settings keys (suggested)

```toml
[workflow]
enabled = true
max_parallel = 16
max_agents = 1000
max_script_bytes = 65536
run_timeout_ms = 1800000
# opt-in product flags (if implemented)
require_opt_in = false
```

| Rule | Requirement |
|------|-------------|
| Project config | Must not silently enable dangerous workflow power if product policy mirrors plugins/MCP (document: global vs project). **Minimum:** caps and enabled flag documented. |
| Ceiling | Settings cannot raise above compile-time ceilings without explicit constant change |

---

## 6. Worker execution & permissions

### 6.1 Spawning

Each `agent()`:

1. Increments run agent counter; if `> max_agents` → `agent_cap_exceeded`.
2. Acquires semaphore slot (`max_parallel`).
3. Builds `SecurityPolicy` / tool allowlist from §4.3 intersection.
4. Strips **always**: `subagent`, `workflow`, and any other orchestration aliases (`NESTED_AGENT_TOOLS` extended).
5. Runs turn via existing subagent infrastructure (reuse `SubagentTool` / turn loop).
6. Converts result to Lua value; releases semaphore.

### 6.2 Permission matrix (acceptance)

| Scenario | Expected |
|----------|----------|
| Default run + default agent | Cannot `write_file` / `edit` / `bash` |
| `profile = implementer` but `write_allow = {}` | Still **no** writes |
| `write_allow = {"src/a.rs"}`, edit other path | Denied |
| `create_files = false`, tool creates new path | Denied |
| `path_deny` overlaps allow | Deny wins |
| Worker calls `subagent` / `workflow` | Denied (tool absent or policy deny) |
| `approval = escalate` | Approvals route to parent session (if infrastructure exists); else document fallback |

### 6.3 Bash / commands

- Default: **no bash** in workers.
- If run policy allows `bash`, existing `blocked_commands` / `guarded_commands` / permission mode still apply **and** path policy should constrain cwd/redirection as far as current security allows.
- Spec **does not** require perfect shell path sandbox beyond current `SecurityPolicy`; it **does** require default-off.

### 6.4 Replacing hard-coded subagent bg cap

Today `MAX_BACKGROUND_SUBAGENTS = 8` in `subagent.rs`. Workflow runs **MUST** use the **workflow** semaphore (`max_parallel`, default 16), not be stuck at 8.

| Requirement | Detail |
|-------------|--------|
| W-PAR | Workflow-internal agents honor `workflow.max_parallel` |
| W-ISO | Standalone `subagent` tool may keep its own cap; document both |

---

## 7. Lifecycle, cancel, progress

### 7.1 Lifecycle

```text
validate input → create run_id → open journal →
  load Lua sandbox → inject builtins → run entrypoint →
  serialize result → close journal → return ToolResult
```

### 7.2 Cancel

- Parent cancel / tool cancel **MUST** abort in-flight workers (best-effort) and set `status=cancelled`.
- Lua side sees error or host abort (document behavior).

### 7.3 Progress events (minimum)

Emit structured progress usable by TUI/SDK (names illustrative):

| Event | Payload (min) |
|-------|----------------|
| `workflow.started` | `run_id`, `max_parallel`, `max_agents` |
| `workflow.phase` | `title` |
| `workflow.log` | `message` |
| `workflow.agent_started` | `agent_index`, `label?` |
| `workflow.agent_completed` | `agent_index`, `ok` |
| `workflow.completed` / `failed` / `cancelled` | stats |

If full `RuntimeEvent` enum expansion is deferred, **at least** tool progress streaming / task registry entry of type `local_workflow` must expose the same data to the TUI.

### 7.4 Storage

| Path | Content |
|------|---------|
| `{data_dir}/workflows/{run_id}/journal.jsonl` | start/result lines |
| `{data_dir}/workflows/{run_id}/meta.json` | script hash, args hash, stats, status |
| **Not** under project `.navi/` for agent bookkeeping | AGENTS.md “No Workspace Metadata” |

---

## 8. Concurrency & caps (normative numbers)

| Parameter | Default | Ceiling | Behavior on exceed |
|-----------|---------|---------|-------------------|
| `max_parallel` | 16 | 64 | Clamp input; semaphore |
| `max_agents` | 1000 | 5000 | Fail next `agent()` with `agent_cap_exceeded` |
| Script size | 64 KiB | — | `script_too_large` |
| Run timeout | 30 min (suggested) | — | `timeout` |

**Acceptance:** A script that schedules 40 agents with `max_parallel=16` never has more than 16 workers in `running` at once (instrument test). A script that calls `agent` 1001 times with default max fails on the 1001st.

---

## 9. Security summary checklist

- [ ] VM cannot read/write host FS except via workers  
- [ ] Workers cannot widen policy beyond run  
- [ ] Workers cannot spawn `subagent`/`workflow`  
- [ ] Default run is read-only  
- [ ] Writes require explicit `write_allow` (+ create flags if creating)  
- [ ] Caps enforced  
- [ ] Cancel works  
- [ ] No project-tree journal pollution  
- [ ] Secrets redaction still applies to persisted journals if they store prompts (prefer redaction on)

---

## 10. Product surfaces (when feature ships)

Per AGENTS.md “Keep All Surfaces In Sync”:

| Surface | v1 requirement |
|---------|----------------|
| `navi-core` | Tool + runtime + tests |
| `navi-sdk` | Events and/or run status if exposed beyond tool result |
| `navi-napi` | Bindings if Tutor needs progress/cancel outside tool stream |
| `navi-tui` | Compact tool summary; optional progress UI; settings for caps |
| CLI | Optional: `navi workflow` list/show later; not blocking core tool |

**Minimum for “tool works in TUI session”:** core + tool registration + TUI summary. SDK/NAPI can follow in the same PR if events are new public types.

---

## 11. Acceptance matrix (verification)

Each row is a **requirement ID**. Implementation is complete when every **Must** row passes.

### 11.1 Tool & schema

| ID | Priority | Requirement | Verification |
|----|----------|-------------|--------------|
| T1 | Must | Tool registered as `workflow` | Unit: registry contains tool |
| T2 | Must | Input requires `script` | Unit: missing script errors |
| T3 | Must | Reject oversize script | Unit: `script_too_large` |
| T4 | Must | Accept `args` object into Lua `args` | Unit: script reads `args.x` |
| T5 | Must | Success returns `run_id`, `status`, `result`, `stats` | Unit |

### 11.2 Lua sandbox

| ID | Priority | Requirement | Verification |
|----|----------|-------------|--------------|
| L1 | Must | Valid `workflow()` script returns value as `result` | Unit |
| L2 | Must | Syntax error → `script_parse_error` | Unit |
| L3 | Must | `require('io')` / `io.open` → sandbox error | Unit |
| L4 | Must | Infinite loop hits instruction or timeout | Unit (bounded time) |
| L5 | Must | No `JSON` required for agent returns | Unit |

### 11.3 Host API

| ID | Priority | Requirement | Verification |
|----|----------|-------------|--------------|
| H1 | Must | `agent` invokes one subagent and returns structured table | Integration w/ mock provider |
| H2 | Must | `pipeline` invokes `fn` once per ipairs item | Unit/mock |
| H3 | Must | `parallel` accepts **only** array of zero-arg functions | Unit: two-arg form errors `invalid_host_call` |
| H4 | Must | `parallel` results order matches thunk order | Unit |
| H5 | Must | `phase` / `log` emit progress or journal lines | Unit |
| H6 | Must | `args` is read-only (write errors or no-ops documented) | Unit |
| H7 | Must | Unknown builtin not present | Unit |

### 11.4 Caps

| ID | Priority | Requirement | Verification |
|----|----------|-------------|--------------|
| C1 | Must | Default max_parallel effective 16 | Config + semaphore test |
| C2 | Must | Never exceeds max_parallel concurrent workers | Integration with barrier mock |
| C3 | Must | max_agents default 1000 enforced | Unit with mock agent |
| C4 | Must | Tool input clamps parallel/agents to ceilings | Unit |
| C5 | Must | Workflow not limited by standalone subagent bg cap of 8 | Integration: 12 concurrent workflow agents OK when max_parallel=16 |

### 11.5 Permissions

| ID | Priority | Requirement | Verification |
|----|----------|-------------|--------------|
| P1 | Must | Default worker cannot write | Integration: edit/write denied |
| P2 | Must | `write_allow` single file only that file | Integration |
| P3 | Must | `create_files=false` blocks new file create | Integration |
| P4 | Must | path_deny wins | Integration |
| P5 | Must | Worker cannot call `subagent` or `workflow` | Integration |
| P6 | Must | Agent opts cannot add tools outside run allowlist | Unit intersection |
| P7 | Must | implementer profile + empty write_allow ⇒ no writes | Integration |
| P8 | Should | `approval=escalate` surfaces to parent | Integration if approval bus available |

### 11.6 Lifecycle

| ID | Priority | Requirement | Verification |
|----|----------|-------------|--------------|
| R1 | Must | `run_id` unique per invocation | Unit |
| R2 | Must | Journal written under `{data_dir}/workflows/{run_id}/` | Unit with temp data_dir |
| R3 | Must | Cancel mid-run → `cancelled`, workers stopped | Integration |
| R4 | Must | Timeout → `timed_out` | Unit with short timeout |
| R5 | Should | Progress events or task snapshot queryable | Integration |
| R6 | Must | No journal under project `.navi/` by default | Unit |

### 11.7 Regression / fixtures

| ID | Priority | Requirement | Verification |
|----|----------|-------------|--------------|
| F1 | Must | Golden Lua fixture: enumerate→map→return (mock agents) | File under `tests/fixtures/workflow/` |
| F2 | Must | Golden Lua: parallel thunks + write_allow pattern (GLM-style) | Fixture |
| F3 | Must | Negative fixture: `parallel(items, fn)` rejected | Fixture |
| F4 | Should | Ported score patterns from Kimi-k2.7 honesty (`fixed` before ok) as **host guidance** in tool description | Doc + description snapshot test |

### 11.8 Surfaces

| ID | Priority | Requirement | Verification |
|----|----------|-------------|--------------|
| U1 | Must | TUI shows compact success/error for `workflow` | Manual or TUI unit if pattern exists |
| U2 | Should | Settings expose max_parallel / max_agents | Config parse test |
| U3 | Should | SDK event variants if new public events added | Compile + napi if required by policy |

---

## 12. Tool description requirements (for the model)

The tool’s model-facing description **MUST** include:

1. Lua entrypoint example (`function workflow() … end`).
2. Host builtins list and `parallel` thunk shape.
3. Default read-only policy; how to grant `write_allow`.
4. Caps (16 / 1000 defaults).
5. Explicit: do not use `require`, `io`, `os`, `JSON.parse`.
6. Explicit: workers do the IO; script only orchestrates.

**Acceptance:** snapshot or string contains these bullets (unit test on description text).

---

## 13. Non-functional

| ID | Requirement |
|----|-------------|
| NF1 | No second Tokio runtime inside the tool |
| NF2 | Respect cancel tokens from turn context |
| NF3 | Avoid holding large worker transcripts in parent tool output; summarize in `result` + journal |
| NF4 | Truncate oversized agent returns before injecting into Lua (document max bytes, e.g. 256 KiB per agent result) |

---

## 14. Phased delivery (optional planning aid)

| Phase | Scope | Exit criteria |
|-------|--------|----------------|
| **P0** | Sandbox + builtins with **mock** agent backend + caps + policy intersection unit tests | §11 L*, H*, C*, P* unit-level green |
| **P1** | Real subagent integration + journal + cancel | §11 R*, P* integration green |
| **P2** | TUI summary + settings + description polish | U1–U2, §12 |
| **P3** | SDK/NAPI events, resume, nested workflow | Optional |

P0+P1 required for “objective met” in engine terms; P2 for interactive product completeness.

---

## 15. Traceability

| Goal phrase | Spec sections |
|-------------|---------------|
| Tool workflow via Lua script | §3, §4, T*, L* |
| Massive subagent orchestration | §6, §8, C*, H* |
| Maximum permission control | §4.3, §5, §6.2, P* |
| Safe by default | §5.1, §9, S*, P1 |
| Observable / cancellable | §7, R* |
| ADR 0015 compliance | §1, §4, engine Lua only |

---

## 16. Sign-off

Implementation **meets the final objective** when:

1. All **Must** rows in §11 pass in CI (`cargo test -p navi-core …` focused).  
2. Manual smoke: session runs a small Lua workflow (2–3 agents) under default policy and a second run with `write_allow` on one file.  
3. ADR 0015 is not violated (no QuickJS default; no project-tree state).  

| Role | Name | Date |
|------|------|------|
| Spec author | — | — |
| Implementer | — | — |
| Reviewer | — | — |
