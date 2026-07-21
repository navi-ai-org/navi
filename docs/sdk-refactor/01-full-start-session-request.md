# 01 — Full startSession request in NAPI

**What to build:** TypeScript hosts can start a Navi session with the full `NaviSessionRequest` surface: fixed session id, project dir, context packets, active skills, initial messages, initial events, timestamps, and optional initial goal—not only `sessionId` + `projectDir`.

**Blocked by:** None — can start immediately.

**Repo:** navi (`navi-sdk` types stay source of truth; `navi-napi` exposes them)

**Status:** done

## Acceptance criteria

- [x] NAPI `startSession` accepts a structured options object (or equivalent) covering every field of Rust `NaviSessionRequest`
- [x] Omitting optional fields preserves current default behavior
- [x] Initial messages seed provider history so the first `sendTurn` continues that conversation
- [x] Initial events / goal are applied when provided
- [x] Types in `index.d.ts` document the request shape (camelCase)
- [x] Unit or integration test covers seed-with-messages path
- [x] Parity docs / guide mention the richer start API
