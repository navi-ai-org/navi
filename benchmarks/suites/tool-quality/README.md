# Tool-quality agent comparison suite

Evaluates **how well an agent uses tools** (search → edit → verify), not only
whether the final answer is correct. Same fixtures and verifier commands run
across multiple code agents under a shared model tier.

## Agents under test

| Agent | How it is driven | Model (default) |
|-------|------------------|-----------------|
| **navi** | `navi bench run` (native metrics) | `opencode` / `deepseek-v4-flash-free` (OpenCode Zen free) |
| **opencode** | `opencode run --auto --format json` | `opencode/deepseek-v4-flash-free` |
| **claude-code** | `claude -p` + **cc-proxy** | `deepseek/deepseek-v4-flash` via `http://localhost:19429` |
| **grok** | Manual/reference slot (optional) | — |

Baseline model intent: **DeepSeek V4 Flash free** (or Command Code
`deepseek/deepseek-v4-flash` when routing Claude Code through cc-proxy).

### Claude Code + cc-proxy

cc-proxy must already be running (Command Code local gateway). Then:

```bash
export CC_PROXY_URL=http://localhost:19429
export CC_PROXY_API_KEY=cc-proxy
# Claude SDK uses ANTHROPIC_BASE_URL; the harness sets this for you.
```

The harness maps Claude’s Anthropic client to the proxy so you can use
DeepSeek V4 Flash with Claude Code’s tool stack without Anthropic billing.

## Cases

| ID | Focus |
|----|--------|
| `tq-smoke-fix` | Minimal read → edit → test |
| `tq-tool-select-edit` | Grep/search to the right file, targeted edit |
| `tq-slug-routing` | Multi-file bug fix (medium) |
| `tq-symbol-ranking` | Ranking logic + verification loops |
| `tq-config-precedence` | Careful layered semantics (hard) |

## Run

```bash
# Full multi-agent comparison (needs navi, opencode, claude, cc-proxy)
just bench-tool-quality

# Navi only (smoke iteration)
just bench-tool-quality-smoke

# Custom
python3 benchmarks/scripts/run_agent_comparison.py \
  --agents navi,opencode,claude-code \
  --cases tq-smoke-fix,tq-tool-select-edit \
  --out benchmarks/runs/agent-compare/run.json
```

Outputs:

- `benchmarks/runs/agent-compare/latest.json` — full metrics
- `benchmarks/runs/agent-compare/latest.md` — score table

## Score (0–100)

```
score = 50% success_rate
      + 20% (1 - failed_tool_rate)
      + 15% tool efficiency (fewer tools on successes)
      + 15% speed (lower wall time on successes)
```

This rewards agents that pass verifiers **with disciplined tools**, not
maximum thrashing.

## Navi-only (native bench)

You can still run the suite through NAVI’s built-in runner only:

```bash
just bench benchmarks/suites/tool-quality \
  benchmarks/runs/tool-quality-navi.json \
  opencode deepseek-v4-flash-free
```
