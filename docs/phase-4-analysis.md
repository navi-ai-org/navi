# Phase 4 Analysis: Subagent Reformulation

## 1. What Phase 4 Requires

Per `docs/refactoring/subagent-and-tooling-reform.md` §3, the subagent system must be simplified so subagents **inherit all base tools** and **always run in yolo mode**, removing model-controlled knobs. Specific changes:

- **`SubagentOptions` removes**: `tools`, `approval`, `max_tokens`, `write_allow`, `create_files`, `create_dirs`.
- **`SubagentOptions` keeps**: `model`, `path_deny` (plus top-level `prompt`, `description`, `background`, `task_id`, `action`).
- **Profile enum** simplified from `planner`/`explorer`/`implementer`/etc. to `repo_search`/`subagent_research` (per §4).
- **Tool inheritance**: no per-subagent allowlists; only `subagent` and `workflow` are stripped.
- **Yolo enforcement**: `SecurityPolicy` set to `PermissionMode::Yolo`; residual approval events auto-approved.
- **Removed code**: `ApprovalMode` enum, `resolve_allowed_tool_names()`, `resolve_approval_mode()`, `write_scope_from_options()`, `READONLY_DENIED_TOOLS`/`WRITE_DENIED_TOOLS`.
- **Workflow bridge**: `build_subagent_bridge_input()` must only pass `path_deny` and `model` into subagent options.
- **Workflow policy**: `AgentPolicyOpts`/`EffectiveAgentPolicy` must lose tool/approval/write fields; run `policy` table retains them.
- **Schema update**: subagent `options.properties` drops the removed fields.

## 2. Implementation Status

### 2.1 Done

| Requirement | Evidence |
|-------------|----------|
| `SubagentOptions` only `model` + `path_deny` | `crates/navi-core/src/tool/builtin/subagent.rs:30-38` |
| `ApprovalMode`/`resolve_approval_mode`/`resolve_allowed_tool_names`/`write_scope_from_options`/`READONLY_DENIED_TOOLS`/`WRITE_DENIED_TOOLS` removed | No matches in `crates/navi-core/src` for those identifiers. |
| Yolo mode enforced | `build_subagent_policy` sets `config.permission_mode = PermissionMode::Yolo` at `subagent.rs:1027`; `build_subagent_context_static` auto-approves residual `ApprovalRequested` events at `subagent.rs:837-839`. |
| Tool inheritance minus nested spawners | `build_subagent_executor` filters only `NESTED_AGENT_TOOLS` (`subagent`, `workflow`) at `subagent.rs:43` and `subagent.rs:1005-1020`. |
| `TurnContext.allowed_tool_names` unrestricted | Set to `None` in `run_foreground` (`subagent.rs:422`) and `spawn_background` (`subagent.rs:580`). |
| `definition()` schema dropped removed options | `subagent.rs:238-269`; only `model` and `path_deny` under `options.properties`; `profile` enum is `["repo_search", "subagent_research"]` at `subagent.rs:249-252`. |
| `build_subagent_context_static` no approval/escalate logic | `subagent.rs:762-846` uses a generic subagent worker prompt and auto-approves; no mode-specific text. |
| Workflow bridge input only `path_deny` + `model` | `crates/navi-core/src/tool/builtin/workflow/backends.rs:432-457`. |
| `AgentPolicyOpts`/`EffectiveAgentPolicy` simplified | `crates/navi-core/src/tool/builtin/workflow/policy.rs:40-60` contain only `profile`, `path_allow`, `path_deny`, `model`, `label`. |
| `parse_agent_opts` ignores old per-agent options | `crates/navi-core/src/tool/builtin/workflow/runtime.rs:517-534` reads only `profile`, `model`, `label`, `path_allow`, `path_deny`. |
| `fork_with_policy_and_tools` still strips nested tools | `crates/navi-core/src/tool/mod.rs:561-577`, line 575. |
| `NESTED_WORKFLOW_TOOLS` defined | `crates/navi-core/src/tool/builtin/workflow/types.rs:15`. |
| Profile registry simplified | `crates/navi-core/src/registry/store.rs:1132-1148` seeds only `repo_search` and `subagent_research`. |
| `BackgroundModelsConfig` simplified | `crates/navi-core/src/config/types.rs:1069-1088` has `default`, `repo_search`, `subagent_research` only. |
| `BackgroundModelResolver` uses task as profile | `crates/navi-core/src/background_model.rs:49-75` resolves `repo_search`/`subagent_research` directly. |

### 2.2 Partially Done / Observations

- **`WritePathScope` is still used** in `subagent.rs:1034-1040` and `workflow/backends.rs:148`. However, this is not a violation: the removed fields are gone from `SubagentOptions`; the remaining `path_deny` is applied via `SecurityPolicy`/`WritePathScope` with universal `write_allow` (`["**"]`) and `create_files=true`/`create_dirs=true` so yolo mode still allows writes while honoring `path_deny`. This is the minimal mechanism `SecurityPolicy` exposes for `path_deny`.

- **`workflow-tool-lua-spec.md` §4.3** describes `profile` as an "Optional worker label ... ignored by subagent backend." In the implementation `profile` is **not** ignored; `SubagentTool::resolve_model_for_profile` (`subagent.rs:692-727`) uses it to select the model provider. The spec wording is misleading/inconsistent with the code.

### 2.3 Not Done (Out of Core Scope or Stale)

- **`evals/suites/beyond/b4_subagents.toml`** still checks for removed identifiers (`Planner`, `SecurityReviewer`, `READONLY_DENIED_TOOLS`, `resolve_allowed_tool_names`, `code_exec`) and expects them to be present (`rg -q ...`, `required = true`). Since Phase 4 removed these, this eval will now fail. It needs to be updated to assert absence or check new behavior.

- **TUI/NAPI surfaces** are not verified in this analysis because the requested scope is `navi-core` subagent/workflow code. `navi-tui` and `navi-napi` still carry background-model lists per §4 (out of Phase 4 core scope).

## 3. Verification Results

### 3.1 Tests

```
cargo test -p navi-core tool::builtin::workflow::tests -- --test-threads=4
```

- Result: **PASS** — 43 passed, 0 failed.
- Includes production-bridge tests that validate `build_subagent_bridge_input` against the live `SubagentTool` schema (`workflow/tests.rs:731-814`, `workflow/tests.rs:891-990`).

```
cargo test -p navi-core subagent -- --test-threads=4
```

- Result: **PASS** — 13 passed, 0 failed in `navi-core` unittests + 1 parity test passed.
- Includes `schema_allows_model_and_path_deny` and `subagent_executor_strips_nested_tools` (`subagent.rs:1075-1290`).

### 3.2 Check

```
cargo check -p navi-core --tests
```

- Result: **PASS** (exit code 0).
- Warnings are pre-existing dead-code warnings in `security.rs` (`extract_shell_path_mentions`, `looks_like_path`) and are unrelated to Phase 4.

### 3.3 Grep for Removed Concepts

- `ApprovalMode`, `agent_profile`, `READONLY_DENIED_TOOLS`, `WRITE_DENIED_TOOLS`, `resolve_allowed_tool_names`, `resolve_approval_mode`, `write_scope_from_options`:
  - **No matches** in `crates/navi-core/src/tool/builtin/subagent.rs` (modulo legitimate `approval_handle`/`approval_resolver` local names and the `WritePathScope` used for `path_deny`).
  - **No matches** in `crates/navi-core/src/tool/builtin/workflow/`.
  - Only matches in `docs/refactoring/subagent-and-tooling-reform.md` (the plan itself) and the stale eval file above.

## 4. Code Quality Observations

1. **Schema/bridge coupling is tested.** `workflow/backends.rs:460-541` validates bridge input against the real `SubagentTool.definition()` schema, preventing drift between `SubagentOptions` and what `workflow` emits.
2. **Yolo enforcement is layered.** `build_subagent_policy` sets `PermissionMode::Yolo`, and the event loop auto-approves any residual `ApprovalRequested` events, making the mode robust.
3. **Tool inheritance is clean.** `build_subagent_executor` filters only `NESTED_AGENT_TOOLS` and `TurnContext.allowed_tool_names` is `None`, so the subagent sees all parent tools except recursive spawners.
4. **Workflow policy separation is preserved.** Run policy still carries `tools`/`approval`/`write_allow`/`create_files`/`create_dirs` (workflow-wide), while per-agent options only narrow `path_allow`/`path_deny` and override `profile`/`model`/`label`. This matches §4.3 of `workflow-tool-lua-spec.md`.
5. **Stale external artifact.** The eval `b4_subagents.toml` has not been updated for the reformulation and will fail in CI.

## 5. Gaps and Recommended Next Steps

1. **Update `evals/suites/beyond/b4_subagents.toml`** to verify absence of the removed identifiers or to test the new behavior (yolo mode, tool inheritance, nested-tool stripping). It currently expects the removed symbols to exist.
2. **Clarify `docs/workflow-tool-lua-spec.md` §4.3** regarding `profile`. The current text says `profile` is ignored by the subagent backend, but the subagent backend uses it for model resolution. Either the spec or the implementation should be reconciled.
3. **TUI/NAPI sweep** (outside Phase 4 core but part of the broader plan): verify `navi-tui` background-model modals and `navi-napi` config serialization only expose `repo_search` and `subagent_research`.
4. **`RunPolicy.approval` still present but not forwarded.** `workflow/mod.rs` still lists `approval` in the `policy` schema (`mod.rs:305`) and `RunPolicy` stores it (`policy.rs:26`). This is not a Phase 4 bug — `RunPolicy` is the workflow-wide source of truth and the spec (`workflow-tool-lua-spec.md` §5.1) still documents an `approval` field. It is simply not propagated to subagents because subagents are always yolo. No action required unless a later spec revision removes it.

## 6. Conclusion

Phase 4 subagent reformulation is **fully implemented in `navi-core`**. The `SubagentOptions` schema, tool inheritance, yolo enforcement, workflow bridge, and workflow policy intersection all match the plan. All targeted tests and `cargo check` pass. The only actionable follow-ups are the stale eval file, a minor spec inconsistency, and non-core TUI/NAPI cleanup.
