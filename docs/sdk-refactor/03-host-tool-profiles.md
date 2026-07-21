# 03 — Host tool profiles

**What to build:** Hosts can select a runtime tool profile so default code-agent tools (filesystem shell/edit against the project) can be disabled and only host-registered tools (and optionally a minimal set) remain visible to the model.

**Blocked by:** 02 — Programmatic engine config / data dir on builder

**Repo:** navi (builder + harness / tool registration)

**Status:** done

## Acceptance criteria

- [x] Documented profiles exist (at least: `code_agent` default, `host_tools_only`, `chat_only` / no-tools)
- [x] NAPI builder can select a profile (and/or explicit allow/deny tool name lists)
- [x] Under `host_tools_only`, built-in project bash/edit-style tools are not offered; host tools still work
- [x] Under `chat_only`, the model cannot call tools
- [x] Profile choice is visible in loaded tooling state or tests
- [x] Guide section: “Embedding without code-agent tools”
