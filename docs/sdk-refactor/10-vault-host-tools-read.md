# 10 — Vault host tools (read path) (Agares)

**What to build:** Navi host tools that read Agares vault domain data (worlds, characters, stories, recent messages) so the agent can ground answers without shell or project filesystem tools.

**Blocked by:** 03 — Host tool profiles; 09 — Vault-scoped Navi engine lifecycle

**Repo:** Agares

**Status:** ready-for-agent

## Acceptance criteria

- [ ] Host tools registered for list/get worlds, characters, stories, and list recent messages
- [ ] Tools use existing vault services / encrypted DB only
- [ ] Engine runs under host_tools_only (or equivalent)—no bash/edit against disk
- [ ] Tool outputs are JSON-serializable and size-bounded for harness observations
- [ ] Demo: agent turn can name a world that exists only in the vault
