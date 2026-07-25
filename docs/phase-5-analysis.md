# Phase 5 Analysis: Memory Extraction → Forked Main Session

## 1. Summary of Phase 5 Requirements

Phase 5 requires memory extraction to stop using a separate, dedicated background model and instead run in a **forked session of the principal session using the main chat model**, so the provider prompt cache is reused and cost is minimized. The concrete changes are:

- Remove `MemoryExtractionModel` and the `memory_extraction_model` field from `AgentRuntimeOptions` and `AgentRuntime`.
- Rewrite `try_extract_memories()` to clone the live session messages and call the main chat model (`shared_model_provider` / `shared_model_name`).
- Remove the opt-in gate: extraction should always run when memory is enabled.
- Remove `memory_extraction` from `BackgroundModelsConfig`.
- Remove the `"memory_extraction"` task mapping from `BackgroundModelResolver`.
- Remove `memory_extraction` / `memoryExtraction` from TUI, NAPI, SDK, and lite surfaces.
- Update `extract.rs` to support a forked-session variant that appends the extraction prompt to existing messages.
- Update `docs/auto-memory.md` if it still describes the old model-selection behavior.

## 2. Current Implementation Status

| Requirement | Status | File / Line Citations |
|-------------|--------|----------------------|
| Remove separate `memory_extraction_model` from runtime | **Done** | `crates/navi-core/src/runtime/mod.rs:291-327` (`AgentRuntimeOptions` has no such field), `crates/navi-core/src/runtime/mod.rs:331-388` (`AgentRuntime` has no such field). `grep` for `MemoryExtractionModel` / `memory_extraction_model` in `/home/enrell/projects/navi` returns only matches in `docs/refactoring/subagent-and-tooling-reform.md`. |
| `try_extract_memories()` forks principal session messages and uses main chat model | **Done** | `crates/navi-core/src/runtime/mod.rs:1877-1946`. It fetches the memory manager, reads `shared_model_provider` and `shared_model_name`, sends `SessionCommand::GetMessages` to the session runtime, and calls `extract_memories_from_messages` with the cloned messages and main model. |
| Remove opt-in gate (extraction always runs when memory enabled) | **Done** | `crates/navi-core/src/runtime/mod.rs:1878-1890` gates only on memory manager initialization and `auto_memory.db_path` existence. No check for a configured extraction model. `crates/navi-core/src/runtime/mod.rs:1203-1212` triggers extraction unless the model used `memory(write)` this turn. |
| Add forked-session variant in `extract.rs` | **Done** | `crates/navi-core/src/memory/extract.rs:71-92` provides `extract_memories_from_messages`, which appends a final user message with the extraction prompt. `crates/navi-core/src/memory/extract.rs:66-70` explicitly documents this as the forked-main-session path. |
| Remove `memory_extraction` from `BackgroundModelsConfig` | **Done** | `crates/navi-core/src/config/types.rs:1066-1088` defines `BackgroundModelsConfig` with only `default`, `repo_search`, and `subagent_research`. `resolve()` only handles those three keys. |
| Remove `"memory_extraction"` from `BackgroundModelResolver` | **Done** | `crates/navi-core/src/background_model.rs:42-75` resolves only `repo_search` / `subagent_research` and falls back to the main chat model. No `memory_extraction` branch. |
| TUI background models list cleaned | **Done** | `crates/navi-tui/src/view/modals.rs:2129-2365` lists only `repo_search` and `subagent_research`. `crates/navi-tui/src/keybindings/modals.rs:2107-2109` (`BG_MODEL_TASKS`) and `crates/navi-tui/src/keybindings/modals.rs:2282-2303` (`set_bg_model_override` / `clear_bg_model_override`) handle only `repo_search` and `subagent_research`. No `MemoryModel` setup phase found in `setup.rs` or `keybindings/modals.rs`. |
| NAPI config serialization cleaned | **Done** | `crates/navi-napi/src/lib.rs:1974-1978` serializes only `default`, `repoSearch`, and `subagentResearch`. `set_background_model` doc string at `crates/navi-napi/src/lib.rs:1278` says `repo_search|subagent_research|default`. |
| SDK background-task normalization cleaned | **Done** | `crates/navi-sdk/src/routing_ops.rs:15` (`BACKGROUND_TASKS`) and `crates/navi-sdk/src/routing_ops.rs:30-41` (`normalize_bg_task`) only accept `default`, `repo_search`, `subagent_research`. `set_background_model` / `clear_background_model` match only those keys. |
| `navi-lite` references cleaned | **Done** | `grep` for `memory_extraction` / `background_model` / `BackgroundModels` in `crates/navi-lite/src/lib.rs` returns no matches. |
| `docs/auto-memory.md` updated | **Not done** | `docs/auto-memory.md:65-69` describes extractMemories generically as "a background `tokio::spawn` calls the model" without specifying that it now uses the **main chat model** in a **forked session**. It does not mention the removed background-model override. |

### Key Code Citations

- **Forked session message retrieval**: `crates/navi-core/src/session.rs:706-710` defines `SessionCommand::GetMessages`; `crates/navi-core/src/session.rs:785-787` clones `messages` and returns them. This is the fork primitive used by `try_extract_memories`.
- **Main chat model read**: `crates/navi-core/src/runtime/mod.rs:1899-1908` clones `shared_model_provider` and `shared_model_name`.
- **Extraction call**: `crates/navi-core/src/runtime/mod.rs:1928-1933` calls `extract_memories_from_messages(messages, provider.as_ref(), &model_name, &store).await`.
- **Extraction prompt appending**: `crates/navi-core/src/memory/extract.rs:77-78` pushes a user message containing `EXTRACT_SYSTEM` plus the extraction request.

## 3. Test and Check Results

| Command | Result | Notes |
|---------|--------|-------|
| `cargo test -p navi-core memory::extract -- --test-threads=4` | **Pass** | 9 passed, 0 failed. |
| `cargo test -p navi-core runtime::tests -- --test-threads=4` | **Pass** | 6 passed, 0 failed. |
| `cargo check -p navi-core --tests` | **Pass** | 2 pre-existing dead-code warnings in `security.rs`, not related to Phase 5. |
| `cargo check -p navi-sdk -p navi-lite -p navi-napi -p navi-tui` | **Pass** | Same 2 warnings from `navi-core`; dependent crates build cleanly. |

## 4. Code Quality Observations

- **Clean surface removal**: All `memory_extraction` / `MemoryExtraction` / `memoryExtraction` identifiers have been removed from the source tree. Only the refactoring plan document still references them.
- **Prompt-cache intent is documented**: `extract_memories_from_messages` is explicitly documented as the forked-main-session path, and `try_extract_memories` sends `GetMessages` to clone live messages before appending the extraction prompt.
- **Unused helper duplication**: `extract_memories` (`crates/navi-core/src/memory/extract.rs:49-64`) is now only used by its own unit tests. It builds `[system(EXTRACT_SYSTEM), user(EXTRACT_USER)]` and then calls `extract_memories_from_messages`, which appends another user message containing `EXTRACT_SYSTEM` again. This causes the extraction system text to appear twice (once as system, once as user) in the test path. The production path via `GetMessages` does not include `EXTRACT_SYSTEM` up front, so production is not affected, but the test helper is slightly awkward and could be simplified.
- **Mutual exclusion is preserved**: The `turn_used_memory_write` flag still gates extraction, so explicit `memory(write)` calls take precedence over background extraction.
- **No observable error propagation**: `try_extract_memories` is fire-and-forget and logs `tracing::debug` on failure, matching the design.
- **Background models remain consistent across boundaries**: `navi-core`, `navi-sdk`, `navi-napi`, and `navi-tui` all agree on the same three keys (`default`, `repo_search`, `subagent_research`).

## 5. Gaps and Recommended Next Steps

1. **Update `docs/auto-memory.md`** to describe the forked-main-session behavior. Add a note that extractMemories reuses the main chat model by cloning the live conversation and appending an extraction prompt, and that there is no separate memory-extraction model override.
2. **Consider simplifying `extract_memories`** so the non-forked helper does not duplicate `EXTRACT_SYSTEM` when it calls `extract_memories_from_messages`. Options:
   - Have `extract_memories` build only the conversation user message and let `extract_memories_from_messages` append the extraction system prompt.
   - Or keep `extract_memories` for standalone use but do not have it call `extract_memories_from_messages`.
   This is a code-quality issue, not a functional bug, since the production path uses `extract_memories_from_messages` directly.
3. **Confirm prompt-cache hit in practice**: The implementation assumes providers cache the prefix of the cloned message list. If the appended user message changes the cache key for the entire suffix, cost savings depend on provider behavior. No code change required, but worth validating with telemetry or provider-specific testing.
4. **No source code changes were made** for this analysis. If the documentation update is approved, edit `docs/auto-memory.md` only.
