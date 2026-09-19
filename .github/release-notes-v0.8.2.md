## Highlights

**0.8.2 is a patch release** — three product fixes plus the release-pipeline fix
that blocked 0.8.1. No config or storage changes; drop-in upgrade from 0.8.0.

> **Note:** 0.8.1 was never published and its tag has been withdrawn. Docker Hub
> stopped publishing `rust:nightly*` image tags, so the musl builds could not
> start and the release job was skipped. 0.8.2 is the first release built with
> the fix.

- **Linux release builds work again.** Both musl jobs (and `warm-cache.yml`,
  which used the same image) build on stable `rust:alpine`; the pinned toolchain
  still comes only from `rust-toolchain.toml` via `rustup install`.
- **Goals can no longer get stuck in an endless auto-continue loop.** Goal tools
  (`get_goal` / `create_goal` / `update_goal`) are now session-core: a harness
  pack's `entry_allow_tools` (or a skill `allow_tools` list) can no longer drop
  them from the model schema or deny them at call time.
- **`code_exec` is actually discoverable as tool chaining now.** The description
  says what it is for ("run several repo steps in ONE call"), every op field
  documents which op uses it, and the schema carries a real example. An empty
  `ops` array used to return `status: "passed"` with zero ops and is now rejected
  with a correct plan in the error.
- **Even spacing between tool calls in the transcript.** A tool card whose body
  ended with blank lines (e.g. an empty `search` result) kept that trailing blank
  row, so the next tool line sat two rows down instead of one.

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.8.0...v0.8.2

### Fixed

- **Release pipeline.** `rust:nightly-alpine` no longer exists on Docker Hub, so
  the Linux release jobs failed at container init and `GitHub Release` was
  skipped. Both workflows now use stable `rust:alpine` and document why.

- **Goal auto-continuation requires a way to close the goal.** `get_goal`,
  `create_goal` and `update_goal` joined the session-core tool set. Auto-
  continuation is also suppressed (with a warning) when `update_goal` is not
  registered at all, which covers host profiles that skip tool bootstrap
  (`host_tools_only` / `chat_only`).

- **`code_exec` tuning.** Benefit-first description, per-field documentation for
  every op, a root-level `examples` entry (which also feeds the
  invalid-arguments recovery hint and the text-only tool manifest), and
  `minItems: 1` on `ops`.

- **Transcript spacing.** Tool cards never open or close with blank rows, so
  every card is separated by exactly one blank line.
