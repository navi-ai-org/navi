# 17 — Converge local GGUF under Navi (later)

**What to build:** Prefer Navi’s provider stack for local inference (OpenAI-compat now; native llama/other engines when Navi ships them). Demote or dual-path Agares-only `node-llama-cpp` so Agares does not maintain a parallel model runtime long term.

**Blocked by:** 08 — Provider upsert; 12 — Story ↔ Navi session mapping

**Repo:** navi (local engine provider) + Agares (UI/provider selection)

**Status:** ready-for-agent (later)

## Acceptance criteria

- [ ] Documented path for local models via Navi providers in Agares settings
- [ ] Story path can run without Agares `LLMService` node-llama provider when Navi local is configured
- [ ] Migration note for existing Agares GGUF import UX (register path → Navi model selection)
- [ ] Clear fallback if Navi local runtime unavailable
- [ ] Decision ADR or note: single intelligence runtime
