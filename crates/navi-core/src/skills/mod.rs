mod builtin;
mod store;

pub use builtin::{CREATE_SKILL_ID, builtin_skills};
pub use store::SkillStore;

use crate::config::SkillsConfig;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Origin of a skill record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SkillSource {
    /// Shipped with the engine binary.
    Builtin,
    /// Stored in `data_dir/skills.sqlite` (canonical user store).
    #[default]
    Store,
}

/// A discovered skill (SQLite store or builtin).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillManifest {
    /// Unique skill identifier.
    pub id: String,
    /// Human-readable skill name.
    pub name: String,
    /// Optional description for pickers / catalogs.
    pub description: Option<String>,
    /// Optional version string (e.g. "1.0.0").
    pub version: Option<String>,
    /// Optional author or maintainer.
    pub author: Option<String>,
    /// Tags for categorization and filtering.
    pub tags: Vec<String>,
    /// Skill ids that must be active for this skill to work.
    pub requires: Vec<String>,
    /// When non-empty and skill is active, only these tools are exposed (intersection across skills).
    #[serde(default)]
    pub allow_tools: Vec<String>,
    /// Tools to hide while this skill is active.
    #[serde(default)]
    pub deny_tools: Vec<String>,
    /// When true, this skill is treated as a harness and materialized into a pack.
    #[serde(default)]
    pub harness: bool,
    /// Path to the SQLite store or `builtin:…` marker.
    pub path: PathBuf,
    /// Instruction body when the skill is active.
    pub instructions: String,
    /// Where the skill was loaded from.
    #[serde(default)]
    pub source: SkillSource,
    /// User vs project scope (store / writes).
    #[serde(default)]
    pub scope: SkillWriteScope,
}

/// Discovers skills: builtins + SQLite store only.
pub fn discover_configured_skills(
    config: &SkillsConfig,
    project_dir: &Path,
    data_dir: &Path,
) -> Result<Vec<SkillManifest>> {
    if !config.enabled {
        return Ok(Vec::new());
    }

    let mut skills = builtin_skills();

    if let Ok(store) = SkillStore::open(data_dir) {
        let project_key = project_skill_key(project_dir);
        match store.list_for_discovery(Some(&project_key)) {
            Ok(stored) => skills.extend(stored),
            Err(err) => tracing::warn!(error = %err, "failed to list skills from store"),
        }
    }

    skills.sort_by(|a, b| a.id.cmp(&b.id));
    skills.dedup_by(|a, b| a.id == b.id);
    Ok(skills)
}

/// Stable project key for project-scoped store rows.
pub fn project_skill_key(project_dir: &Path) -> String {
    let canon = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());
    // Short hash-like key from path (no extra deps).
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    canon.to_string_lossy().hash(&mut h);
    format!("{:x}", h.finish())
}

/// Compute the tool allowlist from active skill policies.
///
/// - If no active skill sets `allow_tools`, returns `None` (no skill-based filter).
/// - Otherwise returns the **intersection** of all non-empty `allow_tools` lists,
///   minus any `deny_tools` from active skills.
pub fn skill_tool_allowlist(active: &[SkillManifest]) -> Option<Vec<String>> {
    let with_allow: Vec<&SkillManifest> = active
        .iter()
        .filter(|s| !s.allow_tools.is_empty())
        .collect();
    if with_allow.is_empty() {
        return None;
    }
    let mut set: HashSet<String> = with_allow[0].allow_tools.iter().cloned().collect();
    for skill in with_allow.iter().skip(1) {
        set.retain(|t| skill.allow_tools.iter().any(|a| a == t));
    }
    for skill in active {
        for deny in &skill.deny_tools {
            set.remove(deny);
        }
    }
    let mut list: Vec<String> = set.into_iter().collect();
    list.sort();
    Some(list)
}

/// Filters discovered skills to only those that are explicitly active in config
/// or included in the `active` list.
pub fn active_skills(
    available: &[SkillManifest],
    configured_active: &[String],
    session_active: &[String],
) -> Vec<SkillManifest> {
    let requested = if session_active.is_empty() {
        configured_active
    } else {
        session_active
    };
    if requested.is_empty() {
        return Vec::new();
    }

    available
        .iter()
        .filter(|skill| {
            requested
                .iter()
                .any(|name| name == &skill.id || name == &skill.name)
        })
        .cloned()
        .collect()
}

/// Renders active skills into a text block for injection into the system prompt.
/// Returns `None` if there are no active skills.
pub fn render_active_skills(skills: &[SkillManifest]) -> Option<String> {
    if skills.is_empty() {
        return None;
    }

    let mut output = String::from("=== Active Skills ===\n");
    for skill in skills {
        output.push_str(&format!("- id: {}; name: {}\n", skill.id, skill.name));
        if let Some(description) = &skill.description {
            output.push_str(&format!("  description: {}\n", description.trim()));
        }
        if let Some(version) = &skill.version {
            output.push_str(&format!("  version: {}\n", version));
        }
        if let Some(author) = &skill.author {
            output.push_str(&format!("  author: {}\n", author));
        }
        if !skill.tags.is_empty() {
            output.push_str(&format!("  tags: {}\n", skill.tags.join(", ")));
        }
        if !skill.requires.is_empty() {
            output.push_str(&format!("  requires: {}\n", skill.requires.join(", ")));
        }
        output.push_str(skill.instructions.trim());
        output.push_str("\n\n");
    }
    Some(output)
}

/// Renders a catalog of available skills without exposing their instruction text.
/// Returns `None` if there are no available skills.
pub fn render_available_skills(skills: &[SkillManifest]) -> Option<String> {
    if skills.is_empty() {
        return None;
    }

    let mut output = String::from(
        "=== Available Skills ===\nThese skills are available. Use the `load_skill` tool with a skill id when you decide a skill is relevant. The instruction text is not included here.\n",
    );
    for skill in skills {
        output.push_str(&format!("- id: {}; name: {}\n", skill.id, skill.name));
        if let Some(description) = &skill.description {
            output.push_str(&format!("  description: {}\n", description.trim()));
        }
        if let Some(version) = &skill.version {
            output.push_str(&format!("  version: {}\n", version));
        }
        if let Some(author) = &skill.author {
            output.push_str(&format!("  author: {}\n", author));
        }
        if !skill.tags.is_empty() {
            output.push_str(&format!("  tags: {}\n", skill.tags.join(", ")));
        }
        if !skill.requires.is_empty() {
            output.push_str(&format!("  requires: {}\n", skill.requires.join(", ")));
        }
    }
    Some(output)
}

/// Where a user-authored skill is stored (SQLite scope).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SkillWriteScope {
    /// User skill in `data_dir/skills.sqlite` — shared across Desktop + TUI.
    #[default]
    User,
    /// Project-scoped row in the same database (keyed by project path hash).
    Project,
}

/// Payload for creating or updating a skill in the SQLite skill store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillWriteRequest {
    /// Skill id. If empty, derived from `name` via [`slugify_skill_id`].
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    /// Tools available while this skill is active (empty = no skill tool lock).
    #[serde(default)]
    pub allow_tools: Vec<String>,
    #[serde(default)]
    pub deny_tools: Vec<String>,
    /// When true, materialize a harness pack for this skill after saving.
    #[serde(default)]
    pub harness: bool,
    /// Markdown body (instructions). Required non-empty after trim.
    pub instructions: String,
    #[serde(default)]
    pub scope: SkillWriteScope,
}

/// Result of writing a skill to disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillWriteResult {
    pub skill: SkillManifest,
    pub path: PathBuf,
    pub created: bool,
}

/// Resolve a validated skill id from a write request.
pub fn resolve_skill_id(request: &SkillWriteRequest) -> Result<String> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err(anyhow::anyhow!("skill name is required"));
    }
    let id = {
        let raw = request.id.trim();
        if raw.is_empty() {
            slugify_skill_id(name)
        } else {
            slugify_skill_id(raw)
        }
    };
    if id.is_empty() || id == "." || id == ".." || id.contains('/') || id.contains('\\') {
        return Err(anyhow::anyhow!("invalid skill id"));
    }
    Ok(id)
}

/// Normalize a free-form name into a stable skill directory id.
pub fn slugify_skill_id(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in raw.trim().chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if matches!(c, '-' | '_' | ' ' | '/' | '.') && !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { "skill".into() } else { out }
}

/// Create or update a skill in the SQLite skill store (shared Desktop + TUI).
pub fn write_skill(
    request: &SkillWriteRequest,
    project_dir: &Path,
    data_dir: &Path,
) -> Result<SkillWriteResult> {
    let store = SkillStore::open(data_dir)?;
    let project_key = match request.scope {
        SkillWriteScope::Project => Some(project_skill_key(project_dir)),
        SkillWriteScope::User => None,
    };
    store.upsert(request, project_key.as_deref())
}

/// Load full skill content (including instructions) by id from discovered skills.
pub fn load_skill_by_id(
    config: &SkillsConfig,
    project_dir: &Path,
    data_dir: &Path,
    skill_id: &str,
) -> Result<SkillManifest> {
    let skills = discover_configured_skills(config, project_dir, data_dir)?;
    skills
        .into_iter()
        .find(|s| s.id == skill_id || s.name == skill_id)
        .ok_or_else(|| anyhow::anyhow!("skill `{skill_id}` not found"))
}

/// Delete a skill from the SQLite store (never deletes builtins).
pub fn delete_skill(skill_id: &str, _project_dir: &Path, data_dir: &Path) -> Result<bool> {
    let id = slugify_skill_id(skill_id);
    if id.is_empty() {
        return Err(anyhow::anyhow!("invalid skill id"));
    }
    if builtin_skills().iter().any(|s| s.id == id) {
        return Err(anyhow::anyhow!("cannot delete built-in skill `{id}`"));
    }
    let store = SkillStore::open(data_dir)?;
    store.delete(&id)
}

/// Whether a skill can be edited/deleted from the UI (store-backed, not builtin).
pub fn skill_is_editable(skill: &SkillManifest) -> bool {
    matches!(skill.source, SkillSource::Store)
}

/// Fields extracted from a skill markdown or TOML file before write.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedSkillFile {
    pub id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub tags: Vec<String>,
    pub allow_tools: Vec<String>,
    pub deny_tools: Vec<String>,
    pub harness: bool,
    pub instructions: String,
}

/// Parse a skill file by path extension (`.md` / `.markdown` / `.toml`).
///
/// When `name` is missing from the file, uses `fallback_name` (typically the file stem).
pub fn parse_skill_file(path: &Path, raw: &str, fallback_name: &str) -> Result<ParsedSkillFile> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" => Ok(parse_skill_md(raw, fallback_name)),
        "toml" => parse_skill_toml(raw, fallback_name),
        other => Err(anyhow::anyhow!(
            "unsupported skill file extension '.{other}' (expected .md, .markdown, or .toml)"
        )),
    }
}

/// Parse skill markdown with optional YAML-ish frontmatter between `---` fences.
///
/// Supported frontmatter keys: `name`, `description`, `version`, `tags`, `id`,
/// `author`, `allow_tools`, `deny_tools`, `harness`. Body after the closing `---` is instructions.
pub fn parse_skill_md(raw: &str, fallback_name: &str) -> ParsedSkillFile {
    let trimmed = raw.trim_start();
    if let Some(rest) = trimmed.strip_prefix("---") {
        // Allow optional whitespace/newlines after opening fence.
        let rest = rest.strip_prefix('\r').unwrap_or(rest);
        let rest = rest.strip_prefix('\n').unwrap_or(rest);
        if let Some(end) = rest.find("\n---") {
            let front = &rest[..end];
            let body = rest[end + 4..]
                .trim_start_matches('\r')
                .trim_start_matches('\n')
                .to_string();
            let mut parsed = ParsedSkillFile {
                instructions: body,
                harness: false,
                ..Default::default()
            };
            for line in front.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let Some((key, value)) = split_yaml_key_value(line) else {
                    continue;
                };
                match key {
                    "name" => parsed.name = unquote(value),
                    "description" => {
                        let v = unquote(value);
                        if !v.is_empty() {
                            parsed.description = Some(v);
                        }
                    }
                    "version" => {
                        let v = unquote(value);
                        if !v.is_empty() {
                            parsed.version = Some(v);
                        }
                    }
                    "author" => {
                        let v = unquote(value);
                        if !v.is_empty() {
                            parsed.author = Some(v);
                        }
                    }
                    "id" => {
                        let v = unquote(value);
                        if !v.is_empty() {
                            parsed.id = Some(v);
                        }
                    }
                    "tags" => parsed.tags = parse_yaml_list(value),
                    "allow_tools" => parsed.allow_tools = parse_yaml_list(value),
                    "deny_tools" => parsed.deny_tools = parse_yaml_list(value),
                    "harness" => parsed.harness = parse_yaml_bool(value),
                    _ => {}
                }
            }
            if parsed.name.trim().is_empty() {
                parsed.name = fallback_name.trim().to_string();
            }
            if parsed.name.trim().is_empty() {
                parsed.name = "Imported Skill".into();
            }
            return parsed;
        }
    }

    let mut name = fallback_name.trim().to_string();
    if name.is_empty() {
        name = "Imported Skill".into();
    }
    ParsedSkillFile {
        name,
        instructions: raw.to_string(),
        ..Default::default()
    }
}

/// Parse a skill TOML file (`name`, `description`, `version`, `tags`, `instructions`, …).
pub fn parse_skill_toml(raw: &str, fallback_name: &str) -> Result<ParsedSkillFile> {
    #[derive(Deserialize)]
    struct SkillFile {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        author: Option<String>,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        allow_tools: Vec<String>,
        #[serde(default)]
        deny_tools: Vec<String>,
        #[serde(default)]
        harness: bool,
        #[serde(default)]
        instructions: String,
    }
    let file: SkillFile =
        toml::from_str(raw).map_err(|e| anyhow::anyhow!("failed to parse skill TOML: {e}"))?;
    let mut name = file
        .name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback_name.trim().to_string());
    if name.is_empty() {
        name = "Imported Skill".into();
    }
    Ok(ParsedSkillFile {
        id: file
            .id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        name,
        description: file
            .description
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        version: file
            .version
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        author: file
            .author
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        tags: file.tags,
        allow_tools: file.allow_tools,
        deny_tools: file.deny_tools,
        harness: file.harness,
        instructions: file.instructions,
    })
}

/// List builtins + store skills regardless of `[skills].enabled`.
///
/// Used by `navi skill list` so the store remains inspectable when discovery is off.
pub fn list_installed_skills(project_dir: &Path, data_dir: &Path) -> Result<Vec<SkillManifest>> {
    let mut skills = builtin_skills();
    if let Ok(store) = SkillStore::open(data_dir) {
        let project_key = project_skill_key(project_dir);
        match store.list_for_discovery(Some(&project_key)) {
            Ok(stored) => skills.extend(stored),
            Err(err) => tracing::warn!(error = %err, "failed to list skills from store"),
        }
    }
    skills.sort_by(|a, b| a.id.cmp(&b.id));
    skills.dedup_by(|a, b| a.id == b.id);
    Ok(skills)
}

fn split_yaml_key_value(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once(':')?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    Some((key, value.trim()))
}

fn unquote(value: &str) -> String {
    let v = value.trim();
    if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
        || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
    {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

/// Parse a simple YAML list value: `[a, b]`, comma-separated, or a single token.
fn parse_yaml_list(value: &str) -> Vec<String> {
    let t = value.trim();
    if t.is_empty() {
        return Vec::new();
    }
    if let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return inner
            .split(',')
            .map(|s| unquote(s.trim()))
            .filter(|s| !s.is_empty())
            .collect();
    }
    t.split(',')
        .map(|s| unquote(s.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

fn parse_yaml_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "yes" | "1" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SkillsConfig {
        SkillsConfig {
            enabled: true,
            active: Vec::new(),
        }
    }

    #[test]
    fn discovers_builtin_create_skill() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let skills =
            discover_configured_skills(&cfg(), tempdir.path(), tempdir.path()).expect("skills");
        assert!(skills.iter().any(|s| s.id == CREATE_SKILL_ID));
        let create = skills.iter().find(|s| s.id == CREATE_SKILL_ID).unwrap();
        assert!(!create.allow_tools.is_empty());
        assert!(create.allow_tools.iter().any(|t| t == "skill_save"));
    }

    #[test]
    fn write_and_discover_store_skill() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let data = tempdir.path().join("data");
        let project = tempdir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();

        let result = write_skill(
            &SkillWriteRequest {
                id: String::new(),
                name: "My Helper".into(),
                description: Some("Helps with X".into()),
                version: None,
                author: None,
                tags: vec!["util".into()],
                requires: vec![],
                allow_tools: vec!["read_file".into()],
                deny_tools: vec![],
                harness: false,
                instructions: "Do the thing carefully.".into(),
                scope: SkillWriteScope::User,
            },
            &project,
            &data,
        )
        .expect("write");
        assert!(result.created);
        assert_eq!(result.skill.id, "my-helper");
        assert_eq!(result.skill.source, SkillSource::Store);

        let skills = discover_configured_skills(&cfg(), &project, &data).expect("discover");
        assert!(skills.iter().any(|s| s.id == "my-helper"));
        assert!(skills.iter().any(|s| s.id == CREATE_SKILL_ID));
    }

    #[test]
    fn write_roundtrip_preserves_tool_policy() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        write_skill(
            &SkillWriteRequest {
                id: "reviewer".into(),
                name: "Code Reviewer".into(),
                description: Some("Reviews PRs".into()),
                version: Some("1.0.0".into()),
                author: Some("NAVI".into()),
                tags: vec!["code".into(), "review".into()],
                requires: vec!["socratic".into()],
                allow_tools: vec!["read_file".into(), "bash".into()],
                deny_tools: vec![],
                harness: false,
                instructions: "Review thoroughly.".into(),
                scope: SkillWriteScope::User,
            },
            tempdir.path(),
            tempdir.path(),
        )
        .expect("write");
        let loaded =
            load_skill_by_id(&cfg(), tempdir.path(), tempdir.path(), "reviewer").expect("load");
        assert_eq!(loaded.name, "Code Reviewer");
        assert_eq!(loaded.allow_tools, vec!["read_file", "bash"]);
        assert_eq!(loaded.requires, vec!["socratic"]);
    }

    #[test]
    fn skill_tool_allowlist_intersects() {
        let a = SkillManifest {
            id: "a".into(),
            name: "A".into(),
            description: None,
            version: None,
            author: None,
            tags: vec![],
            requires: vec![],
            allow_tools: vec!["read_file".into(), "bash".into()],
            deny_tools: vec![],
            harness: false,
            path: PathBuf::from("a"),
            instructions: "a".into(),
            source: SkillSource::Store,
            scope: SkillWriteScope::User,
        };
        let b = SkillManifest {
            id: "b".into(),
            name: "B".into(),
            description: None,
            version: None,
            author: None,
            tags: vec![],
            requires: vec![],
            allow_tools: vec!["read_file".into(), "skill_save".into()],
            deny_tools: vec![],
            harness: false,
            path: PathBuf::from("b"),
            instructions: "b".into(),
            source: SkillSource::Store,
            scope: SkillWriteScope::User,
        };
        assert_eq!(skill_tool_allowlist(&[a, b]).unwrap(), vec!["read_file"]);
    }

    #[test]
    fn cannot_delete_builtin() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let err = delete_skill(CREATE_SKILL_ID, tempdir.path(), tempdir.path()).unwrap_err();
        assert!(err.to_string().contains("built-in"));
    }

    #[test]
    fn returns_empty_when_disabled() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        write_skill(
            &SkillWriteRequest {
                id: "x".into(),
                name: "X".into(),
                description: None,
                version: None,
                author: None,
                tags: vec![],
                requires: vec![],
                allow_tools: vec![],
                deny_tools: vec![],
                harness: false,
                instructions: "body".into(),
                scope: SkillWriteScope::User,
            },
            tempdir.path(),
            tempdir.path(),
        )
        .unwrap();
        let config = SkillsConfig {
            enabled: false,
            active: Vec::new(),
        };
        let skills =
            discover_configured_skills(&config, tempdir.path(), tempdir.path()).expect("skills");
        assert!(skills.is_empty());
    }

    #[test]
    fn active_skills_match_by_id() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        write_skill(
            &SkillWriteRequest {
                id: "socratic".into(),
                name: "Socratic".into(),
                description: None,
                version: None,
                author: None,
                tags: vec![],
                requires: vec![],
                allow_tools: vec![],
                deny_tools: vec![],
                harness: false,
                instructions: "Ask one question first.".into(),
                scope: SkillWriteScope::User,
            },
            tempdir.path(),
            tempdir.path(),
        )
        .unwrap();
        let skills =
            discover_configured_skills(&cfg(), tempdir.path(), tempdir.path()).expect("skills");
        let active = active_skills(&skills, &["socratic".into()], &[]);
        assert_eq!(active.len(), 1);
        let rendered = render_active_skills(&active).unwrap();
        assert!(rendered.contains("Ask one question first."));
    }

    #[test]
    fn parse_skill_md_frontmatter() {
        let raw = r#"---
name: Code Reviewer
description: Reviews PRs carefully
version: "1.0.0"
id: reviewer
author: NAVI
tags: [code, review]
allow_tools: [read_file, bash]
deny_tools: write_file
---
Review thoroughly.
Use checklists.
"#;
        let parsed = parse_skill_md(raw, "fallback");
        assert_eq!(parsed.name, "Code Reviewer");
        assert_eq!(parsed.id.as_deref(), Some("reviewer"));
        assert_eq!(parsed.description.as_deref(), Some("Reviews PRs carefully"));
        assert_eq!(parsed.version.as_deref(), Some("1.0.0"));
        assert_eq!(parsed.author.as_deref(), Some("NAVI"));
        assert_eq!(parsed.tags, vec!["code", "review"]);
        assert_eq!(parsed.allow_tools, vec!["read_file", "bash"]);
        assert_eq!(parsed.deny_tools, vec!["write_file"]);
        assert!(parsed.instructions.contains("Review thoroughly."));
        assert!(parsed.instructions.contains("Use checklists."));
    }

    #[test]
    fn parse_skill_md_uses_fallback_name_without_frontmatter() {
        let parsed = parse_skill_md("Just do the thing.", "my-skill");
        assert_eq!(parsed.name, "my-skill");
        assert_eq!(parsed.instructions, "Just do the thing.");
    }

    #[test]
    fn parse_skill_toml_and_install_roundtrip() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let data = tempdir.path().join("data");
        let project = tempdir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();

        let raw = r#"
name = "Helper"
description = "Helps"
version = "0.1.0"
id = "helper"
tags = ["util"]
allow_tools = ["read_file"]
instructions = "Help carefully."
"#;
        let path = PathBuf::from("helper.toml");
        let parsed = parse_skill_file(&path, raw, "stem").expect("parse");
        assert_eq!(parsed.name, "Helper");
        assert_eq!(parsed.id.as_deref(), Some("helper"));

        let result = write_skill(
            &SkillWriteRequest {
                id: parsed.id.clone().unwrap_or_default(),
                name: parsed.name.clone(),
                description: parsed.description.clone(),
                version: parsed.version.clone(),
                author: parsed.author.clone(),
                tags: parsed.tags.clone(),
                requires: vec![],
                allow_tools: parsed.allow_tools.clone(),
                deny_tools: parsed.deny_tools.clone(),
                harness: false,
                instructions: parsed.instructions.clone(),
                scope: SkillWriteScope::User,
            },
            &project,
            &data,
        )
        .expect("write");
        assert!(result.created);
        assert_eq!(result.skill.id, "helper");

        let listed = list_installed_skills(&project, &data).expect("list");
        assert!(listed.iter().any(|s| s.id == "helper"));
        assert!(listed.iter().any(|s| s.id == CREATE_SKILL_ID));
    }

    #[test]
    fn list_installed_skills_ignores_enabled_flag() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        write_skill(
            &SkillWriteRequest {
                id: "listed".into(),
                name: "Listed".into(),
                description: None,
                version: None,
                author: None,
                tags: vec![],
                requires: vec![],
                allow_tools: vec![],
                deny_tools: vec![],
                harness: false,
                instructions: "body".into(),
                scope: SkillWriteScope::User,
            },
            tempdir.path(),
            tempdir.path(),
        )
        .unwrap();
        let disabled = SkillsConfig {
            enabled: false,
            active: Vec::new(),
        };
        let via_discover =
            discover_configured_skills(&disabled, tempdir.path(), tempdir.path()).unwrap();
        assert!(via_discover.is_empty());
        let listed = list_installed_skills(tempdir.path(), tempdir.path()).unwrap();
        assert!(listed.iter().any(|s| s.id == "listed"));
    }
}
