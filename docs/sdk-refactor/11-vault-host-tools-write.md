# 11 — Vault host tools (write path) (Agares)

**What to build:** Navi host tools that create/update worlds, characters, stories and append messages, with write-kind security and approval behavior appropriate for a vault app.

**Blocked by:** 04 — Security / permission profile; 10 — Vault host tools (read)

**Repo:** Agares

**Status:** ready-for-agent

## Acceptance criteria

- [ ] Write tools for create/update world, character, story; append message
- [ ] Tools marked write (or equivalent) and respect approval policy
- [ ] Successful tool calls persist in SQLCipher vault and are visible in Agares UI after refresh
- [ ] Denied approval does not partially corrupt domain rows
- [ ] Demo: Create flow or agent turn creates a character that appears in Library
