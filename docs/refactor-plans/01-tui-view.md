# Plan: Split TUI View Rendering

**Status: DONE** — Completed 2026-05-30.

## Current Problem

`crates/navi-tui/src/view.rs` is still a large orchestration file. It mixes terminal frame layout, chat viewport rendering, modal rendering, status/header/footer rendering, model/provider rows, and debug/session views. This makes visual changes risky because unrelated UI regions share one file and one import surface.

## Goal

Turn `view.rs` into a small top-level render orchestration hub while preserving existing TUI behavior, keybindings, render cache behavior, and tests.

## Proposed Module Shape

- `crates/navi-tui/src/view.rs`: public hub and frame-level orchestration.
- `crates/navi-tui/src/view/chat.rs`: chat viewport, message rows, selection, scroll integration.
- `crates/navi-tui/src/view/input.rs`: input box, cursor positioning, multiline prompt display.
- `crates/navi-tui/src/view/status.rs`: header, footer, status text, loading indicators.
- `crates/navi-tui/src/view/modals.rs`: modal dispatcher and shared modal shell.
- `crates/navi-tui/src/view/model_picker.rs`: model picker rendering.
- `crates/navi-tui/src/view/provider_settings.rs`: provider/account views.
- `crates/navi-tui/src/view/sessions.rs`: saved session picker.
- `crates/navi-tui/src/view/debug.rs`: debug modal content.

## Execution Steps

1. Read `view.rs`, `render/`, `state.rs`, and TUI tests to identify public helper functions used outside `view.rs`.
2. Create `src/view/` modules and move one UI region at a time.
3. Keep `view.rs` re-exporting internal helpers only when needed by existing tests.
4. Keep all layout constants and visual strings unchanged in the first pass.
5. Run `cargo fmt`, `cargo test -p navi-tui`, and `cargo check` after each major move.

## Risks

- Off-by-one layout changes in terminal rectangles.
- Scroll/selection regressions in chat and picker views.
- Render cache invalidation changes if message fields are accessed differently.

## Guardrails

- Do not change `TuiApp` fields.
- Do not change keybindings or modal state transitions.
- Do not modify theme colors or text labels.
- Do not combine this refactor with design changes.

## Acceptance Criteria

- `view.rs` becomes a hub under roughly 300 lines.
- Each major visual region has its own file.
- `cargo test -p navi-tui` passes.
- `cargo check` passes with no warnings.
