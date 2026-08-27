# 09 — Vault-scoped Navi engine lifecycle (Agares)

**What to build:** Agares main process owns a Navi engine whose data dir lives under the vault/app data directory; engine is created after vault unlock and fully disposed on vault lock and app quit—not just `navi:status` export probing.

**Blocked by:** 02 — Programmatic engine config; 08 — Provider upsert / OpenAI-compat

**Repo:** Agares

**Status:** ready-for-agent

## Acceptance criteria

- [ ] Engine data dir is under `AGARES_DATA_DIR` (or vault data), not the monorepo root
- [ ] Unlock path can construct engine; lock/quit disposes engine and clears sessions
- [ ] IPC status reports ready/error/model summary, not only native export names
- [ ] Failure to load native module is a soft error in UI, not main-process crash
- [ ] Ollama OpenAI-compat (or configured provider) selectable after engine start
- [ ] No Navi files written outside the chosen data dir for default operations
