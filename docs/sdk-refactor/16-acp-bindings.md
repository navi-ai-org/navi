# 16 — ACP bindings (optional)

**What to build:** Expose ACP multi-agent peer APIs on NAPI: list ACP agents and delegate a turn (and simple variant), matching Rust `list_acp_agents` / `delegate_acp_turn`.

**Blocked by:** None — can start immediately (not required for Agares MVP).

**Repo:** navi

**Status:** done

## Acceptance criteria

- [x] Methods added to engine API parity lists and NAPI
- [x] Types documented in `index.d.ts`
- [x] Smoke test or documented manual test with a mock/local ACP peer if available
- [x] Marked optional in sdk-refactor index for product hosts that do not need multi-agent yet
