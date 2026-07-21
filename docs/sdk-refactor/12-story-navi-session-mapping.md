# 12 — Story ↔ Navi session mapping (Agares)

**What to build:** Opening a Story starts or resumes a Navi session seeded with world lore, character sheet, and recent vault messages; user sends map to `sendTurn` and assistant text is stored as vault messages.

**Blocked by:** 01 — Full startSession request; 09 — Vault-scoped lifecycle; 10 — Vault host tools (read)

**Repo:** Agares

**Status:** ready-for-agent

## Acceptance criteria

- [ ] Stable mapping between story id and Navi session id (or explicit map table)
- [ ] Session start seeds context and/or initial messages from vault
- [ ] User message path: vault persist + Navi turn + assistant vault persist
- [ ] Re-opening the same story resumes the same Navi session when still alive, or reseeds correctly after lock
- [ ] Works with host_tools_only profile
- [ ] Demo: multi-turn roleplay reply appears in StoryChat and survives app restart (vault), session reseed after unlock
