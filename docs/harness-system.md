# Harness System Vision

**Status:** Design vision (not fully implemented)  
**Audience:** Product, engine, and embedding hosts  
**Related:** [Goal System](goal-system.md) · [SDK Agents](sdk-agents.md) · [Workflow Lua](workflow-tool-lua-spec.md) · [ADR 0013 WASM plugins](adr/0013-wasm-only-plugins.md)

This document captures the product and architecture vision for **harness packs** on NAVI: how skills, goals, loops, graphs, verifiers, and self-improvement jobs compose so users can build custom agent harnesses *using NAVI itself*.

---

## 1. Problem statement

### 1.1 The industry interchange format is prose

In the wider agent ecosystem, the dominant portable unit is a **skill document** (typically `SKILL.md` / markdown with frontmatter): policy, steps, and tool hints in natural language.

That is necessary for sharing, but insufficient for reliability:

| Approach | What the model sees | Who enforces the loop? |
|---|---|---|
| Skill markdown only | Long procedural text | Prompt-following luck |
| Skill + `loop.toml` / `graph.toml` on disk | Extra files *if* the model reads them | Still the model, unless the runtime interprets them |
| Skill + **runtime-executed** specs | Goals, tool allowlists, gates | **The engine** |
| Skill + **materialize + feedback jobs** | Specs the agent *wrote* and later *revises* | Engine + human signal + metrics |

Without the last two rows, a “graph” is cosmetic YAML—a second prompt.

### 1.2 NAVI’s opportunity

NAVI is not only a chat TUI. It is a **local agentic engine** (runtime, tools, providers, sessions, security, plugins, MCP, goals, SDK). The product bet:

> Skills remain the **import UI** of the ecosystem.  
> Harnesses are the **local compilation** that NAVI materializes, runs, and recompiles from user feedback—optionally extending the engine via WASM plugins and MCP.

Users should be able to design **loop engineering** and **graph engineering** workflows *as content* (packs), not only as hard-coded core features.

---

## 2. Concepts

### 2.1 Loop engineering

**Loop engineering** is the design of a closed control cycle:

```text
Plan → Execute → Verify → Reflect → (Improve / Steer) → Repeat
         ↑______________________________________________|
```

A serious loop specifies:

1. **Exit criteria** — what counts as done (tests, browser smoke, goal complete, human sign-off).
2. **Feedback signal** — numeric score *and* textual “why”.
3. **Retry policy** — what changes on the next iteration (prompt, tools, budget, subagent).
4. **Budget** — tokens, wall time, max turns (see [Goal System](goal-system.md)).
5. **Observability** — each iteration leaves durable artifacts (events, plan file, screenshots, run JSON).

The public discourse around *harness engineering* (the software around the model: context, tools, subagents, memory, evaluation, next-step policy) is the same layer: optimize the **system**, not only weights.

### 2.2 Graph engineering

**Graph engineering** designs the agent as a **state graph**, not a monologue:

| Piece | Meaning |
|---|---|
| **Nodes** | Agents, skills, tools, humans, verifiers |
| **Edges** | Conditional routing (“tests fail → fixer”) |
| **State** | Shared session/goal/plan/artifacts |
| **Topologies** | Linear, fan-out/fan-in, hierarchical, adversarial (implementer ↔ reviewer) |

Related industry framing: deterministic **workflows** vs open **agents** with tools; multi-actor orchestration (e.g. graph runtimes such as LangGraph-style patterns).

On NAVI today, **subagents**, **workflow (Lua)**, and **skill tool allowlists** are embryos of a graph runtime. The vision elevates them to a first-class harness language.

### 2.3 How loop and graph compose

```text
                    GRAPH (structure)
         who runs, with which tools, when
                         │
                         ▼
              ┌─────────────────────┐
              │   LOOP (dynamics)   │  plan / act / verify / reflect
              │   per node / global │
              └─────────────────────┘
                         │
                         ▼
              HARNESS (product pack)
    skills · goals · security · events · CLI · eval
```

---

## 3. Harness pack layout

### 3.1 User-facing interchange (minimal)

What the world can ship and what `navi skill install` accepts:

```text
design-loop/
  SKILL.md                 # required — policy, steps, tool hints
  harness.manifest.toml    # optional — marks skill as a harness
```

### 3.2 Local compilation (materialized)

Default storage: **`{data_dir}/harnesses/<skill-id>/`**  
(Linux data dir is typically `~/.local/share/navi`.)

Do **not** auto-create project `.navi/` bookkeeping; project `.navi/config.toml` remains user-authored only (see AGENTS.md).

```text
{data_dir}/harnesses/design-loop/
  SKILL.md                 # may be enriched beyond the import
  loop.toml                # budget, max_turns, stop conditions
  graph.toml               # optional nodes/edges (or linear default)
  verifiers/               # recipes: bash, browser, future plugin tools
  CAPABILITY.md            # assumed NAVI + project capabilities
  CHANGELOG.md             # harness evolution history
  runs/                    # per-run traces + user feedback
```

Optional later: export a pack back into a repo for version control when the user asks.

### 3.3 Example mental `loop.toml` (illustrative)

```toml
id = "design-loop"
max_turns = 15
token_budget = 80_000
stop = ["verify.ok", "goal.complete", "goal.blocked", "budget"]

[[verify]]
id = "preview_smoke"
kind = "browser"           # or bash | plugin
args = { url = "http://localhost:3000", actions = ["goto", "screenshot"] }
```

### 3.4 Example mental `graph.toml` (illustrative)

```toml
entry = "explore"

[[nodes]]
id = "explore"
role = "read_only"
tools = ["search", "read_file", "browser", "tool_search"]

[[nodes]]
id = "implement"
role = "write"
tools = ["search", "read_file", "edit", "write_file", "bash"]

[[nodes]]
id = "verify"
role = "verify"
verifiers = ["preview_smoke"]

[[edges]]
from = "explore"
to = "implement"

[[edges]]
from = "implement"
to = "verify"

[[edges]]
from = "verify"
to = "implement"
when = "verify.failed"
```

These formats are **local compilation targets**, not a requirement for the global skill ecosystem.

---

## 4. Lifecycle: three jobs around a skill

```text
  navi skill install / plugin skill pack
                 │
                 ▼
       ┌─────────────────────┐
       │ JOB A — Materialize │  self-interview + research + write pack
       └──────────┬──────────┘
                  │
         user activates skill / --skill
                  ▼
       ┌─────────────────────┐
       │ RUN — Execute       │  goals + tools + graph/subagents
       └──────────┬──────────┘
                  │
                  ▼
       ┌─────────────────────┐
       │ JOB B — Harvest     │  traces + ask user feedback
       └──────────┬──────────┘
                  │
                  ▼
       ┌─────────────────────┐
       │ JOB C — Evolve      │  patch skill/loop/graph; propose plugin/MCP
       └─────────────────────┘
```

### 4.1 Job A — Materialize (on install / first activate)

External contract stays simple:

```bash
navi skill install ./design-loop.md
# → enqueue materialize-harness(skill_id)
```

The job (headless session or isolated subagent) should:

1. **Self-interview** (internal reasoning; `question` only if domain facts are missing).
2. **Capability survey** — this build/session of NAVI (tools Direct/Deferred, browser, goals, workflow, installed plugins, MCP).
3. **Project survey** — stack, test/build commands, AGENTS.md, preview URLs.
4. **Write** `loop.toml`, `graph.toml`, verifier stubs, `CAPABILITY.md`.
5. **Never invent tools** not present on the capability card; record gaps and fall back (e.g. bash/browser).

Compiler-style system framing for Job A:

```text
You are the NAVI harness compiler.
Inputs: SKILL.md, capability card, project card.
Outputs: only files under the harness pack path.
Never invent tools not listed on the capability card.
If a capability is missing, record it under gaps[] and continue with fallbacks.
```

### 4.2 Job B — Feedback after use

Triggers (examples):

- Session ends with harness skill active  
- Goal reaches terminal / blocked  
- User runs an explicit feedback command  
- Headless flag such as `--harness-feedback`

Collect:

| Signal | Source |
|---|---|
| Goal complete / blocked / budget | engine events |
| Tool error rate / retries | session events |
| Free-text feedback | `question` tool or CLI prompt |
| Structured ratings | multi-select (correctness, speed, trust, missing capability) |

Persist under `runs/<timestamp>.json` plus user feedback.

### 4.3 Job C — Evolve

With feedback + traces, classify changes:

| Priority | Kind | Examples |
|---|---|---|
| P0 | Safety | Fewer destructive defaults, stronger verify gates |
| P1 | Reliability | Better graph split, tighter loop stops |
| P2 | Capability | Propose WASM plugin or MCP server |

Mutations:

1. Prompt / `SKILL.md` steps  
2. Loop params (budget, verify order)  
3. Graph topology  
4. Verifier recipes  
5. **Local WASM plugin** proposal (deterministic host tools)  
6. **MCP** proposal (external services)

Security: prompt/graph evolution under `{data_dir}` is low risk; **plugin install and MCP enable require explicit approval**. Project config must not silently enable plugins/MCP (existing NAVI policy).

---

## 5. Enforcement: reducing prompt-only compliance

| Level | Mechanism | Compliance |
|---|---|---|
| Soft | SKILL.md + capability cards in context | Low |
| Medium | Host injects goal + plan path + “next node” steering each turn | Medium |
| Hard | Runtime exposes only tools for the *current node*; advance edge only if verifier OK | High |

A strong harness uses **hard gates at boundaries** (verify, tool jail per phase) and soft freedom *inside* a node.

Existing NAVI knobs the materializer should target:

- Skill `allow_tools` / `deny_tools`  
- Plan mode (write jail)  
- Goals (auto-continue + budget)  
- Subagent allowlist intersection  
- Workflow (Lua) orchestration  

---

## 6. Capability awareness (system / developer context)

### 6.1 What the code-agent prompt does today

The default harness prompt (see `navi-core` harness builder) covers:

- Identity and inspect → edit → verify workflow  
- When to use `plan` / `create_goal`  
- **Static** core tool list  
- Power tools via `tool_search`  
- Project **AGENTS.md** as a developer message  
- Active skill instructions  
- Optional memory injection  

Gaps that block self-materializing harnesses:

| Missing awareness | Failure mode |
|---|---|
| Live Direct vs Deferred tool inventory | Graph invents unavailable tools |
| Build features (browser, goals enabled) | Assumes browser when disabled |
| Installed plugins / MCP tools | Never proposes real extensions |
| Host surfaces (skill install, headless flags) | Unrealistic harnesses for CI |
| Existing harness versions | Re-materializes from scratch every time |

### 6.2 Capability card (proposed)

Inject a short, **hash-stable** developer block (invalidate only when the tool/plugin/MCP set changes):

```text
## NAVI capabilities (this session)
- goals: enabled | max_auto_continue: 50
- browser: available
- tools.direct: [search, read_file, edit, write_file, bash, plan, question, tool_search, memory, …]
- tools.deferred: discover via tool_search (code, browser, subagent, workflow, …)
- plugins.installed: [id → tools]
- mcp.connected: [id → tools]
- harnesses.ready: [design-loop@3, …]
```

### 6.3 Project card

Complement AGENTS.md with structured hints when available: package manager, test/build commands, preview URL, constraints.

### 6.4 Active harness card

When a harness skill is active: pack path, loop stop conditions, graph nodes + tool sets, verifier ids.

---

## 7. Plugins and MCP as harness evolution

Feedback must not stop at “better prose.” Upgrade ladder:

```text
1. Prompt / SKILL steps
2. Loop params
3. Graph topology
4. Verifier recipes
5. Host tool / WASM plugin   ← extends local NAVI
6. MCP server                ← extends external world
```

**Propose a plugin** when the gap is local, deterministic, and schema-shaped (e.g. structured a11y audit, coverage delta). Flow: scaffold → `navi plugin install` (approval) → bind tools into graph/skill allowlist.

**Propose MCP** when the capability is an external service or an existing MCP server. Skill/harness may declare `requires_mcp = ["…"]`.

---

## 8. Mapping to NAVI today

| Vision piece | Present foundation | Still needed |
|---|---|---|
| Skill install | `navi skill install`, `skill_save`, plugin skill import | Hook **on_install → materialize job** |
| Skill activate | `--skill`, config `skills.active`, TUI | Bind skill → harness pack path |
| Long loop | Thread [goals](goal-system.md) auto-continue | Interpret `loop.toml` (max turns, stop predicates) |
| Multi-agent | `subagent`, `workflow` | Executable `graph.toml` |
| Self-improve content | Model can edit files / save skills | Job C + policy + CHANGELOG |
| Plugins / MCP | Install paths exist | Proposal pipeline + skill↔plugin binding |
| Capability awareness | Static list + `tool_search` | Live capability card |
| Project awareness | AGENTS.md, search | Structured project card |
| Eval | `navi eval` / bench | Cases under harness pack |

---

## 9. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Infinite self-modification | Budget the meta-loop; version harnesses; require feedback threshold |
| Plugin spam | Only propose after repeated same gap (e.g. N runs) |
| Mid-node prompt drift | Hard enforcement on edges (verify/tool jail) |
| Prompt-cache thrash | Capability card keyed by hash |
| Trust boundary | Evolve packs in data_dir; never auto-enable project plugins/MCP |

---

## 10. Suggested implementation phases

1. **Capability card** in developer context (cheap, unlocks materialization quality).  
2. **Harness pack layout** under `{data_dir}/harnesses/` + skill_id → pack.  
3. **Job A materialize** (headless; dedicated compiler skill/prompt).  
4. **Run path**: active skill loads loop (goal budget/max turns) + soft graph via subagent/workflow.  
5. **Job B feedback** (`question` + run artifacts).  
6. **Job C evolve** (patch pack; *propose* plugin/MCP with approval).  
7. **Hard graph execution** once soft packs prove value.

---

## 11. Product one-liner

**Skills are the ecosystem import UI; harnesses are the local compilation that NAVI materializes, runs with goals/tools/graphs, and recompiles from user feedback—optionally extending the engine through plugins and MCP.**

---

## 12. Presentation asset

A visual overview for stakeholders lives at:

[presentations/harness-vision/index.html](presentations/harness-vision/index.html)

Open in a browser for the slide-style walkthrough.
