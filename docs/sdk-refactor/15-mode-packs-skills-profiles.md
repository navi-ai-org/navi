# 15 — Mode packs as skills/profiles (Agares)

**What to build:** Chat, create, and narrate modes switch Navi prompt + tool allowlists (skills and/or profiles) without exposing code-agent tools.

**Blocked by:** 05 — Prompt profile; 12 — Story session mapping; 14 — CreateAssistant on Navi

**Repo:** Agares (+ navi skills content if needed)

**Status:** ready-for-agent

## Acceptance criteria

- [ ] Each product Mode maps to a Navi skill set and/or profile
- [ ] Switching mode updates session skills or starts a session with the right pack
- [ ] Chat mode tool surface ⊆ read + message append (as designed); create mode includes scaffold writes
- [ ] No mode enables project bash/edit by default
- [ ] Demo: same story, switch mode, observe different system behavior / tools in events
