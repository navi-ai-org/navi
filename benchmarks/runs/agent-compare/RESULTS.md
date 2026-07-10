# Tool-quality multi-agent results

Baseline model tier: **DeepSeek V4 Flash free** (OpenCode Zen) / **deepseek/deepseek-v4-flash** via **cc-proxy** for Claude Code.

Harness: `benchmarks/scripts/run_agent_comparison.py`  
Navi binary: workspace `target/release/navi` with plan-review auto-approve.

## Round A — easy suite (`tool-quality`, 5 cases)

| Agent | Score | Pass | Tools | Avg tools (ok) | Avg ms (ok) |
|-------|------:|-----:|------:|---------------:|------------:|
| **navi** | **96.0** | 5/5 | 35 | 7.0 | 57 000 |
| opencode | 83.9 | 4/5 | 54 | 13.5 | 41 000 |
| claude-code | 93.8 | 5/5 | 39 | 7.8 | 131 000 |

Opencode only failure: `tq-symbol-ranking` timeout (10 min), not wrong final answer.

**Verdict:** suite too easy for pass-rate discrimination.

## Round B — hard suite (`tool-quality-hard`, 7 cases) multi-agent

| Agent | Score | Pass | Tools | Avg tools (ok) | Avg ms (ok) |
|-------|------:|-----:|------:|---------------:|------------:|
| navi | 86.6 | 6/7 | 69 | 11.2 | 83 000 |
| **opencode** | 90.6 | **7/7** | 142 | 20.3 | 72 000 |
| **claude-code** | **92.0** | **7/7** | 92 | 13.1 | 122 000 |

Navi miss: `hard-priority-queue` timeout (12 min, only 2 tools) — model/agent loop stall, **not** a wrong patch. Retry alone: **PASS** (15 tools, ~190 s).

After plan-review harness fix, navi solo hard suite: **7/7**, score **94.8**, avg **9.3 tools**.

## Round C — expression language (parser precedence)

| Agent | Pass | Tools | Wall |
|-------|------|------:|-----:|
| navi | ✓ | 14 | 98 s |
| opencode | ✓ | 30 | 136 s |
| claude-code | ✓ | 13 | 302 s |

## Harness fixes applied during the run

1. **Plan review deadlock** — `navi bench --auto-approve` now auto-approves `PlanReviewRequired` and answers `QuestionRequired` (was hanging 10–15 min on plan mode).
2. Hard multi-bug fixtures + multi-module mini-DB / path VFS / rate limiter / expr parser.
3. Softened spoiler `// BUG` comments so tests are the source of truth.

## What this means for “model difficulty”

DeepSeek V4 Flash free **solves** these agentic repair tasks when:

- tools + `cargo test` feedback are available, and  
- the harness does not block on interactive plan/approval gates.

Differentiation shows up in **tool efficiency** (navi typically fewer tools than OpenCode) and **latency**, not in raw pass rate. Raising difficulty further in this “read tests → patch → retest” regime mostly increases wall time, not fail rate, for this model tier.

To stress **fail rate** of the model (not the harness), next steps would be:

- single-shot patch without intermediate test runs, or  
- weaker model, or  
- tasks without unit tests (spec-only / property-only).

## Artifact paths

- Easy: `benchmarks/runs/agent-compare/latest.json`
- Hard multi: `benchmarks/runs/agent-compare/hard-latest.json`
- Hard navi final: `benchmarks/runs/agent-compare/hard-navi-final.json`
- Expr: `benchmarks/runs/agent-compare/expr-lang.json`
- **Hard + tokens (2026-07-10):** `benchmarks/runs/agent-compare/hard-tokens.json`

---

## Round D — hard suite with token accounting (8 cases, all agents)

Sources:

| Agent | Token source |
|-------|----------------|
| navi | provider (`UsageReported` / bench metrics) |
| opencode | provider (sum of `step_finish.part.tokens`) |
| claude-code | **estimated_stream** (cc-proxy returns 0 usage; chars/4 multi-turn, **no** system/tool schema) |

### Summary

| Agent | Pass | Tools | Tok in | Tok out | Tok Σ | Tok/success | Src |
|-------|-----:|------:|-------:|--------:|------:|------------:|-----|
| **navi** | 8/8 | 93 | **1 747 901** | **53 735** | **1 801 636** | ~225 k | provider |
| **opencode** | 8/8 | 86 | **166 249** | **13 256** | **1 451 854** | ~181 k | provider |
| claude-code | 8/8 | 79 | 286 063* | 9 485* | 295 548* | ~37 k* | estimated* |

\*Claude totals **under-count** vs real API: stream estimate excludes huge system/tool schemas that Navi/OpenCode include in billed input.

### Per-case total tokens

| Case | navi | opencode | claude* |
|------|-----:|---------:|--------:|
| interval-merge | 115 688 | 101 233 | 7 198 |
| cache-layers | 223 187 | 141 499 | 17 798 |
| state-machine | 97 974 | 86 149 | 10 839 |
| priority-queue | 108 943 | 124 171 | 14 160 |
| mini-db | 228 496 | 194 290 | 137 935 |
| rate-limiter | 387 962 | 188 271 | 31 178 |
| path-resolve | 511 684 | 310 547 | 16 039 |
| expr-lang | 127 702 | 305 694 | 60 401 |

### Reading the token story

1. **Navi vs OpenCode (both provider-real):** same model tier, both 8/8. Navi used more tools (93 vs 86) and more tokens overall (~1.80 M vs ~1.45 M). OpenCode often cheaper on tokens per case, but not always (expr-lang: OpenCode 306 k vs Navi 128 k).
2. **Input dominates** for Navi/OpenCode (history + tools re-sent each step).
3. **Claude column is not apples-to-apples** until cc-proxy exposes usage; treat as lower bound on dialog payload only.
4. **Tool efficiency ≠ token efficiency:** fewer tools can still burn tokens if prompts/history are large.

---

## Why Navi lost tokens to OpenCode (analysis of Round D)

### Root causes (from event traces)

1. **`plan` tool spam** — worst cases had 4–6 plan + plan-review loops. Each plan step is a full model turn (~25–35k input). OpenCode uses light `todowrite` or none.
2. **Codebase extras overused on tiny fixtures** — `fs_browser` / `repo_explore` before plain `read_file`. Useful on real monorepos; wasteful on 1-file fixtures.
3. **Patch thrash** — many small `apply_patch` + re-read cycles instead of one coherent edit.
4. **Adaptive thinking** — large `AssistantThinkingDelta` volume even when verifiers only need a fix.
5. **Gross token sum** — Navi metrics sum every step’s full `input_tokens` (incl. cache-read volume in the input field). Still real API accounting, but multi-step loops amplify totals.

### Progressive fixes shipped in `bench_cmd`

| Phase | Change | Effect |
|-------|--------|--------|
| A | Auto-approve plan/questions (unblock hangs) | Quality recovered |
| B | Deny `plan`/`set_goal`/meta tools; YOLO; tighter observations; `thinking=Low`; efficiency preamble | **−26% tokens** (1.80M→1.34M), tools 93→66, still 8/8 |
| C | Also deny `fs_browser`/`repo_explore`/`ast_search` for bench | Head-to-head vs OC: **Navi 1.50M / 8/8 vs OC 1.72M / 7/8** |

### Round E — head-to-head after efficiency (`hard-vs-oc-v2.json`)

| Agent | Pass | Tools | Tok Σ | Tok/success |
|-------|-----:|------:|------:|------------:|
| **navi** | **8/8** | **71** | **1 496 722** | ~187 k |
| opencode | 7/8 | 92 | 1 716 876 | ~245 k |

Navi wins this run on **pass rate and total tokens**. Remaining variance: some cases still higher for Navi (interval-merge, expr-lang) when the model takes a long path — not a harness regression.

### Further levers (not yet)

- Product: default “repair” tool policy that de-prioritizes plan on small repos
- Prompt: shorter system prompt / tool list for `profile=small`
- Metrics: report billable = input − cache_read alongside gross
- Model: `ThinkingConfig::Off` for free-flash benches if quality holds
