# NAVI eval suites

Local harness evals are versioned verifier-replay cases used to compare harness
modes and catch regressions.

Run the B0 baseline suite:

```bash
cargo run -p navi-cli -- eval run evals/suites/b0
```

Run the beyond-parity implementation suite:

```bash
cargo run -p navi-cli -- eval run evals/suites/beyond
```

Run the local harness replay gate:

```bash
just harness-replay
```

Use JSON output for tooling:

```bash
cargo run -p navi-cli -- eval run evals/suites/b0 --json
```

Each case is a `.toml` or `.json` file with this shape:

```toml
version = 1
id = "stable-case-id"
title = "Human readable title"
category = "simple_repo_task"
mode = "parity"
task = "Task prompt or objective."
tags = ["b0"]

[[verifiers]]
verifier_type = "command"
command = "test -f Cargo.toml"
timeout_ms = 30000
required = true
```

Required fields:

- `version`: currently `1`.
- `id`: stable and unique inside a suite directory.
- `title`, `category`, `task`.
- at least one non-empty verifier command.

Supported modes:

- `parity`
- `verifier-first`
- `branch-race`

The B0 runner replays setup and verifier commands through `VerifierRunner`.
Agent token/tool metrics are optional until eval cases are wired to full runtime
turn replay; when absent, token-derived aggregate metrics are serialized as
`null` and printed as `n/a`.

Generate eval candidates and dataset JSONL from stored traces:

```bash
cargo run -p navi-cli -- eval generate-from-traces ~/.local/share/navi \
  --output-dir evals/generated \
  --dataset-jsonl evals/generated/dataset.jsonl
```

Evaluate a replay/superiority gate from saved `EvalRun` JSON:

```bash
cargo run -p navi-cli -- eval gate candidate.json \
  --baseline baseline.json \
  --trace-data-dir ~/.local/share/navi \
  --min-success-rate 1.0 \
  --max-success-drop 0.0 \
  --superiority
```
