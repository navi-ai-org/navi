# 07 — Snapshot reopen helper in NAPI

**What to build:** TypeScript hosts can reopen a saved session snapshot with full history (and attachment rehydration rules) without hand-rolling `initialMessages` / `initialEvents`.

**Blocked by:** 01 — Full startSession request in NAPI

**Repo:** navi

**Status:** done

## Acceptance criteria

- [x] NAPI exposes a helper equivalent to `session_request_from_snapshot` (or `startSessionFromSnapshot`)
- [x] Reopened session continues coherently after `sendTurn`
- [x] Attachment rehydration rules documented (project path vs data_dir attachments)
- [x] Works with host-injected data dir from ticket 02 when available
- [x] Test covers reopen from snapshot JSON
