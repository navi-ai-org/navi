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
cargo run -p navi-cli -- bench run benchmarks/suites/smoke --auto-approve
```

Write a comparable JSON run:

```bash
cargo run -p navi-cli -- bench run benchmarks/suites/smoke \
  --auto-approve \
  --output benchmarks/runs/candidate.json
```

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
