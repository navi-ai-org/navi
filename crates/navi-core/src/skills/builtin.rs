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
        "1.3.0",
        &["navi", "builtin", "skills", "authoring", "harness"],
        &[
            "skill_list",
            "skill_get",
            "skill_save",
            "skill_delete",
            "load_skill",
            "question",
            "read_file",
        ],
        CREATE_SKILL_INSTRUCTIONS,
    )
}

fn harness_author_manifest() -> SkillManifest {
    base_navi_manifest(
        HARNESS_AUTHOR_ID,
        "Author NAVI Harness Pack",
        "How to author SKILL.md + harness packs (materialize, soft graph allow_tools, loop caps). Load when the user wants a multi-step harness.",
        "1.0.0",
        &["navi", "builtin", "harness", "authoring"],
        &[
            "skill_list",
            "skill_get",
            "skill_save",
            "load_skill",
            "question",
            "read_file",
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

You help the user design and **save** a durable skill as markdown on disk.

When the user says things like "adicione uma skill", "create a skill for X", or
"add a skill that…", **load this skill first** (you may already have it open),
then use `skill_save` — do not invent a private file format under random paths.

## Skill pools (folders)

Skills are organized like a filesystem:

| Path | Meaning |
|------|---------|
| `{data_dir}/skills/<id>/SKILL.md` | Root-level skill (top catalog) |
| `{data_dir}/skills/<pool>/<id>/SKILL.md` | Skill inside a **pool** (folder) |
| `{project}/.navi/skills/…` | Same layout for project scope |

The **Available Skills** catalog and bare `skill_list` show only:
- root skills (metadata), and
- **pools** as folders (id, name, skill_count) — **not** every nested skill.

To work with pool members (e.g. NAVI authoring skills under `navi`):

1. `skill_list` with `{ "pool": "navi" }` — catalog of skills in that folder.
2. `load_skill` / `skill_get` with `{ "id": "navi-create-skill", "pool": "navi" }`
   or `{ "id": "navi/navi-create-skill" }` — full instructions.
3. `skill_save` with `"pool": "navi"` (or another pool id) to place a new skill
   inside a folder; omit `pool` for root-level skills.

Example: user asks to add a skill for NAVI itself → open pool `navi`, read the
create-skill skill, then `skill_save` (often with `pool: "navi"` if it is a
NAVI-product skill).

## What a skill is

| Field | Purpose |
|-------|---------|
| `id` | Stable slug (e.g. `code-reviewer`). Optional — derived from name. |
| `name` | Human title. |
| `description` | One line for pickers / catalogs. |
| `instructions` | Markdown the agent follows when the skill is active. |
| `pool` | Optional folder id (e.g. `navi`). Empty = root catalog. |
| `allow_tools` | Hint / focused authoring list. Does **not** lock the root session by itself. |
| `deny_tools` | Optional extra denylist metadata. |
| `tags` / `requires` | Optional metadata. `requires` lists skill ids for harness chains. |
| `scope` | `user` (shared Desktop + TUI) or `project` (this repo only). |
| `harness` | When `true`, NAVI materializes a harness pack (`loop.toml` + `graph.toml`). |

## Tool policy rules (important)

1. **Catalog / discovery never soft-locks tools.** Builtins like this one may list
   `allow_tools` for documentation; that does **not** restrict the main session.
2. **Soft session allowlist** applies only when a **harness** skill is
   **session-active** (`harness: true` and/or a materialized pack under
   `{data_dir}/harnesses/<id>/` with entry-node `allow_tools`).
3. Host security (permission mode, path guards) still applies on top.
4. For authoring skills, keep recommended `allow_tools` tight — only what that job needs.
5. **Never** browse `{data_dir}` with `search` / `bash` / raw `read_file` for skills —
   use `skill_list` / `skill_get` / `load_skill` / `skill_save`. Private storage is jailed.

## Harness skills

A **harness** is a multi-step, multi-skill workflow that NAVI materializes into a
pack under `{data_dir}/harnesses/<skill-id>/`. When you create a harness:

1. Ask the user whether this is a **single skill** or a **harness (multi-node workflow)**.
2. If harness:
   - Set `harness = true` in `skill_save`.
   - Use `requires` to declare the ordered chain of sub-skills (e.g. `analyst, ideator, critic`).
   - Ensure each sub-skill already exists via `skill_save` first (or create them as part of the same turn).
   - After saving the main harness skill, NAVI materializes `loop.toml` and `graph.toml` automatically.
   - Soft graph `allow_tools` and loop caps apply only when that harness skill is session-active
     (CLI `--skill` / host activate / config), not merely because it is installed.
3. Hard graph edge execution is still MVP-soft — do not promise automatic routing between nodes.

Also load `navi-harness-author` (pool `navi`) for pack layout details.

## Workflow

1. Clarify the job: when should this skill activate? What must the agent do / not do?
2. If the skill belongs to a product area (e.g. NAVI), `skill_list` that pool first.
3. If ambiguous, use `question` before saving.
4. Draft a **template** (below) with the user.
5. Choose a **minimal** recommended `allow_tools` list from real tool names.
6. Call `skill_save` with the structured fields (set `pool` when appropriate). For harnesses, set `harness: true`.
7. Call `skill_get` to verify; offer to refine.

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

Use **`skill_save`** with JSON fields:

```json
{
  "name": "…",
  "description": "…",
  "instructions": "…",
  "allow_tools": ["read_file", "…"],
  "tags": ["…"],
  "pool": "navi",
  "scope": "user",
  "harness": false
}
```

- **`skill_list`** — catalog (no pool) or open a pool (`pool` set).
- **`skill_get`** / **`load_skill`** — full body.
- **`skill_delete`** — only if the user confirms removing a skill.

## Anti-patterns

- Do **not** dump every skill into the root catalog when a pool fits better.
- Do **not** write skills into random config trees outside `data_dir/skills` or `.navi/skills`.
- Do **not** grant `bash` / `edit` / `write_file` unless the skill truly needs them.
- Do **not** save empty instructions or empty names.
- Do **not** set `harness = true` without defining `requires` or writing a clear multi-step procedure; a vague harness is just a slow prompt.
- Do **not** assume marketplace skills match the current engine harness API — engine essentials stay builtin.
"#;

const HARNESS_AUTHOR_INSTRUCTIONS: &str = r#"# Author a NAVI Harness Pack

Teach the user (and yourself) how harness packs work on this engine version.

## Two activation paths

| Path | What happens |
|------|----------------|
| **CLI / install** | `navi skill install` / `skill_save` with `harness: true` → materialize under `{data_dir}/harnesses/<id>/`. Soft apply when the skill is **session-active**. |
| **Chat** | User says "use the design harness" / "roda o design-loop" → activate that skill id for the session; model may `load_skill` + `skill_save` without dumping `graph.toml` by hand. |

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

1. Create leaf skills with `skill_save` (focused instructions + recommended tools).
2. Create the main skill with `harness: true` and `requires: [leaf ids…]`.
3. Confirm pack path in the `skill_save` response (`harness_pack`) or via `navi harness show <id>`.
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

Do **not** use `search`, `bash`, or raw filesystem tools on `{data_dir}` (sessions, memory, skills store). That path is jailed. Browse and mutate skills only through skill tools.

Project `.navi/skills` is user-authored project scope — still prefer skill tools over ad-hoc shell.
"#;
