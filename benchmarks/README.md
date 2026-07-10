# NAVI agentic benchmarks

Benchmarks run NAVI as a real headless code agent inside an isolated fixture,
capture the runtime trajectory, then validate the final workspace with verifier
commands.

This is separate from `evals/`:

- `evals/` are cheap verifier-replay regression checks.
- `benchmarks/` measure agent behavior: success rate, tokens, tool calls, time,
  changed files, diff size, tool failures, and verifier outcomes.

Run a suite:

```bash
just bench
```

By default, `just bench` runs the full corpus under `benchmarks/suites` with
`opencode` + `deepseek-v4-flash-free`, so it is a real token-consuming agentic
benchmark. Pass a custom suite, output path, provider, and model when needed:

```bash
just bench benchmarks/suites benchmarks/runs/candidate.json opencode deepseek-v4-flash-free
```

For a cheap smoke-only check:

```bash
just bench-smoke
```

Open the local visual report and load one or more run JSON files:

```bash
just bench-report
```

`just bench` and `just bench-report` generate `benchmarks/runs/index.js`, so the
report automatically loads all JSON checkpoints currently in `benchmarks/runs/`.
The in-page JSON button is only for adding ad hoc files.

Initial corpus:

| Case | Difficulty | Focus |
|---|---:|---|
| `smoke-fix-rust-test` | easy | end-to-end runner smoke test |
| `medium-slug-routing` | medium | multi-file debugging and string normalization |
| `medium-high-symbol-ranking` | medium-high | search relevance, ranking, alternatives, deterministic ordering |
| `hard-config-precedence` | hard | layered runtime config precedence and careful semantics |

Compare against a baseline:

```bash
cargo run -p navi-cli -- bench compare benchmarks/runs/candidate.json \
  --baseline benchmarks/runs/baseline.json \
  --min-success-rate 1.0
```

Each case is a `.toml` or `.json` file:

```toml
version = 1
id = "stable-case-id"
title = "Human readable title"
category = "bug_fix"
fixture = "benchmarks/fixtures/my-fixture"
task = "Fix the failing test."
max_turns = 8
max_tool_calls = 60
timeout_ms = 600000

[agent]
mode = "parity"
profile = "medium"

[[verifiers]]
verifier_type = "command"
command = "cargo test"
required = true
```

Relative `fixture` paths are resolved from the `--project` root, defaulting to
the current directory. Fixtures are copied to a temp workspace before each case.

`max_turns` and `max_tool_calls` are optional. When omitted, the benchmark
runner does not impose those limits. When present, the event that exceeds the
limit is kept in the trajectory, the turn is cancelled, and the case fails with a
limit error.

Use `--keep-workspaces` to inspect a failed run's temporary workspace.

## Multi-agent tool-quality comparison

Compare **navi**, **OpenCode**, and **Claude Code** (via **cc-proxy** so Claude
Code can use DeepSeek V4 Flash) on the same fixtures. See
[`suites/tool-quality/README.md`](suites/tool-quality/README.md).

```bash
# Full comparison (navi + opencode + claude-code)
just bench-tool-quality

# Navi-only smoke (2 cases)
just bench-tool-quality-smoke

# Navi-only full suite (native bench JSON)
just bench-tool-quality-navi
```

Defaults:

| Agent | Model |
|-------|--------|
| navi | `opencode` / `deepseek-v4-flash-free` |
| opencode | `opencode/deepseek-v4-flash-free` |
| claude-code | `deepseek/deepseek-v4-flash` through `http://localhost:19429` (cc-proxy) |

Score weights success rate, failed-tool rate, tool efficiency, and wall time.
Results: `benchmarks/runs/agent-compare/latest.json` + `.md`.
