# NAVI Plugin System Verification Plan

## Purpose
This document defines when a milestone is considered complete.
No milestone can be marked done until all verification criteria pass.

## Per-Milestone Verification

For EACH milestone:

1. Code compiles without warnings
2. `cargo fmt --check` passes
3. `cargo clippy --workspace --all-targets -- -D warnings` passes
4. Unit tests pass
5. Integration tests pass
6. Security tests for that milestone pass
7. Traceability matrix is updated
8. No TODO security bypass remains
9. Relevant docs are updated

## Verification Commands

```bash
# Format check
cargo fmt --check

# Lint check
cargo clippy --workspace --all-targets -- -D warnings

# All tests
CARGO_TEST_THREADS=4 cargo test --workspace

# Per-crate tests
cargo test -p navi-plugin-manifest
cargo test -p navi-plugin-security
cargo test -p navi-plugin-broker
cargo test -p navi-plugin-runtime
cargo test -p navi-plugin-registry

# Red-team tests
cargo test --test plugin_redteam
```

## Security Gate Verification

Before ANY release, verify:

1. No community plugin can run native in-process
2. No plugin can read env directly
3. No plugin can access network without HTTP broker
4. No plugin can access filesystem without FS broker
5. No plugin can register free-form tool description
6. No plugin can shadow built-in tool
7. No plugin can bypass reconsent on new capabilities
8. All red-team tests pass
9. All security defaults are enforced
10. Audit log is functional

## Completion Format

When reporting milestone completion:

```
Milestone: M{N} — {name}
Requirements implemented: REQ-{list}
Files changed: {list}
Tests added: {count} tests
Verification: all commands pass
Traceability updated: {count} rows
Remaining blockers: {list or none}
```
