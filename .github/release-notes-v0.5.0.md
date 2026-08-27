## Highlights

**0.5.0** adds glob pattern support to the search tool, ships four production fixes across the bash tool and config layer, and broadens test coverage for critical tool paths. The macOS backend (`navi-os-macos`) was also updated to compile against `core-graphics` 0.24 and the Rust 2024 edition.

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.4.9...v0.5.0

### Search

- **Glob pattern support** in the `search` tool — file-name patterns (e.g. `**/*.rs`, `src/**/{mod,lib}.rs`) are now matched in addition to full-path globs, with a new integration test harness covering edge cases.

### Production fixes

- Bash tool: corrected argument quoting on Windows PowerShell so commands with embedded quotes no longer break.
- Config persistence: `defaults.rs` no longer overwrites user-provided `security` / `approvals` sections on save.
- `code_exec` tool: sandbox snapshot roots are now canonicalized before comparison, preventing false-positive rollback on Windows.
- `computer_use` tool: password-field detection now skips value reads on macOS (matching the Windows behavior).

### macOS backend

- `navi-os-macos` updated for `core-graphics` 0.24 API changes (`CGEventSource` moved to `event_source` module, `ScrollPhase` → `ScrollEventUnit`, `EventField` constants, `CGDisplay::image()`).
- Rust 2024 edition: all `unsafe fn` bodies wrapped in `unsafe { }` blocks (`unsafe_op_in_unsafe_fn` is now an error).
- `accessibility-sys` 0.2: CoreFoundation types now imported from `core-foundation-sys` directly (no longer re-exported).
- `navi-sdk` now depends on `navi-computer-use` behind the `computer-use` feature so the macOS doctor check can call `is_accessibility_trusted_macos()`.

### Test coverage

- New unit + edge-case tests for `memory`, `plan`, `sandbox_tool`, `search_tool`, `subagent`, and `workflow` tools.
- Coverage gate floors raised for critical tool files.

### Bindings

- `@navi-agent/napi` **0.5.0** and platform packages
- `@navi-agent/navi` **0.5.0** CLI packages
- Workspace crate versions bumped to **0.5.0**

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh -s -- --version 0.5.0
```

```bash
npm install -g @navi-agent/navi@0.5.0
npm install @navi-agent/napi@0.5.0
```

## Changelog

- Tag range: https://github.com/navi-ai-org/navi/compare/v0.4.9...v0.5.0
- See [CHANGELOG.md](https://github.com/navi-ai-org/navi/blob/v0.5.0/CHANGELOG.md)
