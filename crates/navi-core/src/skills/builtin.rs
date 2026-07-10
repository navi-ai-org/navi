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
            "Author a durable NAVI skill in the local skill database with the right tools and instructions."
                .into(),
        ),
        version: Some("1.0.0".into()),
        author: Some("NAVI".into()),
        tags: vec![
            "navi".into(),
            "builtin".into(),
            "skills".into(),
            "authoring".into(),
        ],
        requires: vec![],
        // Tools needed to invent and persist skills — not general coding tools.
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
        path: PathBuf::from("builtin:navi-create-skill"),
        source: SkillSource::Builtin,
        scope: SkillWriteScope::User,
        instructions: CREATE_SKILL_INSTRUCTIONS.into(),
    }
}

const CREATE_SKILL_INSTRUCTIONS: &str = r#"# Create a NAVI Skill

You help the user design and **save** a durable skill into NAVI’s skill database
(`skills.sqlite` under the data dir). Skills are **not** free-form MD files in
`~/.config/navi`. They are records with instructions **and** a tool policy.

## What a skill is

| Field | Purpose |
|-------|---------|
| `id` | Stable slug (e.g. `code-reviewer`). Optional — derived from name. |
| `name` | Human title. |
| `description` | One line for pickers / catalogs. |
| `instructions` | Markdown the agent follows when the skill is active. |
| `allow_tools` | **Required for focused skills.** Only these tools are offered to the model while this skill is active (intersection if several skills set allow lists). |
| `deny_tools` | Optional extra denylist. |
| `tags` / `requires` | Optional metadata. |
| `scope` | `user` (shared Desktop + TUI) or `project` (this repo only). Prefer `user` unless the user asks otherwise. |

## Tool policy rules

1. A skill that only injects prose without `allow_tools` does **not** lock tools.
2. If **any** active skill sets non-empty `allow_tools`, the session tool set is the **intersection** of those lists.
3. Host security (permission mode, path guards) still applies on top.
4. For authoring skills, keep `allow_tools` tight — only what that job needs.

## Workflow

1. Clarify the job: when should this skill activate? What must the agent do / not do?
2. Draft a **template** (below) with the user.
3. Choose a **minimal** `allow_tools` list from real tool names (e.g. `read_file`, `bash`, `apply_patch`, `skill_save`).
4. Call `skill_save` with the structured fields.
5. Call `skill_get` to verify; offer to refine.

If anything is ambiguous, use `question` before saving.

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
  "scope": "user"
}
```

Use **`skill_list`** / **`skill_get`** to inspect existing skills.
Use **`skill_delete`** only if the user confirms removing a skill.

## Anti-patterns

- Do **not** write `SKILL.md` into the user’s config tree.
- Do **not** grant `bash` / `apply_patch` unless the skill truly needs them.
- Do **not** save empty instructions or empty names.
"#;
