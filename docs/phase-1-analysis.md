# Phase 1 Analysis: Remove `package_manager` Tool

## Summary of Phase 1 Requirements

Per `docs/refactoring/subagent-and-tooling-reform.md` §2, Phase 1 removes the `package_manager` builtin tool entirely because it duplicates `bash` for dependency operations. The required changes are:

1. Delete `crates/navi-core/src/tool/builtin/package_manager/` and its five files.
2. Remove `mod package_manager` and `PackageManagerTool` re-export from `crates/navi-core/src/tool/builtin/mod.rs`.
3. Remove `PackageManagerTool` import and registration from `crates/navi-core/src/tool/mod.rs`.
4. Remove the `package_manager` metadata entry from `crates/navi-core/src/tool/builtin/metadata.rs`.
5. Remove `"package_manager"` from `crates/navi-cli/src/harness_cmd.rs` deferred list.
6. Remove `"package_manager"` from `crates/navi-core/src/harness_pack/materialize.rs` deferred list.
7. Remove all `package_manager` suggestion redirects and helper functions from `crates/navi-core/src/tool/builtin/bash.rs`.
8. Remove `"package_manager"` from `READONLY_DENIED_TOOLS` / `WRITE_DENIED_TOOLS` in `crates/navi-core/src/tool/builtin/subagent.rs`.
9. Remove `package_manager` references from `crates/navi-core/src/tool/registry.rs` tests.
10. Remove `package_manager`-specific tests and references from `crates/navi-core/src/tool/tests.rs`.
11. Replace `package_manager` in `crates/navi-core/src/config.rs` test `deny_tools` example.

## Current Implementation Status

**Status: Complete.**

| Required Change | Current State | Evidence |
|-----------------|---------------|----------|
| Delete `package_manager` directory | Done | `find_file_by_name` for `crates/navi-core/src/tool/builtin/package_manager/**` returned no files. |
| Remove from `builtin/mod.rs` | Done | `crates/navi-core/src/tool/builtin/mod.rs:1-27` lists `mod bash`, `mod browser`, etc., but no `mod package_manager`; no `PackageManagerTool` re-export. |
| Remove from `tool/mod.rs` | Done | `crates/navi-core/src/tool/mod.rs:26-32` import block has no `PackageManagerTool`; `register_builtin_tools()` (`tool/mod.rs:1105-1181`) registers `ReadTool`, `BashTool`, `CodeExecTool`, etc., but not `PackageManagerTool`. |
| Remove from `metadata.rs` | Done | `grep -i package` in `crates/navi-core/src/tool/builtin/metadata.rs` returned 0 matches. |
| Remove from `harness_cmd.rs` | Done | `crates/navi-cli/src/harness_cmd.rs:40-48` deferred array contains `repo_explore`, `workflow`, `ast_search`, `symbol_goto`, `view_image`, `apply_patch`, `sandbox`; no `package_manager`. |
| Remove from `materialize.rs` | Done | `crates/navi-core/src/harness_pack/materialize.rs:179-222` `default_capability_inventory()` direct and deferred arrays have no `package_manager`. |
| Update `bash.rs` | Done | `grep -i package` in `crates/navi-core/src/tool/builtin/bash.rs` returned 0 matches; package commands will now execute via `bash` directly. |
| Update `subagent.rs` | Done | `grep` for `READONLY_DENIED_TOOLS`, `WRITE_DENIED_TOOLS`, and `package_manager` in `crates/navi-core/src/tool/builtin/subagent.rs` returned 0 matches. |
| Update `registry.rs` tests | Done | `crates/navi-core/src/tool/registry.rs:367-385` `registry_never_auto_promotes_deferred_tools` uses `repo_explore`, `ast_search`, `symbol_goto`; no `package_manager`. |
| Update `tool/tests.rs` | Done | `grep` for `package_manager`/`PackageManager`/`package-manager` in `crates/navi-core/src/tool/tests.rs` returned 0 matches. Test `removed_tools_are_not_registered` (`tool/tests.rs:400-416`) checks `top_files`/`tool_workflow`; package removal is implicit. |
| Update `config.rs` tests | Done | `grep` for `package_manager` in `crates/navi-core/src/config.rs` returned 0 matches. |

## Verification

### Grep Results

- `crates/`: **0 matches** for `package_manager|PackageManager|package-manager`.
- `docs/workflow-tool-lua-spec.md`: **0 matches**.
- `crates/navi-core/tests/parity_check.rs`: **0 matches**.
- Whole repo (`/home/enrell/projects/navi`): only matches are in `docs/refactoring/subagent-and-tooling-reform.md` (the plan document itself) and one unrelated GitHub Actions option `package-manager-cache` in `.github/workflows/publish-npm.yml:104`.

### Build and Tests

```bash
cargo check -p navi-core
```
- **Result:** `Finished dev profile [unoptimized + debuginfo] target(s) in 18.12s`, exit code 0.
- **Warnings:** two unrelated `dead_code` warnings in `crates/navi-core/src/security.rs` (`extract_shell_path_mentions` at 1388, `looks_like_path` at 1544). No `package_manager` or compile errors.

```bash
cargo test -p navi-core -- --test-threads=4
```
- **Result:** exit code 0.
- **Unit tests:** 1031 passed, 0 failed.
- **Integration tests (`tests/parity_check.rs`):** 21 passed, 0 failed.
- **Doc-tests:** 0 run.

## Code Quality Observations

1. **No source-code remnants.** The module, metadata, registration, CLI/materialize lists, bash redirects, subagent denied-tool lists, registry/tool tests, and config tests have all been cleaned up.
2. **`bash.rs` now executes package commands directly.** The previous npm/bun/cargo/go detection and `NativeSuggestion` redirects to `package_manager` are gone, satisfying the requirement to let `bash` handle dependency operations.
3. **Phase 2 changes appear already applied.** `metadata.rs` already exposes `browser`, `code`, `code_edit`, `code_exec`, and `subagent` as `ToolExposure::Direct`; `harness_cmd.rs` and `materialize.rs` list those tools in the `direct` array. This is outside Phase 1 scope but shows the code has moved past the old deferred layout.
4. **Stale plan document.** `docs/refactoring/subagent-and-tooling-reform.md` §2 still references historical line numbers and file paths for `package_manager`. This is documentation drift, not source drift, but should be noted if the doc is meant to stay authoritative.

## Gaps and Recommended Next Steps

- **No Phase 1 source gaps.** The tool is fully removed and the codebase compiles and tests pass.
- **Minor test-coverage recommendation:** Consider adding `package_manager` to the `removed_tools_are_not_registered` test (`crates/navi-core/src/tool/tests.rs:400-416`) to guard against accidental re-registration. No source change was made for this analysis.
- **Documentation hygiene:** If `docs/refactoring/subagent-and-tooling-reform.md` is a living plan, update §2 to indicate Phase 1 is done or archive the historical line-number references.
- **Proceed to Phase 2.** The next phase (Tool Exposure: Deferred → Direct) is already largely reflected in the current source, but should be formally verified against §1 of the reform doc.

No source code was modified for this analysis, and no commits were made.
