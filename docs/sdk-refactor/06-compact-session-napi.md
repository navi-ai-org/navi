# 06 — compactSession on engine API + NAPI

**What to build:** Manual session compaction is part of the public engine API list and callable from TypeScript, matching Rust `compact_session`.

**Blocked by:** None — can start immediately.

**Repo:** navi

**Status:** done

## Acceptance criteria

- [x] `compact_session` added to `NAVI_ENGINE_API_METHODS` and `NAVI_NAPI_BOUND_METHODS`
- [x] NAPI method `compactSession(sessionId)` returns a structured outcome (or JSON equivalent)
- [x] Parity test still passes
- [x] Behavior matches Rust force-compact path (reduces history under load)
- [x] Documented in napi guide under session management
