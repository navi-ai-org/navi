# 08 — Provider upsert / OpenAI-compat from host

**What to build:** Hosts can register or update a custom OpenAI-compatible provider (base URL, id, label, key env) and select a model—e.g. Ollama at `http://localhost:11434/v1`—from the app, not only by hand-editing TOML.

**Blocked by:** 02 — Programmatic engine config / data dir

**Repo:** navi

**Status:** done

## Acceptance criteria

- [x] API (NAPI + SDK) to upsert a custom provider entry with `base_url` and credential env/key handling
- [x] Host can `selectModel` / `setModel` for that provider after upsert
- [x] Documented Ollama (or generic OpenAI-compat) example
- [x] Persist choice via existing save targets where appropriate
- [x] Failure modes: unreachable base URL does not crash engine build; status/list still works
- [x] Test with mock or local fake OpenAI-compat endpoint if feasible
