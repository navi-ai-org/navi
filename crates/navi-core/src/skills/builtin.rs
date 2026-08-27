//! Built-in NAVI skills shipped with the engine (not stored in SQLite).

use super::{SkillManifest, SkillSource, SkillWriteScope};
use std::path::PathBuf;

/// Id of the skill that teaches NAVI how to author other skills.
pub const CREATE_SKILL_ID: &str = "navi-create-skill";
/// Id of the skill that teaches harness pack authoring / materialize limits.
pub const HARNESS_AUTHOR_ID: &str = "navi-harness-author";
/// Id of the skill that teaches skill pools / catalog navigation.
pub const SKILL_POOLS_ID: &str = "navi-skill-pools";

/// Returns all built-in skills.
pub fn builtin_skills() -> Vec<SkillManifest> {
    vec![
        create_skill_manifest(),
        harness_author_manifest(),
        skill_pools_manifest(),
    ]
}

fn base_navi_manifest(
    id: &str,
    name: &str,
    description: &str,
    version: &str,
    tags: &[&str],
    allow_tools: &[&str],
    instructions: &str,
) -> SkillManifest {
    SkillManifest {
        id: id.into(),
        name: name.into(),
        description: Some(description.into()),
        version: Some(version.into()),
        author: Some("NAVI".into()),
        tags: tags.iter().map(|t| (*t).to_string()).collect(),
        requires: vec![],
        allow_tools: allow_tools.iter().map(|t| (*t).to_string()).collect(),
        deny_tools: vec![],
        // Engine authoring skills must never soft-lock the root session when
        // discovered in the catalog. Soft allowlists apply only for session-active
        // harness skills / materialize packs (see harness_pack::apply).
        harness: false,
        pool: Some("navi".into()),
        path: PathBuf::from(format!("builtin:navi/{id}")),
        source: SkillSource::Builtin,
        scope: SkillWriteScope::User,
        instructions: instructions.into(),
    }
}

fn create_skill_manifest() -> SkillManifest {
    base_navi_manifest(
        CREATE_SKILL_ID,
        "Create NAVI Skill",
        "Author a durable NAVI skill as markdown on disk (optionally inside a skill pool). Load this when the user asks to add/create a skill.",
        "1.4.0",
        &["navi", "builtin", "skills", "authoring", "harness"],
        &[
            "skill_list",
            "skill_get",
            "skill_delete",
            "load_skill",
            "question",
            "read_file",
            "write_file",
            "edit",
            "run",
        ],
        CREATE_SKILL_INSTRUCTIONS,
    )
}

fn harness_author_manifest() -> SkillManifest {
    base_navi_manifest(
        HARNESS_AUTHOR_ID,
        "Author NAVI Harness Pack",
        "How to author SKILL.md + harness packs (materialize, soft graph allow_tools, loop caps). Load when the user wants a multi-step harness.",
        "1.1.0",
        &["navi", "builtin", "harness", "authoring"],
        &[
            "skill_list",
            "skill_get",
            "load_skill",
            "question",
            "read_file",
            "write_file",
            "edit",
            "run",
        ],
        HARNESS_AUTHOR_INSTRUCTIONS,
    )
}

fn skill_pools_manifest() -> SkillManifest {
    base_navi_manifest(
        SKILL_POOLS_ID,
        "NAVI Skill Pools",
        "How skill pools, skill_list, and load_skill work. Load when the user is lost in the skill catalog.",
        "1.0.0",
        &["navi", "builtin", "skills", "pools"],
        &["skill_list", "skill_get", "load_skill", "read_file"],
        SKILL_POOLS_INSTRUCTIONS,
    )
}

const CREATE_SKILL_INSTRUCTIONS: &str = r#"# Create a NAVI Skill

You help the user design and **save** a durable NAVI skill as a markdown (`.md`) or TOML (`.toml`) file on disk.

Skills live in the filesystem skill store:

| Path | Meaning |
|------|---------|
| `{data_dir}/skills/<id>/SKILL.md` | Root-level user skill |
| `{data_dir}/skills/<pool>/<id>/SKILL.md` | Skill inside a **pool** (folder) |
| `{project}/.navi/skills/<id>/SKILL.md` | Project-scoped skill |

When the user says things like "adicione uma skill", "create a skill for X", or
"add a skill that…", **load this skill first** (you may already have it open),
then write the skill file to `{data_dir}/skills/<id>/SKILL.md` — do not invent a
private file format under random paths.

## What a skill is

| Field | Purpose |
|-------|---------|
| `id` | Stable slug (e.g. `code-reviewer`). Optional — derived from name. |
| `name` | Human title. |
| `description` | One line for pickers / catalogs. |
| `instructions` | Markdown the agent follows when the skill is active. |
| `pool` | Optional folder id (e.g. `navi`). Empty = root catalog. |
| `allow_tools` | **Required for focused skills.** Only these tools are offered to the model while this skill is active (intersection if several skills set allow lists). |
| `deny_tools` | Optional extra denylist metadata. |
| `tags` / `requires` | Optional metadata. `requires` lists skill ids for harness chains. |
| `scope` | `user` (shared Desktop + TUI) or `project` (this repo only). |
| `harness` | When `true`, NAVI materializes a harness pack (`loop.toml` + `graph.toml`). |

## Tool policy rules

1. A skill that only injects prose without `allow_tools` does **not** lock tools.
2. If **any** active skill sets non-empty `allow_tools`, the session tool set is the **intersection** of those lists.
3. Host security (permission mode, path guards) still applies on top.
4. For authoring skills, keep `allow_tools` tight — only what that job needs.
5. **Never** browse `{data_dir}` with `search` / raw filesystem tools for skills. Use `skill_list` / `skill_get` / `load_skill` to inspect, and `write_file` / `edit` to mutate skill files.

## Harness skills

A **harness** is a multi-step, multi-skill workflow that NAVI materializes into a pack under `{data_dir}/harnesses/<skill-id>/`. When you create a harness:

1. Ask the user whether this is a **single skill** or a **harness (multi-node workflow)**.
2. If harness:
   - Set `harness = true` and `requires: [sub-skill-ids…]` in the main skill frontmatter.
   - Write each sub-skill's `SKILL.md` first.
   - Write the main harness `SKILL.md` to `{data_dir}/skills/<id>/SKILL.md`.
   - Run `navi harness materialize <id>` (via `run`) to generate `loop.toml`, `graph.toml`, etc.
   - Verify with `navi harness show <id>` or by reading `{data_dir}/harnesses/<id>/graph.toml`.
   - Soft graph `allow_tools` and loop caps apply only when that harness skill is session-active (CLI `--skill` / host activate / config), not merely because it is installed.
3. Hard graph edge execution is still MVP-soft — do not promise automatic routing between nodes.

Also load `navi-harness-author` (pool `navi`) for pack layout details.

## Workflow

1. Clarify the job: when should this skill activate? What must the agent do / not do?
2. If the skill belongs to a product area (e.g. NAVI), `skill_list` that pool first.
3. If ambiguous, use `question` before saving.
4. Draft a **template** (below) with the user.
5. Choose a **minimal** `allow_tools` list from real tool names.
6. Write the skill file with `write_file`. For pools, include `pool: "<pool>"` in the frontmatter.
7. If the skill is a harness, run `navi harness materialize <id>` via `run`.
8. Call `skill_get` to verify; offer to refine.

## Skill template (copy into `instructions`)

```markdown
# <Skill Name>

## When to use
- …

## Goals
- …

## Procedure
1. …
2. …

## Constraints
- Do not …
- Prefer …

## Done when
- …
```

## Saving

Write the skill as a `SKILL.md` file. Example frontmatter:

```markdown
---
name: My Skill
id: my-skill
description: "One-line summary."
tags: [example]
allow_tools: [read_file, write_file]
requires: []
scope: user
harness: false
---

# My Skill

## When to use
- …
```

- User-scope root skills go to `{data_dir}/skills/<id>/SKILL.md`.
- User-scope pool skills go to `{data_dir}/skills/<pool>/<id>/SKILL.md`.
- Project-scoped skills go to `{project}/.navi/skills/<id>/SKILL.md`.

You can also run `navi skill install path/to/SKILL.md` via `run` when the user provides a local skill file.

Use **`skill_list`** / **`skill_get`** / **`load_skill`** to inspect existing skills.
Use **`skill_delete`** only if the user confirms removing a skill.

## Anti-patterns

- Do **not** write skills into random config trees outside `{data_dir}/skills/` or `{project}/.navi/skills/`.
- Do **not** save empty instructions or empty names.
- Do **not** set `harness = true` without defining `requires` or writing a clear multi-step procedure; a vague harness is just a slow prompt.
- Do **not** assume marketplace skills match the current engine harness API — engine essentials stay builtin.
"#;

const HARNESS_AUTHOR_INSTRUCTIONS: &str = r#"# Author a NAVI Harness Pack

Teach the user (and yourself) how harness packs work on this engine version.

## Two activation paths

| Path | What happens |
|------|----------------|
| **CLI / install** | Write the skill `SKILL.md` to `{data_dir}/skills/<id>/SKILL.md` with `harness: true` and `requires`, then run `navi harness materialize <id>` to generate the pack under `{data_dir}/harnesses/<id>/`. Soft apply when the skill is **session-active**. |
| **Chat** | User says "use the design harness" / "roda o design-loop" → activate that skill id for the session; model may `load_skill` and then write `SKILL.md` / run `navi harness materialize` without dumping `graph.toml` by hand. |

## What materialize writes

```text
{data_dir}/harnesses/<id>/
  loop.toml      # max_turns, optional token_budget, stop hints
  graph.toml     # soft entry node + allow_tools (MVP)
  …
```

## Soft graph limits (MVP)

- Entry-node `allow_tools` may soft-lock the **session** only while the harness skill is active.
- Catalog discovery of skills with `allow_tools` does **not** lock tools.
- Hard edge routing and feedback evolve jobs are **not** implemented — document intent in SKILL.md prose, do not fake hard routing.

## Workflow

1. Create leaf skills by writing their `SKILL.md` files (focused instructions + recommended tools).
2. Create the main skill file with `harness: true` and `requires: [leaf ids…]`.
3. Run `navi harness materialize <id>` (via `run`) and confirm the pack path with `navi harness show <id>`.
4. Activate with CLI `--skill <id>` / host session skills / chat intent — not by inventing files under project `.navi/`.

Engine essentials (`navi-create-skill`, this skill, `navi-skill-pools`) ship **builtin** so marketplace version skew cannot teach stale harness APIs.
"#;

const SKILL_POOLS_INSTRUCTIONS: &str = r#"# NAVI Skill Pools

Skills are a filesystem-like catalog.

## Surfaces

| Call | Result |
|------|--------|
| `skill_list` (no pool) | Root skills + **pool folders** only |
| `skill_list` `{ "pool": "navi" }` | Skills inside that pool (metadata) |
| `load_skill` / `skill_get` | Full instructions + policy |

## Built-in pool `navi`

Engine authoring skills live here (not at the root catalog):

- `navi-create-skill` — create/save skills
- `navi-harness-author` — harness packs / soft graph
- `navi-skill-pools` — this skill

Open with: `skill_list` → `{ "pool": "navi" }` → `load_skill` with `pool/id`.

## Private storage

Do **not** use `search` or raw filesystem tools on `{data_dir}` (sessions, memory, skills store). That path is jailed. Browse skills with `skill_list` / `skill_get` / `load_skill`, and mutate them by writing `SKILL.md` files to `{data_dir}/skills/<pool>/<id>/SKILL.md` (or `{project}/.navi/skills/...` for project scope).

Project `.navi/skills` is user-authored project scope — still prefer writing skill files over ad-hoc shell.
"#;
