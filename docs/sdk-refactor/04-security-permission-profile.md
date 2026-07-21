# 04 — Security / permission profile from host

**What to build:** Hosts can set permission mode and a non-code security posture when building the engine (approvals for writes, no accidental permissive default), so vault-style apps do not inherit code-agent trust assumptions.

**Blocked by:** 02 — Programmatic engine config / data dir; 03 — Host tool profiles

**Repo:** navi

**Status:** done

## Acceptance criteria

- [x] Host can set permission mode at build or session level consistently with existing `get/setPermissionMode`
- [x] A documented “host app” security profile exists (or config knobs) for write approvals without requiring TUI
- [x] Permissive security remains opt-in, never default for new host profiles
- [x] Approval events still surface for gated host tools when policy requires them
- [x] NAPI can resolve approvals as today; profile docs show the flow
- [x] Tests cover deny/allow for a write-kind host tool under the host profile
