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
