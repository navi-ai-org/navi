# NAVI Architecture & Technical Debt Report
## 2026-05-29

---

## HIGH severity

| # | Issue | Location | Status |
|---|-------|----------|--------|
| 1 | `navi-core` public API has almost no doc comments | All `navi-core/src/*.rs` | PENDING |
| 2 | `unsafe` blocks lack `// SAFETY:` comments | `navi-plugin-host/src/lib.rs`, `navi-plugin-api/src/lib.rs` | DONE |
| 3 | Blocking `std::process::Command` in async `patch.rs` | `navi-core/src/tool/builtin/patch.rs` | DONE |
| 4 | `HeaderValue::from_str().unwrap()` in all 10 providers | `navi-openai/src/providers/behavior.rs` | DONE |

## MEDIUM severity

| # | Issue | Location | Status |
|---|-------|----------|--------|
| 5 | Duplicate `ProviderId` enum in navi-core and navi-openai | `provider_id.rs` + `types.rs` | DONE |
| 6 | `navi-cli/acp.rs` imports from navi-core when SDK re-exports exist | `navi-cli/src/acp.rs` | DONE |
| 7 | `navi-sdk` public API returns `anyhow::Result` everywhere | `navi-sdk/src/engine.rs` | PENDING |
| 8 | No `RwLock` for read-heavy patterns (`loaded_config`, `sessions`) | `navi-sdk/src/engine.rs` | DONE |
| 9 | `BashBackgroundState.label` as `&'static str` instead of enum | `navi-core/src/tool/builtin/bash.rs` | DONE |
| 10 | String-based provider ID comparisons across 4 files | `credentials.rs`, `runtime.rs`, `mapping.rs` | DONE |
| 11 | `#[allow(dead_code/unused)]` blanket suppressions in navi-tui | `render.rs`, `keybindings.rs` | DONE |
| 12 | Duplicated approval resolution logic | `runtime/mod.rs` + `turn/mod.rs` | DONE |
| 13 | `navi-openai/src/lib.rs` ~800 lines of tests mixed with 89 lines of code | `navi-openai/src/lib.rs` | PENDING |
| 14 | Functions >100 lines (5 functions) | `modals.rs`, `bash.rs`, `engine.rs` | PENDING |
| 15 | Blocking `std::fs` in `SessionStore` methods | `navi-core/src/session.rs` | WONTFIX |
| 16 | Blocking `std::fs::read_to_string` in `ensure_system_prompt` | `navi-core/src/turn/mod.rs` | DONE |

## LOW severity

| # | Issue | Location | Status |
|---|-------|----------|--------|
| 17 | SDK re-exports provider-specific `github_copilot_device_oauth` | `navi-sdk/src/lib.rs` | DONE |
| 18 | `OpenAiProvider` missing `Debug` derive | `navi-openai/src/provider.rs` | DONE |
| 19 | `LoadedConfig` cloned on every engine operation | `navi-sdk/src/engine.rs` | PENDING |
| 20 | `SessionId(pub String)` — public inner field defeats newtype | `navi-core/src/session.rs` | DONE |
| 21 | Missing test coverage (oauth, turn, tooling) | Various | PENDING |
| 22 | `navi-tui` has non-workspace dependencies | `navi-tui/Cargo.toml` | DONE |

---

### Notes

- **#15 WONTFIX**: SessionStore methods use sync `std::fs` but operate on small local JSON files. The overhead of `spawn_blocking` is not justified for these fast operations. The `ensure_system_prompt` fix (#16) was higher priority since it reads on every turn.

---

## Friction-free fixes (can be done without architectural changes)

1. `HeaderValue::from_str().unwrap()` → return `Result` in `behavior.rs`
2. `// SAFETY:` comments on 4 unsafe blocks
3. Remove duplicate `ProviderId` in navi-openai, use navi-core's
4. `navi-cli/acp.rs` imports → route through navi-sdk
5. `BashBackgroundState.label` → enum
6. String provider comparisons → `ProviderId` enum
7. `OpenAiProvider` add `Debug` derive
8. `SessionId` make inner field private

## Higher-effort fixes (need planning)

- navi-sdk typed error enum (replacing `anyhow::Result`)
- Blocking I/O → async in `session.rs`, `turn/mod.rs` (patch.rs DONE)
- `RwLock` for read-heavy engine state
- navi-core doc comments (large surface area)
- Large file splits
