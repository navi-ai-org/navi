# NAVI Dead-Code Audit: Phases 1–5 Refactoring

## Scope and Commits Inspected

- **Repository:** `/home/enrell/projects/navi`
- **Audit focus:** Dead/unreachable code left over from the phase 1–5 tooling/subagent/background-model refactoring.
- **Key commits inspected (`git log --oneline -30`):**
  - `29337d0` fix(core,tui,docs): address phase-analysis gaps
  - `a96e213` docs: phase 1-5 refactoring analysis reports
  - `90ff72f` feat(core): subagent reform and forked-session memory extraction
  - `2232f47` refactor(core,cli): promote browser/code/code_edit/code_exec/subagent to Direct exposure
  - `39cd1cd` refactor(core,cli,tui): remove package_manager builtin tool
  - `6c98551` feat(security): unblock data_dir reads; gate writes by approval mode
- **Refactoring plan document:** `docs/refactoring/subagent-and-tooling-reform.md`
- **Analysis documents reviewed:** `docs/phase-{1,2,3,4,5}-analysis.md`

## Methodology

Commands run during the audit:

```bash
# Whole-workspace checks
cargo check --all
cargo check -p navi-core --tests
cargo clippy --all -- -W dead_code

# Targeted tests (all passed)
cargo test -p navi-core security -- --test-threads=4
cargo test -p navi-core memory::extract -- --test-threads=4
cargo test -p navi-core subagent -- --test-threads=4
cargo test -p navi-core background_model -- --test-threads=4
cargo check -p navi-tui
cargo check -p navi-sdk -p navi-napi -p navi-lite

# Grep sweeps for removed identifiers
grep -E "package_manager|AgentProfile|ApprovalMode|READONLY_DENIED_TOOLS|WRITE_DENIED_TOOLS|resolve_allowed_tool_names|resolve_approval_mode|write_scope_from_options|memory_extraction|MemoryExtraction" -R crates/
grep -E "cheap_general|cheap_code|naming|long_context_cheap|research_synthesis|simple_code_edit|compaction" -R crates/
```

All workspace checks and targeted tests completed successfully. The only compiler warnings were two `dead_code` warnings in `crates/navi-core/src/security.rs`.

## Dead-Code Candidates

| File | Line / Identifier | Type | Why it is dead | Confidence | Safe to delete? | Notes / Deletion risk |
|------|-------------------|------|----------------|------------|-----------------|----------------------|
| `crates/navi-core/src/security.rs` | `1388` `extract_shell_path_mentions` | private `fn` | Emitted by `cargo check`/`cargo clippy` `-W dead_code`; never called in production or tests. | High | Yes | Remove together with `looks_like_path`. |
| `crates/navi-core/src/security.rs` | `1544` `looks_like_path` | private `fn` | Only called by the dead `extract_shell_path_mentions` (line `1399`); no other callers. | High | Yes | Remove together with `extract_shell_path_mentions`. |
| `crates/navi-core/src/memory/extract.rs` | `49` `extract_memories` | public `async fn` | Only used by its own unit tests. Production path (`runtime/mod.rs:1928`) calls `extract_memories_from_messages` directly. | High | Yes, with test updates | Update the 6 unit tests to call `extract_memories_from_messages` with a single user message. Avoids the duplicate `EXTRACT_SYSTEM` noted in `docs/phase-5-analysis.md`. |
| `crates/navi-core/src/tool/builtin/subagent.rs` | `58` `_prompt_cache` field | struct field | Stored at line `90` but never read. `run_foreground`/`spawn_background` create a fresh `PromptCache` for each subagent turn. | High | Yes | Remove the field and the `prompt_cache` constructor parameter (line `79`). Callers in `runtime/mod.rs`, `navi-sdk/src/engine.rs`, `workflow/backends.rs`, `workflow/tests.rs`, and `subagent.rs` tests need updating. |
| `crates/navi-core/src/tool/builtin/subagent.rs` | `100` `with_background_resolver` | public `fn` | No callers anywhere in the workspace. | High | Yes | Remove method; then `background_resolver`/`provider_builder` become removable. |
| `crates/navi-core/src/tool/builtin/subagent.rs` | `63` `background_resolver` field | struct field | Always `None` in `new` (line `94`) and only set by the unused `with_background_resolver`. | High | Yes | Remove together with `with_background_resolver`. |
| `crates/navi-core/src/tool/builtin/subagent.rs` | `67` `provider_builder` field | struct field | Only set/read if `with_background_resolver` is called; otherwise always `None`. | High | Yes | Remove together with `with_background_resolver` and `ProviderBuilderFn`. |
| `crates/navi-core/src/tool/builtin/subagent.rs` | `46` `ProviderBuilderFn` | public `type` alias | Only used by the dead `provider_builder` field and `with_background_resolver` parameter. | High | Yes | Also remove `pub use` in `tool/builtin/mod.rs:58` and `tool/mod.rs:37`/`lib.rs:181`. |
| `crates/navi-core/src/background_model.rs` | `11` `ResolvedBackgroundModel` | public `struct` | Only produced/consumed by `BackgroundModelResolver`. | Medium-High | Yes, if resolver removed | No external callers in SDK/TUI/NAPI. |
| `crates/navi-core/src/background_model.rs` | `22` `BackgroundModelResolver` | public `struct` | Referenced only as an `Option` field in `SubagentTool` and constructed only in its own tests. | Medium-High | Yes, if resolver removed | The `profile` field in the subagent schema is currently a no-op because no resolver is ever wired. |
| `crates/navi-core/src/background_model.rs` | `30` `BackgroundModelResolver::new` | public `fn` | Called only by `#[cfg(test)]` `test_resolver`. | Medium-High | Yes, with module removal | No runtime constructor found in `navi-core`, `navi-sdk`, `navi-tui`, `navi-napi`, or `navi-lite`. |
| `crates/navi-core/src/background_model.rs` | `52` `BackgroundModelResolver::resolve` | public `fn` | Only reachable from `SubagentTool::resolve_model_for_profile`, but `background_resolver` is always `None`. | Medium-High | Yes, with module removal | Would be dead code even if wired because `seed_default_profiles` is also uncalled (see below). |
| `crates/navi-core/src/background_model.rs` | entire module (`1-213`) | module | No runtime callers; only exercises its own tests. | Medium-High | Yes, with import/export cleanup | Remove `pub use` in `lib.rs:195` and `use crate::background_model::BackgroundModelResolver` in `subagent.rs:13`. |
| `crates/navi-core/src/registry/store.rs` | `1132` `seed_default_profiles` | public `fn` | Called only by registry tests. No runtime invocation from `load_registry`, `build.rs`, or any initialization path. | Medium | Conditional | If the profile-based background model resolver is removed, this function becomes entirely dead. If the resolver is kept and wired, this function (or equivalent seeding) must be called at runtime. |
| `crates/navi-core/src/registry/store.rs` | `1092` `query_models_by_profile` | public `fn` | Only called by the unused `BackgroundModelResolver` and registry tests. | Medium | Conditional | Same as above. |
| `crates/navi-core/src/tool/builtin/subagent.rs` | `249-252` `profile` schema enum | JSON schema field | Reachable but functionally a no-op because the resolver is never wired. | Medium | No (yet) | Removing this would be a user-visible schema change. Recommend first deciding whether to wire `BackgroundModelResolver` or drop profile-based model selection entirely. |
| `README.md` | `128` “package manager” | user-facing docs | The `package_manager` builtin tool was removed in commit `39cd1cd`; the README still lists it as a built-in tool. | High (doc drift) | Yes | Low-risk wording update. |
| `docs/refactoring/subagent-and-tooling-reform.md` | historical line references throughout | planning doc | Still describes the *pre-refactor* state with old line numbers, removed identifiers (`AgentProfile`, `ApprovalMode`, `READONLY_DENIED_TOOLS`, `WRITE_DENIED_TOOLS`, `memory_extraction`, etc.), and deleted files. | High (doc drift) | Yes | Archive, mark as completed, or rewrite as an ADR. Not source code, but causes confusion. |

## Commands Run and Key Results

### 1. Whole-workspace check

```bash
cargo check --all
```

**Result:** success (exit `0`). Two warnings:

```text
warning: function `extract_shell_path_mentions` is never used
    --> crates/navi-core/src/security.rs:1388:4
warning: function `looks_like_path` is never used
    --> crates/navi-core/src/security.rs:1544:4
```

### 2. Tests check

```bash
cargo check -p navi-core --tests
```

**Result:** success (exit `0`); same two `dead_code` warnings.

### 3. Clippy dead-code lint

```bash
cargo clippy --all -- -W dead_code
```

**Result:** only the two `security.rs` functions flagged. (Clippy also emitted many style/quality warnings unrelated to dead code.) The lint name `clippy::dead_code` does not exist; the correct invocation is `-W dead_code`.

### 4. Targeted tests

| Command | Result |
|---------|--------|
| `cargo test -p navi-core security -- --test-threads=4` | 87 passed, 0 failed |
| `cargo test -p navi-core memory::extract -- --test-threads=4` | 9 passed, 0 failed |
| `cargo test -p navi-core subagent -- --test-threads=4` | 13 passed, 0 failed + 1 parity test passed |
| `cargo test -p navi-core background_model -- --test-threads=4` | 3 passed, 0 failed |
| `cargo check -p navi-tui` | success |
| `cargo check -p navi-sdk -p navi-napi -p navi-lite` | success |

### 5. Grep for removed identifiers

- `package_manager`/`PackageManager` in `crates/`: only `crates/navi-core/src/tool/tests.rs:409-420` (a regression test asserting the tool is absent).
- `AgentProfile`, `ApprovalMode`, `READONLY_DENIED_TOOLS`, `WRITE_DENIED_TOOLS`, `resolve_allowed_tool_names`, `resolve_approval_mode`, `write_scope_from_options`: **0 matches** in `crates/`.
- `cheap_general`, `cheap_code`, `naming`, `long_context_cheap`, `research_synthesis`, `simple_code_edit`, `memory_extraction`, `memoryExtraction`, `MemoryExtractionModel`, `memory_extraction_model`: **0 matches** in `crates/` except an unrelated “naming” comment in `navi-openai/src/providers/openai.rs:274`.
- `BackgroundModelResolver::new`: only called in `background_model.rs` tests; no runtime constructor.
- `with_background_resolver`: only defined in `subagent.rs`; no callers.

## What Was Already Clean

- The `package_manager` module, imports, metadata, CLI/materialize lists, bash redirects, subagent denied-tool lists, registry/tool tests, and config tests are fully removed.
- `browser`, `code`, `code_edit`, `code_exec`, and `subagent` are correctly promoted to `ToolExposure::Direct` in `tool/builtin/metadata.rs` and the harness/materialize inventories.
- `BackgroundModelsConfig` exposes only `default`, `repo_search`, and `subagent_research`; TUI/NAPI/SDK/lite surfaces match.
- `AgentProfile`, `ApprovalMode`, `READONLY_DENIED_TOOLS`, `WRITE_DENIED_TOOLS`, `resolve_allowed_tool_names`, `resolve_approval_mode`, and `write_scope_from_options` are gone from the subagent/workflow code.
- `MemoryExtractionModel`, `memory_extraction_model`, and `memory_extraction` background-model routes are gone from runtime/config/TUI/NAPI/SDK.

## Recommended Deletion Order (high-confidence, low-risk first)

1. **`security.rs` `extract_shell_path_mentions` + `looks_like_path`** — compiler-flagged, private, no callers. Pure deletion.
2. **`subagent.rs` `_prompt_cache` field + constructor parameter** — clearly unused; simplifies `SubagentTool::new` signature. Update all call sites (runtime, SDK, workflow tests/backends, subagent tests).
3. **`subagent.rs` `ProviderBuilderFn`, `with_background_resolver`, `background_resolver`, `provider_builder`** — a single self-contained cluster of unwired subagent infrastructure. Remove from `subagent.rs`, `builtin/mod.rs`, `tool/mod.rs`, and `lib.rs` re-exports.
4. **`memory/extract.rs` `extract_memories` + rewrite its tests** — public but only self-tested. Point tests at `extract_memories_from_messages` to align with the forked-main-session path.
5. **`background_model.rs` module + `lib.rs` export + `subagent.rs` import** — remove once the resolver cluster is deleted. This also deletes the module’s 3 unit tests, which is safe because they only test the dead module.
6. **Conditional: `registry/store.rs` `seed_default_profiles` and `query_models_by_profile`** — remove only if the decision is made to drop profile-based background model selection entirely. If the feature is kept, these must be wired into runtime startup instead.
7. **Documentation hygiene** — update `README.md:128` and archive/rewrite `docs/refactoring/subagent-and-tooling-reform.md`.

## Notes and Caveats

- **No source code was deleted or modified during this audit.** This report is read-only.
- Removing `background_model.rs` is a medium-confidence recommendation because the `profile` field remains in the subagent JSON schema. The product decision is whether to wire profile-based model selection or remove the `profile` parameter from the subagent tool. If `profile` is kept, `BackgroundModelResolver` should be constructed and passed via `with_background_resolver` instead of deleted.
- The `evals/suites/beyond/b4_subagents.toml` verifier (`! rg -q ...`) is **correct and up-to-date**; it asserts the absence of removed identifiers. It is not dead/stale.
- `crates/navi-core/src/tool/tests.rs:409-420` (`removed_tools_are_not_registered`) is a valid regression test asserting removed tools stay unregistered; it is not dead code.

## Resolution

The high and medium-confidence dead-code items above were removed in commit `07b003e` (`fix(core): remove dead code from phase 1-5 audit`).
- `security.rs` `extract_shell_path_mentions` + `looks_like_path` deleted.
- `SubagentTool` `_prompt_cache`, `ProviderBuilderFn`, `with_background_resolver`, `background_resolver`, and `provider_builder` removed; `profile` schema field and invocation parsing removed; `resolve_model` now honors `options.model` as a model-name override.
- `background_model.rs` module and public re-exports deleted.
- `memory::extract::extract_memories` removed; tests updated to call `extract_memories_from_messages`.
- Registry profile dead code (`seed_default_profiles`, `query_models_by_profile`, `upsert_profile`, `Profile`, `RankedModel`, `ModelProfileEntry`) removed; unused `model_profiles`/`profiles` tables dropped from the schema.
- `README.md` no longer lists the removed `package_manager` tool.
