# 05 — Prompt profile for non-code agents

**What to build:** Hosts can run sessions whose system prompt is not the default code-agent prompt—e.g. roleplay assistant or create/scaffolding assistant—via profile, skills pack, or a host-supplied prompt path.

**Blocked by:** 03 — Host tool profiles

**Repo:** navi (prompt builder / skills / profile wiring)

**Status:** done

## Acceptance criteria

- [x] At least one non-code prompt profile or documented skills pack for “assistant / creative” use
- [x] Host can activate it without forking navi-core
- [x] Code-agent default remains when profile is default/`code_agent`
- [x] Session can still attach skills on top of the base profile
- [x] Test or golden assertion shows system prompt differs under non-code profile
- [x] Guide documents how Agares-like hosts should set chat vs create prompts
