# 13 — Stream + cancel in StoryChat (Agares)

**What to build:** StoryChat shows assistant token/delta streaming from Navi runtime events and can cancel an in-flight turn.

**Blocked by:** 12 — Story ↔ Navi session mapping

**Repo:** Agares

**Status:** ready-for-agent

## Acceptance criteria

- [ ] IPC or main bridge forwards assistant deltas to the renderer
- [ ] Partial assistant text updates live in the chat UI
- [ ] Cancel control calls `cancelTurn` and stops further deltas
- [ ] Cancelled turn does not leave a corrupted half-message without a defined policy (document: keep partial vs discard)
- [ ] Demo: long reply streams; cancel stops generation
