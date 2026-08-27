# Phase 2 Analysis: Deferred → Direct Tool Exposure

## Scope

Analyze the status of NAVI Phase 2 refactor: promoting `browser`, `code`, `code_edit`, `code_exec`, and `subagent` from `ToolExposure::Deferred` to `ToolExposure::Direct`.

Source of truth: `docs/refactoring/subagent-and-tooling-reform.md` §1 (“Tool Exposure: Deferred → Direct”).

## Summary of Required Changes

1. Change the five tools to `ToolExposure::Direct` in `crates/navi-core/src/tool/builtin/metadata.rs`.
2. Update hardcoded capability inventory lists that mirror exposure:
   - `crates/navi-cli/src/harness_cmd.rs`
   - `crates/navi-core/src/harness_pack/materialize.rs`
3. Update `crates/navi-core/src/tool/registry.rs` tests that used the five tools as Deferred examples.
4. Update any schema/prompt/test code that assumes these tools are Deferred.

No source code modifications were made during this analysis. The existing tree already contains the promotion.

## Current Implementation Status

| Requirement | Status | Evidence |
|-------------|--------|----------|
| `ToolExposure` enum exists with `Direct`/`Deferred` variants | Done | `crates/navi-core/src/tool/metadata.rs:228-242` |
| `browser` → `Direct` | Done | `crates/navi-core/src/tool/builtin/metadata.rs:183` |
| `code` → `Direct` | Done | `crates/navi-core/src/tool/builtin/metadata.rs:222` |
| `code_edit` → `Direct` | Done | `crates/navi-core/src/tool/builtin/metadata.rs:240` |
| `code_exec` → `Direct` | Done | `crates/navi-core/src/tool/builtin/metadata.rs:258` |
| `subagent` → `Direct` | Done | `crates/navi-core/src/tool/builtin/metadata.rs:474` |
| `navi-cli` harness direct list updated | Done | `crates/navi-cli/src/harness_cmd.rs:34-38` lists the five tools in `direct`; deferred list at `:41-48` no longer contains them |
| `materialize` default inventory updated | Done | `crates/navi-core/src/harness_pack/materialize.rs:199-203` lists the five tools in `direct`; deferred list at `:206-213` no longer contains them |
| Registry test uses remaining Deferred examples | Done | `crates/navi-core/src/tool/registry.rs:373-382` uses `repo_explore`, `ast_search`, `symbol_goto` as Deferred examples |
| `tool/tests.rs` expects the five in visible set | Done | `crates/navi-core/src/tool/tests.rs:2030-2049` asserts `code`, `code_edit`, `code_exec`, `browser` are in `executor.definitions()`; `subagent` is covered by `builtin/metadata.rs` tests at `:680-705` |
| `inventory_build.rs` partitions exposure correctly | Done | `crates/navi-core/src/harness_pack/inventory_build.rs:20-26` routes `Direct | ModelOnly` to direct and `Deferred | Hidden` to deferred |
| `registry.rs` `visible_definitions` filters correctly | Done | `crates/navi-core/src/tool/registry.rs:76-87` returns only `Direct | ModelOnly` definitions |

Overall: **Phase 2 exposure promotion is implemented and consistent across the metadata, harness, and materialization paths.**

## Verification Results

### `grep` exposure audit

```text
ToolExposure::Direct  - 24 occurrences (metadata, registry, tests, harness)
ToolExposure::Deferred - 25 occurrences (still used for ast_search, symbol_*, repo_explore, init_session, mark_feature_done, current_time, sleep, get_context_remaining, new_context_window, view_image, append_note, history_ops, wait, sandbox, and test fixtures)
```

The five target tools no longer have `ToolExposure::Deferred` anywhere in the source.

### Build / test

```bash
cargo check -p navi-core
```

- Result: **success** (exit 0)
- Warnings: 2 dead-code warnings in `crates/navi-core/src/security.rs` (`extract_shell_path_mentions` at `:1388`, `looks_like_path` at `:1544`); unrelated to Phase 2.

```bash
cargo test -p navi-core -- --test-threads=4
```

- Result: **all passed**
- `navi-core` unit tests: 1031 passed
- `tests/parity_check.rs`: 21 passed
- Doc-tests: 0
- Total: 1052 passed / 0 failed

### Targeted parity tests

```bash
cargo test -p navi-core p1_tool_metadata_exists_for_all_builtin_tools -- --test-threads=4
```

- Result: `p1_tool_metadata_exists_for_all_builtin_tools ... ok`

```bash
cargo test -p navi-core p2_deferred_tools_not_in_visible_definitions -- --test-threads=4
```

- Result: `p2_deferred_tools_not_in_visible_definitions ... ok`

## Code Quality Observations

1. **Feature gating is correct.** `code` and `code_edit` are registered only when the `code-vfs` feature is enabled; `browser` only when `browser` is enabled. Both are default features in `crates/navi-core/Cargo.toml:45`, so the promoted tools are visible in normal builds.

2. **Subagent registration is runtime/SDK-level, not `ToolExecutor` default.** `subagent` is registered in `crates/navi-core/src/runtime/mod.rs:1704-1715` and `crates/navi-sdk/src/engine.rs:506-517`. Its metadata is `Direct`, so it will appear in the model schema once the runtime is wired.

3. **Goal tools are also `Direct`.** `crates/navi-core/src/goal/tools.rs:77,176,311` sets `get_goal`, `create_goal`, `update_goal` to `ToolExposure::Direct`.

4. **Stale documentation/comments remain.**
   - `crates/navi-core/src/runtime/tests.rs:96` still claims “Goal tools are Deferred exposure,” which contradicts the current `Direct` metadata.
   - `crates/navi-core/src/tool/mod.rs:983-994` `tool_search` hint still lists `subagent` and `package` as discoverable/deferred examples and includes `subagent` in the `power_catalog` as if it needs discovery.
   - `crates/navi-core/src/harness.rs:336-339` Discovery paragraph tells the model to use `tool_search` for `ast_search`, `symbol_*`, and `repo_explore` (correct, since they are still Deferred), but the preceding “Power tools” paragraph at `:300-307` already lists `code`, `code_edit`, `browser`, and `subagent` as schema-visible. The prompt is therefore mostly correct but still implicitly frames `subagent` as a deferred/discoverable power tool.

5. **Parity test `visible.len() <= 15` may be too tight for the full runtime.** `crates/navi-core/tests/parity_check.rs:38-42` asserts the bare `ToolExecutor` visible tool count is ≤ 15. A bare `ToolExecutor` does not register `subagent`, `set_session_title`, or the goal tools. When the runtime/SDK registers those (all `Direct`), the visible schema will grow past 15. This did not fail unit tests because the test uses `ToolExecutor::new` only.

## Gaps and Recommended Next Steps

1. **Update `tool_search` hint / power catalog.** Remove `subagent` from the deferred-examples list and remove the `package` reference (the `package_manager` tool has already been removed from source). File: `crates/navi-core/src/tool/mod.rs:983-994`.

2. **Clarify the harness system prompt.** Keep the Discovery paragraph for still-deferred tools (`ast_search`, `symbol_*`, `repo_explore`) but make it clear that `code`, `code_edit`, `code_exec`, `browser`, and `subagent` are now directly in the schema. File: `crates/navi-core/src/harness.rs:300-307,336-339`.

3. **Reconcile the `visible.len() <= 15` parity assertion.** Decide whether the 15-tool cap is still a product target. If the runtime registers `subagent`, `set_session_title`, and the three goal tools, the cap will be exceeded. Update the assertion or document the cap as applying only to the bare `ToolExecutor`. File: `crates/navi-core/tests/parity_check.rs:38-42`.

4. **Fix stale goal-tool comment.** Change `crates/navi-core/src/runtime/tests.rs:96` to reflect that goal tools are `Direct`.

5. **Rename `tool_search_discovers_deferred_power_tools`.** The test at `crates/navi-core/src/tool/tests.rs:2104` now succeeds because `code` is `Direct` and `tool_search` returns all tools; the test name still implies `code` is deferred. Rename to `tool_search_discovers_code_and_symbol_tools` or similar.

6. **Confirm `package_manager` removal is intentional.** `package_manager` no longer exists in the source tree (only in `docs/refactoring/subagent-and-tooling-reform.md` and `docs/phase-1-analysis.md`), so the hardcoded lists are already consistent. If this was removed out-of-band, note it in the phase log.

## Conclusion

Phase 2’s core exposure promotion is **complete**. The five power tools are `ToolExposure::Direct`, the harness/materialize inventories are updated, and the relevant tests pass. Remaining work is **cleanup of stale user-facing text** (system prompt, `tool_search` catalog, comments) and a possible **parity-test threshold adjustment** once the full runtime tool surface is considered.
