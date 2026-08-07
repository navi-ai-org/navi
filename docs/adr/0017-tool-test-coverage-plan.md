# ADR 0017 — Tool Test Coverage Plan

## Status

Accepted (all phases complete)

## Context

ADR 0016 introduced the computer-use tool surface (`inspect_desktop`,
`inspect_element`, `simulate_input`, `open_application`,
`annotate_screenshot`, `capture_screen`). The initial implementation
shipped with integration and edge-case tests for the new tools, but a
coverage audit (`cargo-llvm-cov` + `scripts/check-critical-coverage.py`)
revealed significant gaps:

| File | Coverage | Lines uncovered | Severity |
|------|----------|-----------------|----------|
| `navi-os-windows/src/input.rs` | **0.0%** | 212/218 | 🔴 Critical |
| `navi-os-windows/src/capture.rs` | **32.4%** | 96/145 | 🟡 High |
| `navi-core/src/tool/builtin/computer_use.rs` | **89.2%** | 111/1054 | 🟡 Medium |
| `navi-os-windows/src/inspect.rs` | **77.4%** | 110/491 | 🟡 Medium |
| `navi-os-windows/src/open_app.rs` | **89.1%** | 20/183 | 🟢 Low |
| `navi-os-windows/src/target.rs` | **85.5%** | 9/62 | 🟢 Low |
| `navi-os-windows/src/com.rs` | **61.9%** | 8/21 | 🟢 Low |

The `AGENTS.md` "Tool testing requirements" section (added alongside
this ADR) defines a three-layer test policy (unit, edge case,
integration) and a coverage gate via `scripts/check-critical-coverage.py`.
This ADR defines the execution plan to bring all tool files to compliance.

## Decision

### 1. Phased rollout by feature area

Tests are added one feature area at a time, highest-impact first. Each
phase is complete only when:

- All three layers (unit, edge case, integration) are present
- `cargo-llvm-cov` coverage meets the target floor for every file in the phase
- `scripts/check-critical-coverage.py` passes
- `cargo test -p <crate> -- --test-threads=4` passes

### 2. Phase ordering

| Phase | Feature area | Files | Target coverage | Status |
|-------|-------------|-------|-----------------|--------|
| 1 | Computer Use | `input.rs`, `capture.rs`, `computer_use.rs`, `inspect.rs`, `open_app.rs` | 80%+ per file | **Complete** |
| 2 | Workflow / Lua | `workflow/backends.rs`, `plan.rs` | 60%+ per file | **Complete** |
| 3 | Subagent | `subagent.rs` | 40%+ | **Complete** |
| 4 | Update / installer | `update.rs` | 40%+ | **Complete** |
| 5 | Remaining tools | all other `builtin/*.rs` | 70%+ per file | **Complete** |

### 3. Phase 1 — Computer Use (active)

#### 3.1 `input.rs` (0% → 80%+)

**Why 0%:** `simulate_input` calls `SendInput` (Win32 FFI) which requires
a desktop session. No unit tests exist for the parsing/helper functions.

**Strategy:** Extract testable logic from FFI calls. The parsing layer
(`get_i32`, `get_str`, `key_to_vk`, action dispatch) is pure and can be
tested without `SendInput`. The FFI layer (`do_mouse_move`, `do_click`,
`do_key_press`, `send_inputs`) is integration-tested with skip-on-failure.

**Tests to add:**

- **Unit (Layer 1):**
  - `key_to_vk` maps all named keys (Enter, Tab, Escape, Shift, F1-F12, etc.)
  - `key_to_vk` maps single characters (a→VK_A, z→VK_Z)
  - `key_to_vk` is case-insensitive ("enter" == "ENTER")
  - `key_to_vk` unknown key defaults to uppercase ASCII of first char
  - `key_to_vk` empty string defaults to VK_SPACE
  - `get_i32` extracts valid integer
  - `get_i32` errors on missing field
  - `get_i32` errors on non-integer value
  - `get_str` extracts valid non-empty string
  - `get_str` errors on missing field
  - `get_str` errors on empty string
  - `build_mouse_input` sets correct type and flags
  - `encode_bmp` (already in capture.rs, but input has no equivalent)

- **Edge case (Layer 2):**
  - `simulate_input` with empty actions vec → `actions_performed: 0`
  - `simulate_input` with action missing `action` field → error with index
  - `simulate_input` with unknown action type → error with action name
  - `simulate_input` with `click` missing `x` → error
  - `simulate_input` with `click` missing `y` → error
  - `simulate_input` with `click` missing `button` → defaults to "left"
  - `simulate_input` with `key` missing `key` → error
  - `simulate_input` with `type` missing `text` → error
  - `simulate_input` with `type` empty text → error
  - `simulate_input` with negative coordinates (multi-monitor)
  - `simulate_input` with very large coordinates (4K+)
  - `simulate_input` with mixed valid + invalid actions → fails at invalid
  - `simulate_input` with unicode text (emoji, surrogate pairs)
  - `simulate_input` with `scroll` delta=0
  - `simulate_input` with `scroll` negative delta (scroll up)
  - `simulate_input` with `scroll` large delta

- **Integration (Layer 3, `#[cfg(windows)]` + skip-on-failure):**
  - `simulate_input` with `mouse_move` to center of screen
  - `simulate_input` with `click` left button
  - `simulate_input` with `key` Enter
  - `simulate_input` with `type` short text

#### 3.2 `capture.rs` (32% → 60%+)

**Why 32%:** `capture_screen` is almost entirely GDI FFI. Only
`encode_bmp` has unit tests.

**Strategy:** The FFI calls (`GetDC`, `BitBlt`, `GetDIBits`) require a
desktop. But error paths and `encode_bmp` are testable without one.

**Tests to add:**

- **Unit (Layer 1):**
  - `encode_bmp` with 1x1 pixel
  - `encode_bmp` with 0x0 (edge case — should handle gracefully or panic with clear message)
  - `encode_bmp` with large dimensions (4K)
  - `encode_bmp` pixel data is bottom-up (row order reversed)
  - `encode_bmp` header fields all correct (already partially tested)

- **Edge case (Layer 2):**
  - `capture_screen` with invalid `out_dir` (e.g. path with null bytes) → error
  - `capture_screen` with read-only `out_dir` → error
  - `capture_screen` with very long path → error

- **Integration (Layer 3, skip-on-failure):**
  - `capture_screen` to temp dir → returns valid screenshot with correct dimensions
  - Screenshot file is readable and has correct BMP header

#### 3.3 `computer_use.rs` (89% → 95%+)

**Why 89%:** The 111 uncovered lines are mostly:
- `annotate_screenshot` partial-failure path (inspect fails → screenshot returned without tree)
- `SimulateInputTool::invoke` deny-list path
- `SimulateInputTool::invoke` backend error path
- `OpenApplicationTool::invoke` backend error path
- Some test helper functions not exercised by the `computer_use` filter

**Tests to add:**

- **Unit (Layer 1):**
  - `annotate_screenshot` with `supports_vision=false` → returns text-only result
  - `collect_element_rects` populates cache correctly
  - `count_elements` / `count_passwords` on complex trees

- **Edge case (Layer 2):**
  - `annotate_screenshot` inspect failure → `inspect_error` field set, message explains partial result
  - `SimulateInputTool` deny-list triggers → `denied: true`, `deny_reason` set
  - `SimulateInputTool` with sensitive field guard → blocked with explanation
  - `OpenApplicationTool` with missing `name` field → error
  - `OpenApplicationTool` with empty `name` → error

- **Integration (Layer 3, skip-on-failure):**
  - `inspect_desktop` → returns windows with element_ids
  - `open_application("notepad")` → launches successfully
  - `annotate_screenshot` → returns annotated image (vision mode)

#### 3.4 `inspect.rs` (77% → 85%+)

**Why 77%:** The 110 uncovered lines are mostly:
- `find_element_recursive` walker error branches (only trigger when UIA fails)
- `walk_tree` error branches for elements without children
- Integration tests that skip when no desktop

**Tests to add:**

- **Unit (Layer 1):**
  - `walk_tree` with `max_depth=0` → returns root only, no children
  - `walk_tree` with `max_depth=1` → root + direct children only
  - `walk_tree` truncates at `MAX_CHILDREN_PER_NODE`

- **Edge case (Layer 2):**
  - `inspect_element` with `element_id` format "w0.e" (missing counter) → error
  - `inspect_element` with `element_id` format "w.e0" (missing window number) → error
  - `inspect_element` with `element_id` for non-existent element → error with recovery hint
  - `inspect_element` with `max_depth=0` + `element_id` → returns element only

- **Integration (Layer 3, skip-on-failure):**
  - `inspect_desktop` returns at least 1 window on a desktop session
  - `inspect_element` with foreground window → returns element tree

#### 3.5 `open_app.rs` (89% → 95%+)

**Why 89%:** The 20 uncovered lines are:
- `shellexecute_error_meaning` for less common codes (8, 11, 27, 28, 29, 30, 32)
- `recovery_hint` for less common codes
- Integration tests that skip

**Tests to add:**

- **Unit (Layer 1):**
  - `shellexecute_error_meaning` for all 13 known codes (0-32)
  - `recovery_hint` for all common codes (2, 3, 5, 26, 31)

- **Edge case (Layer 2):**
  - `recovery_hint` for unknown code (99) → non-empty string
  - `recovery_hint` for code 5 (access denied) → mentions elevation

### 4. Coverage floor updates

After each phase, update `scripts/check-critical-coverage.py` with the
new (higher) floors. Floors ratchet up — never lowered without
justification.

| File | Current floor | Target floor (post-phase) |
|------|--------------|---------------------------|
| `computer_use.rs` | 40% | 90% |
| `inspect.rs` | 50% | 80% |
| `open_app.rs` | 70% | 90% |
| `input.rs` | — (not listed) | 75% |
| `capture.rs` | — (not listed) | 55% |

### 5. Critical function gate updates

After Phase 1, these functions must be hit by at least one test:

- `simulate_input` (input.rs)
- `key_to_vk` (input.rs)
- `capture_screen` (capture.rs) — integration only, skip-on-failure
- `encode_bmp` (capture.rs)
- `collect_element_rects` (computer_use.rs)
- `count_elements` / `count_passwords` (computer_use.rs)

## Consequences

- **Positive:** Every tool file has deterministic unit + edge case tests
  that pass in CI without a desktop. Integration tests verify real
  backend behavior and skip gracefully when headless.
- **Positive:** Coverage gate prevents regressions — removing tests from
  a tool file will fail CI.
- **Negative:** `input.rs` and `capture.rs` FFI functions can only be
  integration-tested (skip-on-failure). Unit coverage will never reach
  100% for these files. The target floors (75%, 55%) reflect this.
- **Negative:** Floors must be maintained. If code is refactored and
  coverage drops, floors may need adjustment — but only with
  justification, never silently.
