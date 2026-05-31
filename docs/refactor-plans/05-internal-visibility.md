# Plan: Tighten Internal Visibility And Reexports

**Status: DONE** — Completed 2026-05-30.

## Current Problem

Several modules now use hubs and `pub(crate)` reexports to preserve compatibility after large mechanical splits. This was the right migration step, but some exported helpers are only consumed by tests or sibling modules. Wide visibility makes future coupling easier.

## Goal

Reduce internal API surface after the module splits have stabilized.

## Target Areas

- `crates/navi-tui/src/render.rs`: reexports from `render/*`.
- `crates/navi-tui/src/keybindings.rs`: test-only reexports and shared helpers.
- `crates/navi-core/src/config.rs`: broad `pub use` hub exports.
- `crates/navi-core/src/tool/builtin/mod.rs`: tool and helper visibility.
- `crates/navi-sdk/src/lib.rs`: facade exports that may be convenience-only.

## Execution Steps

1. Run targeted searches for each reexported item.
2. Classify each item as external public API, crate-internal API, sibling-only helper, or test-only helper.
3. Move test-only imports into test modules where possible.
4. Replace broad glob reexports with explicit reexports where useful.
5. Change `pub(crate)` to `pub(super)` or private where possible.
6. Run `cargo test` after each crate-level tightening.

## Risks

- Rust visibility churn can create noisy diffs with little behavioral value.
- Tests may rely on old hub paths for convenience.
- Over-tightening can make nearby modules awkward to maintain.

## Guardrails

- Do not change public API exposed by `navi-core` or `navi-sdk` unless explicitly planned.
- Prefer clarity over minimizing every single `pub(crate)`.
- Keep test readability high.

## Acceptance Criteria

- No `#[allow(unused_imports)]` is needed solely for test-only hub reexports unless justified.
- Private helpers are not exported through hubs.
- `cargo check` has no warnings.
- `cargo test` passes.
