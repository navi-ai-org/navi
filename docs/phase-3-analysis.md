# Phase 3 Analysis: Background Model Profile Simplification

## 1. What Phase 3 Requires

Per `docs/refactoring/subagent-and-tooling-reform.md` §4, the background-model/profile surface is reduced from:

`cheap_general`, `cheap_code`, `repo_search`, `naming`, `long_context_cheap`, `research_synthesis`, `simple_code_edit`, `compaction`

to three concepts:

| Concept | Behavior |
|---|---|
| `unspecified` / omitted (default) | Use the main chat model (`config.model`) |
| `repo_search` | User config `background_models.repo_search`; if unset, fall back to the main chat model |
| `subagent_research` | User config `background_models.subagent_research`; spec says it must be explicitly defined by the user |

Required removals include the old registry seed profiles, the extra `BackgroundModelsConfig` fields, the old resolver mappings, the subagent `profile` enum values, and stale TUI/NAPI/runtime references.

## 2. Current Implementation Status

| Area | Status | File / Line Citations |
|---|---|---|
| Config struct simplified to `default`, `repo_search`, `subagent_research` | Done | `crates/navi-core/src/config/types.rs:1069-1088` (`BackgroundModelsConfig`); `resolve()` only handles the two task keys (lines 1080-1087) |
| Registry default profiles seeded | Done | `crates/navi-core/src/registry/store.rs:1132-1153` — only `repo_search` and `subagent_research` are seeded |
| Registry `Profile`/`RankedModel` types | Done | `crates/navi-core/src/registry/types.rs:31-65` — comments reference only `repo_search`/`subagent_research` |
| Background model resolver | Mostly done | `crates/navi-core/src/background_model.rs:49-75` resolves explicit override → profile → main-model fallback; tests at lines 151-208 use only `repo_search`/`subagent_research` |
| Subagent tool schema profile enum | Done | `crates/navi-core/src/tool/builtin/subagent.rs:249-252` — `"enum": ["repo_search", "subagent_research"]`; omitting `profile` uses main model (line 693) |
| Subagent options stripped | Done | `crates/navi-core/src/tool/builtin/subagent.rs:31-38` — `SubagentOptions` now only `model` and `path_deny` |
| Workflow bridge input | Done | `crates/navi-core/src/tool/builtin/workflow/backends.rs:438-457` — only `path_deny` and optional `model` are forwarded in `options` |
| Workflow policy options | Done | `crates/navi-core/src/tool/builtin/workflow/policy.rs:41-60` — `AgentPolicyOpts`/`EffectiveAgentPolicy` only carry `profile`, `path_allow`, `path_deny`, `model`, `label` |
| TUI background models list | Done | `crates/navi-tui/src/view/modals.rs:2139-2141` and `:2362-2364` list only the two tasks; `resolve_bg_model_label` / `bg_model_has_override` (lines 2212-2230) check only those fields |
| TUI keybindings/model picker | Done | `crates/navi-tui/src/keybindings/modals.rs:2106-2109` (`BG_MODEL_TASKS`); `set_bg_model_override` / `clear_bg_model_override` (lines 2281-2304) only touch `repo_search`, `subagent_research`, `default`; `crates/navi-tui/src/view/model_picker.rs:390-403` (`bg_model_is_current_override`) matches |
| NAPI config serialization | Done | `crates/navi-napi/src/lib.rs:1974-1978` emits only `default`, `repoSearch`, `subagentResearch`; `set_background_model` doc at line 1278 lists `repo_search\|subagent_research\|default` |
| NAPI TypeScript types | Done | `crates/navi-napi/index.d.ts:237-241` (`EngineConfig.backgroundModels`) only exposes `default`, `repoSearch`, `subagentResearch` |
| SDK routing operations | Done | `crates/navi-sdk/src/routing_ops.rs:14-16` (`BACKGROUND_TASKS`); `normalize_bg_task` (lines 30-41) and `set_background_model` / `clear_background_model` (lines 113-180) only handle `default`, `repo_search`, `subagent_research` |
| Parity/integration tests | Done | `crates/navi-core/tests/parity_check.rs:336-345` (`p10_subagent_options_serde_roundtrip`) uses only `model` and `path_deny`; no old profile names appear |
| Old profile names removed from code | Done | `grep` across `crates/` found no remaining uses of `cheap_general`, `cheap_code`, `naming`, `long_context_cheap`, `research_synthesis`, or `simple_code_edit` as profile/task identifiers (only unrelated occurrences such as the word "naming" in comments) |

## 3. Test and Check Results

```bash
cargo check -p navi-core                         # OK (exit 0)
cargo check -p navi-tui -p navi-sdk -p navi-napi # OK (exit 0)
cargo test -p navi-core -- --test-threads=4      # OK (exit 0)
```

- `navi-core` lib tests: **1031 passed, 0 failed**
- `navi-core` integration/parity tests (`tests/parity_check.rs`): **21 passed, 0 failed**
- Only warnings are two pre-existing `dead_code` warnings in `crates/navi-core/src/security.rs:1388` and `:1544` (`extract_shell_path_mentions`, `looks_like_path`); unrelated to Phase 3.

## 4. Code Quality Observations

- The subagent surface is cleanly aligned with the simplified profile set: the schema, options, and resolver all know only `repo_search` and `subagent_research`.
- Compaction no longer uses a separate background model; `crates/navi-core/src/session.rs:805-816` and `crates/navi-core/src/compact.rs` use the active session model.
- Memory extraction no longer appears as a `background_models` task; there are no `memory_extraction_model` or `compaction_model_name`/`compaction_provider` fields in `runtime/mod.rs` or `turn/mod.rs`.
- The TUI has two copies of the background-model task list (legacy modal + model routing Agents tab). Both are correct, but they could share `BG_MODEL_TASKS` from `keybindings/modals.rs` to avoid drift.

## 5. Gaps and Recommended Next Steps

1. **`navi-tui/src/mouse.rs:1358` hardcoded `len = 5usize`**
   - This is a leftover from the previous longer background-model task list. With only two tasks (`repo_search`, `subagent_research`), mouse wheel scrolling can set `bg_models_selected` beyond the actual list.
   - **Fix:** change `let len = 5usize;` to `BG_MODEL_TASKS.len()` (or `2`). The keyboard handler already uses `BG_MODEL_TASKS.len()` correctly.

2. **`subagent_research` fallback semantics**
   - The Phase 3 spec says `subagent_research` "must be explicitly defined by the user" and the resolver should "require explicit config".
   - `crates/navi-core/src/background_model.rs:69-74` falls back to the main chat model for *any* task, including `subagent_research`, and the test at line 204 expects this behavior.
   - **Decision needed:** either enforce explicit config for `subagent_research` (return an error or use a sentinel when unset) and update the test, or update the spec to match the current fallback behavior. The current implementation is simpler but deviates from the written plan.

3. **`navi-server` route serialization shape (optional)**
   - `crates/navi-server/src/routes/skills_mcp.rs:309-311` serializes `backgroundModels`/`background_models` directly from `BackgroundModelsConfig`, which uses snake-case keys (`repo_search`, `subagent_research`). The NAPI surface manually camelCases to `repoSearch`/`subagentResearch`.
   - If external REST consumers expect camelCase, the server route should build a camelCase object rather than serializing the Rust struct directly.

4. **Documentation update**
   - The refactoring plan itself (`docs/refactoring/subagent-and-tooling-reform.md`) still describes the *intended* state. Once the above gaps are resolved, the doc can be updated from "Draft" to reflect completion or removed if it is no longer the source of truth.

## 6. Summary

Phase 3 is **substantially complete** in the codebase. The old profiles are gone from `navi-core`, `navi-tui`, `navi-sdk`, `navi-napi`, and `navi-server`; the remaining surface exposes only `repo_search`, `subagent_research`, and the main-model fallback. All targeted `cargo check` and `cargo test` runs pass. The only outstanding items are a small TUI scroll constant (`mouse.rs:1358`) and the semantic question of whether `subagent_research` should require explicit user configuration or may fall back to the main model.
