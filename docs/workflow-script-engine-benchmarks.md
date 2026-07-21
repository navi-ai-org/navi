# Workflow Script Engine Benchmarks (JS vs Lua)

**Related decision:** [ADR 0015 — Workflow Script Engine: Lua (`mlua`)](adr/0015-workflow-script-engine-lua.md)

**Status of evidence:** manual model bake-off (static script review against a fixed host-API contract). Not a timed runtime harness; no live subagents were executed. Scores judge **whether a model can author a correct orchestration script** in each language.

**Date of bake-off:** 2026-07 (session research leading to ADR 0015)

---

## 1. Goal

Choose the default **orchestration script language** for NAVI’s future `workflow` tool:

- Script coordinates **subagents only** (not raw tools).
- Host builtins: `agent`, `parallel`, `pipeline`, `phase`, `log`, `args`, `budget`.
- Caps (product intent): default **16** parallel, **1000** agents/run.
- Per-agent policy: tools allowlist, path allow, `create_files` / `create_dirs`, optional `write_allow`.

Candidates narrowed to **QuickJS-style JavaScript** vs **Lua 5.4** after discarding Starlark (removed historically) and deferring Rhai.

---

## 2. Method

### 2.1 Task (identical for every model)

Each model received the same prompt asking for **two** complete scripts (JS and Lua) implementing:

| Phase | Behavior |
|-------|----------|
| A — enumerate | One explorer agent → `{ files: string[] }`; early exit if empty |
| B — audit | `pipeline` over files; missing-auth findings |
| C — verify | Fan-out security_reviewer over flattened findings |
| D — fix (optional) | If `args.fix`, implementers only for **verified ∧ high**, `write_allow: [file]`, no create |
| E — synthesize | Structured return: `ok`, `summary`, `files_scanned`, `findings[]`, `unverified_count` |

Defaults: `routes_glob`, `path_allow`, `fix=false`. Determinism: no time/RNG. No FS/network/shell in-script.

### 2.2 Scoring rubric (0–2 per criterion, max 20)

| # | Criterion |
|---|-----------|
| 1 | Host API only (no inventing `fs` / `JSON.parse` / `require`) |
| 2 | Phases A–E + empty early exit |
| 3 | Policy (tools / path / create flags / write_allow on fix) |
| 4 | Fan-out B/C + parallel D shape |
| 5 | Fix gate (fix ∧ high ∧ verified) |
| 6 | Output contract |
| 7 | Coherent `ok` rule (esp. after fix) |
| 8 | Robustness to null / malformed agent payloads |
| 9 | Language correctness (e.g. no `await` in Lua; `parallel(thunks)` arity) |
| 10 | Prompt quality / operational design |

### 2.3 Cohorts

1. **Small / flash / older** — capability ceiling lower; stress “does Lua collapse?”
2. **Daily drivers** — models used in real NAVI work (user-reported).

---

## 3. Results

### 3.1 Small / flash / older cohort

| Model | JS | Lua | Internal winner | Notes |
|-------|---:|----:|-----------------|-------|
| Hy3 | 19 | 19 | Tie | Best of smalls; strong prompts + robustness |
| DeepSeek-v4-flash | 17 | 18 | **Lua** | JS fragile destructuring / missing null guards |
| Laguna-xs-2.1 | 16 | 16 | Tie | Critical: audit agent prompt **omitted file path** in both langs |
| Kimi-k2.5 | 17 | 16 | **JS** (thin) | Filtered to verified-only → `unverified_count` broken |
| gpt-oss-120b | 14 | 16 | **Lua** | JS async parallel quirks; verify used `tools: []` in both |
| MiniMax-M2.7 | 13 | 13 | Tie (different blockers) | Invented `JSON.parse`; JS lost `file` on flatten; Lua wrong `parallel` arity |

**Small-cohort pattern:** Lua **did not systematically lose**. Wins: DeepSeek-flash, gpt-oss. Ties common. JS-only wins were rare/thin.

### 3.2 Daily-driver cohort

| Model | JS | Lua | Internal winner | Notes |
|-------|---:|----:|-----------------|-------|
| Kimi-k2.7-code | **20** | **20** | Tie | Best honesty: `ok` needs remaining high=0; drop findings only if `fixed === true` |
| GLM-5.2 | **20** | **20** | Tie | Best fix ops: **dedupe implementer per file**; narrow `path_allow`/`write_allow` to that file |
| Grok-4.5 | 19 | 19 | Tie | Best degradation (severity normalize, verify fallbacks); fix-mode `ok` slightly optimistic |
| DeepSeek-v4-pro | 18 | 18 | Tie | Best domain prompts (Express/Nest/…); fix-mode `ok = true` always — bad |

**Daily-cohort pattern:** **Four ties at 18–20/20.** Language was not the differentiator; `ok`/fix honesty, validation, and write-dedupe were.

### 3.3 Combined ranking (best entry per model, either language)

| Rank | Entry | Score |
|------|-------|------:|
| 1 | Kimi-k2.7 JS/Lua | 20 |
| 1 | GLM-5.2 JS/Lua | 20 |
| 3 | Grok-4.5 JS/Lua | 19 |
| 3 | Hy3 JS/Lua | 19 |
| 5 | DeepSeek-v4-pro JS/Lua | 18 |
| 5 | DeepSeek-v4-flash Lua | 18 |
| 7 | DeepSeek-v4-flash JS | 17 |
| 7 | Kimi-k2.5 JS | 17 |
| 9 | Laguna JS/Lua | 16 |
| 9 | gpt-oss Lua / Kimi-k2.5 Lua | 16 |
| 11 | gpt-oss JS | 14 |
| 12 | MiniMax JS/Lua | 13 |

### 3.4 Head-to-head (same model, which language scored higher?)

| Outcome | Count (approx.) | Models |
|---------|----------------:|--------|
| Lua higher | 2 | DeepSeek-flash, gpt-oss-120b |
| JS higher | 1 | Kimi-k2.5 (1 point) |
| Tie | 7+ | Hy3, Laguna, MiniMax, Kimi-2.7, GLM, Grok, DS-pro |

---

## 4. Failure modes (cross-cutting)

These showed up **independent of language** more often than “Lua 1-based bugs”:

| Failure | Examples | Host mitigation |
|---------|----------|-----------------|
| Optimistic `ok` after fix without re-verify | DS-pro, Grok (partial), Hy3 variants | Require `fixed` flag or re-audit; separate `fixes_submitted` |
| Dropping unverified from report | Kimi-k2.5 | Schema: findings must include `verified: false` rows |
| Audit prompt missing target path | Laguna | Lint prompts / inject `item` in `pipeline` |
| Invented `JSON.parse` / string results | MiniMax | Agent returns tables; reject unknown globals at load |
| Wrong `parallel` signature | MiniMax Lua | Strict arity: only array of zero-arg functions |
| Blind reviewer (`tools: []`) | gpt-oss | Profile defaults inject min tools for security_reviewer |
| Erase highs from report after fix | gpt-oss | Never drop findings without explicit re-verify |

**Lua-specific traps** (1-based, no `await`) **rarely** caused the worst scores. Models that failed hard usually **violated the host contract** in both languages or in JS-only ways (fragile `flatMap`, async thunk ambiguity).

---

## 5. Why this favors Lua over QuickJS for NAVI

### 5.1 What the bake-off showed

1. **Daily models write Lua as well as JS** for this orchestration shape (all ties at high scores).
2. **Small models did not collapse on Lua**; several preferred or matched Lua.
3. Quality gaps were **orchestration semantics** (policy, `ok`, file in prompt), not missing JS sugar.
4. Therefore QuickJS is **not required** for “models can author workflows” on NAVI’s provider mix.

### 5.2 Engineering reasons (beyond scores)

| Factor | Lua (`mlua`) | QuickJS (`rquickjs`) |
|--------|--------------|----------------------|
| Language surface to sandbox | Small | Larger (eval, prototypes, timers, Promise edge cases) |
| Async model | Sync-looking calls + host wait | Natural `await`, more interleaving footguns |
| Aligns with ADR 0006 mandatory limits | Easier to reason about | Longer hardening checklist |
| Drop-in reuse of external JS workflow snippets | No | Yes |
| Measured authoring win on daily models | Equal | Equal |
| Historical NAVI Starlark lesson | Prefer tight host API over heavy deps | — |

### 5.3 What we are *not* claiming

- That Lua is better than JS for **all** coding tasks.
- That arbitrary public JS workflow snippets will run unmodified.
- That a future QuickJS backend is forbidden (ADR 0015 keeps host API portable).
- That runtime performance was benchmarked (it was not).

---

## 6. Fixtures recommended for implementation CI

Use these as **golden authoring samples** (mock host, no network):

| Priority | Source | Why |
|----------|--------|-----|
| P0 | Kimi-k2.7 Lua | Honesty on `ok` / `fixed` |
| P0 | GLM-5.2 Lua | Per-file fix dedupe + narrow path_allow |
| P1 | Grok-4.5 Lua | Malformed payload degradation |
| P1 | Hy3 Lua | Strong small-model baseline |
| P2 | DeepSeek-v4-pro Lua | Domain-rich audit prompts (fix `ok` rule before using as fixture) |

Negative tests (must fail load or validation):

- Script references `JSON.parse` / `require` / `io.open`
- `parallel(items, fn)` two-arg form
- Worker policy with `create_files: true` without run-level grant

---

## 7. Reproducing the bake-off

1. Use the dual-language prompt from the design session (host builtins + five-phase auth audit).
2. Run against each model with low temperature; no extra system context differing by model.
3. Score with §2.2; do not execute agents unless adding a separate **runtime** benchmark later.
4. Record model id, date, and scores in a new subsection here if re-run.

---

## 8. Conclusion

For NAVI’s workflow **orchestration** VM, bake-off evidence + sandbox/KISS priorities support:

> **Default engine = Lua 5.4 (`mlua`).**  
> QuickJS remains an optional future backend if JS-first authoring is later measured as a product requirement—not the default based on this data.

See [ADR 0015](adr/0015-workflow-script-engine-lua.md) for the binding decision, host API, sandbox rules, and follow-ups.
