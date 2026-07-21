# Navi SDK refactor — ticket index

Tracer-bullet tickets for making **navi-sdk + `@navi-agent/napi`** a fully host-customizable agent framework, then embedding it in **Agares**.

**Context:** gap analysis (Rust SDK vs NAPI) and Agares vault product needs.  
**Parity baseline:** `NAVI_ENGINE_API_METHODS` and `NAVI_NAPI_BOUND_METHODS` are already aligned (123 methods). Gaps are construction, session seed fidelity, tool/prompt profiles, and product wiring.

## Dependency order

| # | Ticket | Repo focus | Blocked by |
|---|--------|------------|------------|
| 01 | [Full startSession request in NAPI](01-full-start-session-request.md) | navi | — |
| 02 | [Programmatic engine config / data dir](02-programmatic-engine-config.md) | navi | — |
| 03 | [Host tool profiles](03-host-tool-profiles.md) | navi | 02 |
| 04 | [Security / permission profile from host](04-security-permission-profile.md) | navi | 02, 03 |
| 05 | [Prompt profile for non-code agents](05-prompt-profile-non-code.md) | navi | 03 |
| 06 | [compactSession on API + NAPI](06-compact-session-napi.md) | navi | — |
| 07 | [Snapshot reopen helper in NAPI](07-snapshot-reopen-helper.md) | navi | 01 |
| 08 | [Provider upsert / OpenAI-compat from host](08-provider-upsert-openai-compat.md) | navi | 02 |
| 09 | [Vault-scoped Navi engine lifecycle](09-vault-scoped-navi-lifecycle.md) | Agares | 02, 08 |
| 10 | [Vault host tools (read)](10-vault-host-tools-read.md) | Agares | 03, 09 |
| 11 | [Vault host tools (write)](11-vault-host-tools-write.md) | Agares | 04, 10 |
| 12 | [Story ↔ Navi session mapping](12-story-navi-session-mapping.md) | Agares | 01, 09, 10 |
| 13 | [Stream + cancel in StoryChat](13-stream-cancel-storychat.md) | Agares | 12 |
| 14 | [CreateAssistant on Navi](14-create-assistant-navi.md) | Agares | 05, 09, 11 |
| 15 | [Mode packs as skills/profiles](15-mode-packs-skills-profiles.md) | Agares | 05, 12, 14 |
| 16 | [ACP bindings](16-acp-bindings.md) | navi (optional) | — |
| 17 | [Converge local GGUF under Navi](17-converge-local-gguf-navi.md) | both (later) | 08, 12 |
| 18 | [Credentials under vault story](18-credentials-under-vault.md) | Agares | 09 |

## Frontier (can start immediately)

- Agares product tickets **09–15**, **18** (outside this repo)

Completed navi frontier: **01**, **02**, **03**, **04**, **05**, **06**, **07**, **08**, **16**.

## Graph (simplified)

```text
01 ──► 07
01 ──► 12
02 ──► 03 ──► 05 ──► 14 / 15
02 ──► 04 ──► 11
02 ──► 08 ──► 09 ──► 10 ──► 11
                09 ──► 12 ──► 13
                09 ──► 14
06 (parallel)
16 (parallel, optional)
```
