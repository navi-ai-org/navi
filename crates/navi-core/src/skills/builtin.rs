//! Built-in NAVI skills shipped with the engine (not stored in SQLite).

use super::{SkillManifest, SkillSource, SkillWriteScope};
use std::path::PathBuf;

/// Id of the skill that teaches NAVI how to author other skills.
pub const CREATE_SKILL_ID: &str = "navi-create-skill";

/// Returns all built-in skills.
pub fn builtin_skills() -> Vec<SkillManifest> {
    vec![create_skill_manifest()]
}

fn create_skill_manifest() -> SkillManifest {
    SkillManifest {
        id: CREATE_SKILL_ID.into(),
        name: "Create NAVI Skill".into(),
        description: Some(
            "Author a durable NAVI skill as markdown on disk (optionally inside a skill pool)."
                .into(),
        ),
        version: Some("1.2.0".into()),
        author: Some("NAVI".into()),
        tags: vec![
            "navi".into(),
            "builtin".into(),
            "skills".into(),
            "authoring".into(),
            "harness".into(),
        ],
        requires: vec![],
        allow_tools: vec![
            "skill_list".into(),
            "skill_get".into(),
            "skill_save".into(),
            "skill_delete".into(),
            "load_skill".into(),
            "question".into(),
            "read_file".into(),
        ],
        deny_tools: vec![],
        harness: false,
        // Lives in the `navi` pool so the model opens the pool first instead of
        // seeing every authoring skill at the top-level catalog.
        pool: Some("navi".into()),
        path: PathBuf::from("builtin:navi/navi-create-skill"),
        source: SkillSource::Builtin,
        scope: SkillWriteScope::User,
        instructions: CREATE_SKILL_INSTRUCTIONS.into(),
    }
}

const CREATE_SKILL_INSTRUCTIONS: &str = r#"# Create a NAVI Skill

You help the user design and **save** a durable skill as markdown on disk.

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
   inside a folder; ommit `pool` for root-level skills.

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
| `allow_tools` | **Required for focused skills.** Only these tools while the skill is active. |
| `deny_tools` | Optional extra denylist. |
| `tags` / `requires` | Optional metadata. `requires` lists skill ids for harness chains. |
| `scope` | `user` (shared Desktop + TUI) or `project` (this repo only). |
| `harness` | When `true`, NAVI materializes a harness pack (`loop.toml` + `graph.toml`). |

## Tool policy rules

1. A skill that only injects prose without `allow_tools` does **not** lock tools.
2. If **any** active skill sets non-empty `allow_tools`, the session tool set is the **intersection** of those lists.
3. Host security (permission mode, path guards) still applies on top.
4. For authoring skills, keep `allow_tools` tight — only what that job needs.

## Harness skills

A **harness** is a multi-step, multi-skill workflow that NAVI materializes into a
pack under `{data_dir}/harnesses/<skill-id>/`. When you create a harness:

1. Ask the user whether this is a **single skill** or a **harness (multi-node workflow)**.
2. If harness:
   - Set `harness = true` in `skill_save`.
   - Use `requires` to declare the ordered chain of sub-skills (e.g. `analyst, ideator, critic`).
   - Ensure each sub-skill already exists via `skill_save` first (or create them as part of the same turn).
   - After saving the main harness skill, call `skill_save` with `harness = true`. NAVI will materialize `loop.toml` and `graph.toml` automatically.
   - Verify with `navi harness show <id>` or by reading `{data_dir}/harnesses/<id>/graph.toml`.

## Workflow

1. Clarify the job: when should this skill activate? What must the agent do / not do?
2. If the skill belongs to a product area (e.g. NAVI), `skill_list` that pool first.
3. If ambiguous, use `question` before saving.
4. Draft a **template** (below) with the user.
5. Choose a **minimal** `allow_tools` list from real tool names.
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
"#;
