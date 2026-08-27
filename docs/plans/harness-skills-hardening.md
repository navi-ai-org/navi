# Plan: harness + skills hardening (`feat/harness-skills-hardening`)

**Branch:** `feat/harness-skills-hardening`  
**Base:** `main` @ release post-v0.3.5  
**Audience:** human + sandbox agents (Grok/Claude/Codex)  
**Related:** [harness-system.md](../harness-system.md) · [goal-system.md](../goal-system.md) · AGENTS.md

---

## Why this branch exists

A real TUI/session on **v0.3.5** (user home, “add these design skills to navi”) exposed broken product behavior:

1. **Main agent treated like a harness subagent** — tools denied with  
   `tool \`X\` is not in the allowed tool set for this subagent` even for normal chat.
2. **`skill_list` / skill-manage path unusable** when allowlists leak from catalog/builtins.
3. **Create-skill path incomplete for product use** — model should load a built-in create-skill and use `skill_save`, without marketplace version skew.
4. **Harness should activate two ways:** explicit pack/CLI markdown path **and** natural language (“adicione uma skill para X” / “roda o design-loop”).
5. **Marketplace / plugin install** needs verification that it works end-to-end; fix if not.

This plan is the work order for a **remote sandbox** (heavy `cargo test` / compile off the laptop). Merge back via this branch only.

---

## Multi-agent coordination (mandatory)

Many subagents may edit the same repo. **Do not thrash the working tree.**

| Rule | Detail |
|------|--------|
| **No bulk restore** | Never `git restore`, `git checkout -- <paths>`, `git reset --hard`, or `git clean -fd` to “fix dirty”. Dirty files are other agents’ WIP. |
| **No clobber** | Do not overwrite files you did not open/own in this turn. If an edit races (content changed under you), re-read and merge; do not wipe. |
| **Dirty tests** | If `cargo test` fails because another agent broke the tree mid-flight, **wait** (poll status / re-run later). Do not “fix” their code by restoring HEAD. |
| **Own your scope** | Prefer package-scoped checks: `cargo test -p navi-core -- --test-threads=4` (max 4 threads; see AGENTS.md). |
| **Commits** | Prefer small commits per task area. Conventional subjects + changelog body (`### Added` / `### Fixed`). Do not mix unrelated scopes. |
| **State dirs** | No project `.navi/` auto bookkeeping. Engine state → `{data_dir}` only. |

---

## Bugs from the affected session (reproduce → fix → test)

### B1 — Session tool allowlist leak (CRITICAL)

**Symptom:** Main session denies tools with *subagent* allowlist error; `skill_list` / arbitrary tools fail.

**Likely causes (already partially fixed on main for catalog skills):**

- `apply_harness_for_skills` / `harness_allow_tools` applied to **all catalog/builtin skills** that have `allow_tools`, not only active harness packs.
- `TurnContext.allowed_tool_names` set from `harness_allow_tools` for **root** turns (error string still says “subagent”).
- Soft graph entry `allow_tools` merged into the **session-wide** allowlist incorrectly.

**Done on main (v0.3.5 fix path):** catalog skills with `harness: false` no longer push `allow_tools` into the session lock. **Still verify** end-to-end:

- [x] Fresh session, no harness activated → full Direct tool set works (`skill_list`, `read_file`, …).
- [x] Activating a harness skill with pack graph **does** restrict only as designed.
- [x] Subagent still gets its own allowlist; root never inherits subagent allowlist by accident.
- [x] Error messages: root denials must not say “for this subagent” when the deny is harness/root policy.

**Tests required:** unit + integration on `navi-core` runtime/turn:

- root turn with builtin create-skill in catalog → tools not locked;
- root turn with harness-active skill + pack entry allowlist → only those tools;
- nested subagent allowlist independent of parent catalog.

### B2 — Skill tools / create-skill product path

**Symptom:** Agent cannot list/save skills; create-skill not discoverable or not loaded when user asks “adicione uma skill”.

**Work:**

- [x] Confirm builtin `navi-create-skill` (pool `navi`) is in catalog + prompt surface correctly (pools: root + pool folders, members via `skill_list { pool }`).
- [x] `skill_save` / `skill_list` / `load_skill` work under default security modes used by TUI.
- [x] Natural language path: system/prompt or skill description makes the model **load_skill** create-skill before inventing formats.
- [x] CLI: `navi skill install|list` + materialize hook still green.

### B3 — Private storage path jail false positives (session transcript)

**Symptom:** Agent tried `~/.local/share/navi` via search tools and got “inside NAVI private storage”.

**Work:**

- [x] Document that agents must use **skill tools**, not raw FS into `{data_dir}`.
- [x] If product should allow read-only skill browsing via tools only — keep jail; do not open private storage to bash/search.
- [x] Optional: clearer tool error → “use skill_list / skill_get, not filesystem under data_dir”.

---

## Task map

### T1 — Fix session bugs from transcript (B1–B3)

**Owner crate:** `navi-core` (+ small TUI/SDK if events/copy change)  
**Exit criteria:** unit + integration tests; `cargo test -p navi-core -- --test-threads=4` green for new cases; manual mental walkthrough of transcript scenarios.

### T2 — Harness activation: CLI markdown **and** conversational

**Product intent:**

| Path | Behavior |
|------|----------|
| **CLI / install** | `navi skill install` / pack files → materialize under `{data_dir}/harnesses/<id>/`; soft apply when skill is active. |
| **Chat** | User says “use the design harness” / “adicione skill X” → model loads skill, may call `skill_save`, may activate harness without user dumping full graph.toml. |

**Work:**

- [x] Document activation contract in `docs/harness-system.md` (two paths).
- [x] Ensure soft-apply only when skill is **session-active / harness-flagged**, not merely discovered.
- [x] If graph edges are still MVP-soft, document limits; do not fake hard routing.
- [x] Tests: install → materialize → activate → allowlist; chat-driven save skill without locking root tools.

### T3 — Built-in essential skills (not marketplace-first)

**Rationale:** Marketplace skills can teach **stale harness APIs** after a core upgrade. Essentials for “how to use NAVI” stay **builtin** (versioned with the binary).

**Candidates (implement as builtin manifests under `navi-core` skills builtin pool, e.g. pool `navi`):**

| Id (suggested) | Purpose |
|----------------|---------|
| `navi-create-skill` | Already exists — harden description/allow_tools/prompt so models actually use it. |
| `navi-harness-author` | How to author `SKILL.md` + harness pack, materialize, loop/graph limits. |
| `navi-skill-pools` | How pools / `skill_list` / `load_skill` work. |
| Optional later | design-pipeline skills (customer-analyst …) as **separate pool** if product wants them in-tree; prefer builtin only if they are engine-coupled. |

**Design-pipeline skills** from the user session (customer-analyst → ux-verifier):

- Prefer a **builtin pool** `design-dojo` (or similar) **only if** we commit to shipping them with the engine.
- Otherwise: ship as example pack under `examples/` or marketplace **after** create-skill path works.
- If shipped builtin: six skills + optional harness graph (Discover → … → Verify), with `requires`/graph nodes matching [harness-system](../harness-system.md) MVP (soft allow_tools + loop caps).

**Work:**

- [x] Inventory current `builtin_skills()`.
- [x] Add/update builtins with tests for discovery, catalog rendering, pool membership.
- [x] No marketplace dependency for create-skill / harness-author.

### T4 — Marketplace correctness

**Scope:** plugin marketplace CLI + install path (WASM). Skills marketplace if present.

**Work:**

- [x] Trace `navi plugin search|install|update` against config `plugin_marketplace.registry_url`.
- [x] Integration test or docker-friendly mock if network-flaky.
- [x] Confirm install lands under `{data_dir}/plugins/`, not project `.navi/`.
- [x] Document “skills via marketplace vs builtin” version risk in user-guide or harness-system.

### T5 — Harness system correctness audit

Against [harness-system.md](../harness-system.md) MVP table:

| Capability | Verify |
|------------|--------|
| Pack store `{data_dir}/harnesses/<id>/` | store + load roundtrip tests |
| Materialize from skill + inventory | materialize tests |
| CLI skill install → materialize | CLI or core hook test |
| Soft graph entry allow_tools | apply tests; **no catalog leak** |
| Loop max_turns / token budget | runtime apply tests |
| Capability card in developer context | prompt/runtime test |
| Hard edges / feedback jobs | still out of scope unless trivial; document |

### T6 — Tests & coverage (real behavior, not mock theater)

Per AGENTS.md:

```bash
cargo fmt --all -- --check
cargo check -p navi-core
cargo test -p navi-core -- --test-threads=4
# then sdk/cli as needed
cargo test -p navi-sdk -- --test-threads=4
cargo test -p navi-cli -- --test-threads=4
```

**Requirements:**

- Prefer **production path** tests (real `ToolExecutor` / runtime allowlist), not only Mock/WorkerProbe.
- Cover edge cases: empty allowlist, harness:false with allow_tools, pool skills, skill_save project vs user scope, cancel mid-turn.
- Critical-path coverage gate if already in CI — extend for harness/skill modules touched.
- Do **not** require whole-workspace 100% line coverage; **do** require 100% of **new public contracts** for harness activation + skill catalog locking behavior with meaningful assertions.

---

## Suggested implementation order (sandbox)

```text
T1 B1 allowlist leak (tests first)
  → T2 activation contract
  → T5 audit + docs sync
  → T3 builtins (create-skill harden + optional design-dojo)
  → T4 marketplace
  → T6 full package-scoped test pass + fmt
  → push branch; open PR
```

---

## Sandbox bootstrap (for you)

```bash
# On the remote box after gh + git auth:
gh repo clone navi-ai-org/navi
cd navi
git fetch origin
git checkout feat/harness-skills-hardening
# Read this plan:
#   docs/plans/harness-skills-hardening.md
# Then start Grok/agent with instruction to follow multi-agent rules above.
```

**Local merge later:**

```bash
cd ~/projects/navi
git fetch origin
git checkout feat/harness-skills-hardening
git pull --ff-only origin feat/harness-skills-hardening
# review, then merge to main when green
```

---

## Out of scope (unless unblocked)

- Hard graph edge execution / feedback evolve jobs (vision only).
- Tutor UI.
- Publishing a new crates.io/npm release (do on main after merge).
- Force-pushing `main` or rewriting release tags.

---

## Acceptance checklist

- [x] Root session never blocked by catalog skill `allow_tools`.
- [x] Harness soft allowlist only when harness skill/pack is active.
- [x] “Adicione uma skill…” path works via builtin create-skill + `skill_save`.
- [x] CLI skill install + harness materialize still work.
- [x] Marketplace plugin search/install verified or fixed + tested.
- [x] Package-scoped tests green; no bulk git restore used during multi-agent work.
- [x] Docs updated (`harness-system.md` activation paths; this plan checked off).
