## Highlights

**0.8.1 is a patch release** — three fixes, no config or storage changes. Drop-in
upgrade from 0.8.0.

- **Goals can no longer get stuck in an endless auto-continue loop.** Goal tools
  (`get_goal` / `create_goal` / `update_goal`) are now session-core: a harness
  pack's `entry_allow_tools` (or a skill `allow_tools` list) can no longer drop
  them from the model schema or deny them at call time. Before this, an active
  goal kept auto-continuing while the model reported "`update_goal` does not
  exist" — the goal could never be marked complete or blocked.
- **`code_exec` is actually discoverable as tool chaining now.** The tool was
  always in the schema, but it was described as a "typed code-mode plan with
  controlled nested tools" with no example and no per-field docs, so models kept
  emitting one call per step. The description now says what it is for ("run
  several repo steps in ONE call"), every op field documents which op uses it,
  and the schema carries a real example. An empty `ops` array used to return
  `status: "passed"` with zero ops — a silent no-op — and is now rejected with a
  correct plan in the error.
- **Even spacing between tool calls in the transcript.** A tool card whose body
  ended with blank lines (e.g. an empty `search` result) kept that trailing blank
  row, so the next tool line sat two rows down instead of one.

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.8.0...v0.8.1

### Fixed

- **Goal auto-continuation requires a way to close the goal.** `get_goal`,
  `create_goal` and `update_goal` joined the session-core tool set, so harness
  allowlists can no longer lock them out of the model's schema or reject the
  call. Auto-continuation is also suppressed (with a warning) when `update_goal`
  is not registered at all, which covers host profiles that skip tool bootstrap
  (`host_tools_only` / `chat_only`).

- **`code_exec` tuning.** Benefit-first description, per-field documentation for
  every op, a root-level `examples` entry (which also feeds the
  invalid-arguments recovery hint and the text-only tool manifest), and
  `minItems: 1` on `ops`. An empty plan now fails with the schema problem and a
  correct example instead of silently reporting success.

- **Transcript spacing.** Tool cards never open or close with blank rows, so
  every card is separated by exactly one blank line.
