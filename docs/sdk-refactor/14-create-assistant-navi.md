# 14 — CreateAssistant on Navi (Agares)

**What to build:** CreateAssistant uses Navi multi-step turns plus vault write host tools to scaffold worlds/characters/stories instead of a single bare `llm.generate` call.

**Blocked by:** 05 — Prompt profile for non-code agents; 09 — Vault-scoped lifecycle; 11 — Vault host tools (write)

**Repo:** Agares

**Status:** ready-for-agent

## Acceptance criteria

- [ ] Create flow starts a Navi session with create-oriented prompt/profile
- [ ] Agent can call write host tools to materialize entities
- [ ] User sees progress (tool calls and/or assistant text) in Create UI
- [ ] Resulting entities appear in Library and can open a Story
- [ ] Fallback or clear error if Navi engine unavailable (optional thin LLM path)
- [ ] Demo: “create a cyberpunk world with one NPC” yields vault rows
