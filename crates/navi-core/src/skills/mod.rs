mod builtin;
mod store;

pub use builtin::{CREATE_SKILL_ID, builtin_skills};
pub use store::SkillStore;

use crate::config::SkillsConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Origin of a skill record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SkillSource {
    /// Shipped with the engine binary.
    Builtin,
    /// Stored in `data_dir/skills.sqlite`.
    Store,
    /// Legacy filesystem `SKILL.md` discovery.
    #[default]
    File,
}

/// A discovered skill (SQLite, builtin, or legacy SKILL.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillManifest {
    /// Unique skill identifier (derived from the directory name).
    pub id: String,
    /// Human-readable skill name.
    pub name: String,
    /// Optional description extracted from the skill header.
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
    /// Path to the skill directory, SQLite file, or `builtin:…` marker.
    pub path: PathBuf,
    /// The skill's instruction text (body of `SKILL.md` or store body).
    pub instructions: String,
    /// Where the skill was loaded from.
    #[serde(default)]
    pub source: SkillSource,
    /// User vs project scope (store / writes).
    #[serde(default)]
    pub scope: SkillWriteScope,
}

/// Discovers skills: builtins + SQLite store + legacy filesystem `SKILL.md`.
pub fn discover_configured_skills(
    config: &SkillsConfig,
    project_dir: &Path,
    data_dir: &Path,
) -> Result<Vec<SkillManifest>> {
    if !config.enabled {
        return Ok(Vec::new());
    }

    let mut skills = builtin_skills();

    // Primary: SQLite skill database (shared Desktop + TUI).
    if let Ok(store) = SkillStore::open(data_dir) {
        let project_key = project_skill_key(project_dir);
        match store.list_for_discovery(Some(&project_key)) {
            Ok(stored) => skills.extend(stored),
            Err(err) => tracing::warn!(error = %err, "failed to list skills from store"),
        }
    }

    // Legacy: filesystem SKILL.md trees (retrocompat).
    for dir in skill_dirs(config, project_dir, data_dir) {
        if !dir.exists() {
            continue;
        }
        for entry in
            fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))?
        {
            let path = entry?.path();
            if path.is_dir() {
                let skill_md = path.join("SKILL.md");
                if skill_md.exists() {
                    if let Ok(mut m) = load_skill_dir(&path) {
                        m.source = SkillSource::File;
                        skills.push(m);
                    }
                }
            } else if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
                if let Ok(mut m) = load_skill_dir(&dir) {
                    m.source = SkillSource::File;
                    skills.push(m);
                }
            }
        }
    }

    // Prefer store/builtin over legacy file with the same id.
    skills.sort_by(|a, b| {
        let rank = |s: SkillSource| match s {
            SkillSource::Builtin => 0,
            SkillSource::Store => 1,
            SkillSource::File => 2,
        };
        rank(a.source)
            .cmp(&rank(b.source))
            .then_with(|| a.id.cmp(&b.id))
    });
    skills.dedup_by(|a, b| a.id == b.id);
    skills.sort_by(|a, b| a.id.cmp(&b.id));
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

fn skill_dirs(config: &SkillsConfig, project_dir: &Path, data_dir: &Path) -> Vec<PathBuf> {
    if !config.dirs.is_empty() {
        return config
            .dirs
            .iter()
            .map(|path| {
                if path.is_absolute() {
                    path.clone()
                } else {
                    project_dir.join(path)
                }
            })
            .collect();
    }

    vec![
        project_dir.join(".navi").join("skills"),
        data_dir.join("skills"),
    ]
}

fn load_skill_dir(dir: &Path) -> Result<SkillManifest> {
    let skill_path = dir.join("SKILL.md");
    let raw = fs::read_to_string(&skill_path)
        .with_context(|| format!("failed to read {}", skill_path.display()))?;
    let id = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("skill")
        .to_string();
    let (metadata, body) = split_frontmatter(&raw);
    let heading = first_heading(body);
    let name = metadata.name.or(heading).unwrap_or_else(|| id.clone());
    let description = metadata.description.or_else(|| first_plain_line(body));

    let mut allow_tools = metadata.allow_tools;
    let deny_tools = metadata.deny_tools;
    // Legacy frontmatter aliases.
    if allow_tools.is_empty() && !metadata.tools.is_empty() {
        allow_tools = metadata.tools;
    }
    let manifest = SkillManifest {
        id,
        name,
        description,
        version: metadata.version,
        author: metadata.author,
        tags: metadata.tags,
        requires: metadata.requires,
        allow_tools,
        deny_tools,
        path: skill_path,
        instructions: body.trim().to_string(),
        source: SkillSource::File,
        scope: SkillWriteScope::User,
    };
    validate_skill(&manifest)?;
    Ok(manifest)
}

fn validate_skill(manifest: &SkillManifest) -> Result<()> {
    if manifest.id.is_empty() {
        return Err(anyhow::anyhow!("skill directory name cannot be empty"));
    }
    if manifest.instructions.trim().is_empty() {
        return Err(anyhow::anyhow!(
            "skill '{}' has empty instructions in {}",
            manifest.id,
            manifest.path.display()
        ));
    }
    Ok(())
}

/// Where a user-authored skill is stored. Both scopes use the same `SKILL.md`
/// layout so TUI, CLI, Desktop, and N-API all discover them identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SkillWriteScope {
    /// `~/.local/share/navi/skills/<id>/` — shared across all NAVI frontends.
    #[default]
    User,
    /// `<project>/.navi/skills/<id>/` — project-local, still the standard format.
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
    if out.is_empty() {
        "skill".into()
    } else {
        out
    }
}

fn user_skills_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("skills")
}

fn project_skills_dir(project_dir: &Path) -> PathBuf {
    project_dir.join(".navi").join("skills")
}

/// Directory that holds skills for the given write scope.
pub fn skill_write_root(
    scope: SkillWriteScope,
    project_dir: &Path,
    data_dir: &Path,
) -> PathBuf {
    match scope {
        SkillWriteScope::User => user_skills_dir(data_dir),
        SkillWriteScope::Project => project_skills_dir(project_dir),
    }
}

/// Serialize a skill to the on-disk `SKILL.md` format (YAML frontmatter + body).
pub fn render_skill_md(request: &SkillWriteRequest) -> String {
    let mut fm = String::from("---\n");
    fm.push_str(&format!("name: {}\n", request.name.trim()));
    if let Some(d) = request.description.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        fm.push_str(&format!("description: {d}\n"));
    }
    if let Some(v) = request.version.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        fm.push_str(&format!("version: {v}\n"));
    }
    if let Some(a) = request.author.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        fm.push_str(&format!("author: {a}\n"));
    }
    if !request.tags.is_empty() {
        fm.push_str(&format!("tags: {}\n", request.tags.join(", ")));
    }
    if !request.requires.is_empty() {
        fm.push_str(&format!("requires: {}\n", request.requires.join(", ")));
    }
    if !request.allow_tools.is_empty() {
        fm.push_str(&format!("allow_tools: {}\n", request.allow_tools.join(", ")));
    }
    if !request.deny_tools.is_empty() {
        fm.push_str(&format!("deny_tools: {}\n", request.deny_tools.join(", ")));
    }
    fm.push_str("---\n\n");
    let body = request.instructions.trim();
    fm.push_str(body);
    if !body.ends_with('\n') {
        fm.push('\n');
    }
    fm
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
pub fn delete_skill(
    skill_id: &str,
    _project_dir: &Path,
    data_dir: &Path,
) -> Result<bool> {
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

#[derive(Default)]
struct SkillMetadata {
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
    author: Option<String>,
    tags: Vec<String>,
    requires: Vec<String>,
    allow_tools: Vec<String>,
    deny_tools: Vec<String>,
    tools: Vec<String>,
}

fn split_frontmatter(raw: &str) -> (SkillMetadata, &str) {
    let Some(rest) = raw.strip_prefix("---\n") else {
        return (SkillMetadata::default(), raw);
    };
    let Some(end) = rest.find("\n---\n") else {
        return (SkillMetadata::default(), raw);
    };
    let frontmatter = &rest[..end];
    let body = &rest[end + "\n---\n".len()..];
    let mut metadata = SkillMetadata::default();
    for line in frontmatter.lines() {
        if let Some((key, value)) = line.split_once(':') {
            let value = value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            match key.trim() {
                "name" => metadata.name = Some(value),
                "description" => metadata.description = Some(value),
                "version" => metadata.version = Some(value),
                "author" => metadata.author = Some(value),
                "tags" => {
                    metadata.tags = parse_csv_list(&value);
                }
                "requires" => {
                    metadata.requires = parse_csv_list(&value);
                }
                "allow_tools" | "allow-tools" => {
                    metadata.allow_tools = parse_csv_list(&value);
                }
                "deny_tools" | "deny-tools" => {
                    metadata.deny_tools = parse_csv_list(&value);
                }
                "tools" => {
                    metadata.tools = parse_csv_list(&value);
                }
                _ => {}
            }
        }
    }
    (metadata, body)
}

fn parse_csv_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn first_heading(body: &str) -> Option<String> {
    body.lines()
        .find_map(|line| line.trim().strip_prefix("# ").map(str::trim))
        .filter(|line| !line.is_empty())
        .map(str::to_string)
}

fn first_plain_line(body: &str) -> Option<String> {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_and_renders_active_skill() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let skill_dir = tempdir.path().join(".navi").join("skills").join("socratic");
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: Socratic\ndescription: Ask before answering\n---\n# Socratic\nAsk one question first.",
        )
        .expect("write skill");

        let config = SkillsConfig {
            enabled: true,
            dirs: Vec::new(),
            active: vec!["socratic".to_string()],
        };
        let skills =
            discover_configured_skills(&config, tempdir.path(), tempdir.path()).expect("skills");
        let file_skills: Vec<_> = skills
            .iter()
            .filter(|s| s.source == SkillSource::File)
            .collect();
        let active = active_skills(&skills, &config.active, &[]);
        let rendered = render_active_skills(&active).expect("rendered");

        assert_eq!(file_skills.len(), 1);
        assert!(skills.iter().any(|s| s.id == CREATE_SKILL_ID));
        assert!(rendered.contains("Ask one question first."));
    }

    #[test]
    fn parses_extended_frontmatter() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let skill_dir = tempdir.path().join(".navi").join("skills").join("reviewer");
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: Code Reviewer\ndescription: Reviews code quality\nversion: 1.0.0\nauthor: NAVI Team\ntags: code, review, quality\nrequires: socratic\n---\n# Code Reviewer\nReview code for quality.",
        )
        .expect("write skill");

        let config = SkillsConfig {
            enabled: true,
            dirs: Vec::new(),
            active: vec!["reviewer".to_string()],
        };
        let skills =
            discover_configured_skills(&config, tempdir.path(), tempdir.path()).expect("skills");

        let skill = skills
            .iter()
            .find(|s| s.id == "reviewer")
            .expect("reviewer skill");
        assert_eq!(skill.name, "Code Reviewer");
        assert_eq!(skill.version.as_deref(), Some("1.0.0"));
        assert_eq!(skill.author.as_deref(), Some("NAVI Team"));
        assert_eq!(skill.tags, vec!["code", "review", "quality"]);
        assert_eq!(skill.requires, vec!["socratic"]);
    }

    #[test]
    fn write_user_skill_is_discovered_by_both_scopes() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let data_dir = tempdir.path().join("data");
        let project_dir = tempdir.path().join("proj");
        fs::create_dir_all(&project_dir).expect("project");

        let result = write_skill(
            &SkillWriteRequest {
                id: String::new(),
                name: "My Helper".into(),
                description: Some("Helps with X".into()),
                version: Some("0.1.0".into()),
                author: Some("Tester".into()),
                tags: vec!["util".into()],
                requires: vec![],
                allow_tools: vec!["read_file".into()],
                deny_tools: vec![],
                instructions: "# My Helper\nDo the thing carefully.".into(),
                scope: SkillWriteScope::User,
            },
            &project_dir,
            &data_dir,
        )
        .expect("write");

        assert!(result.created);
        assert_eq!(result.skill.id, "my-helper");
        assert_eq!(result.skill.source, SkillSource::Store);
        assert_eq!(result.skill.allow_tools, vec!["read_file"]);

        let config = SkillsConfig {
            enabled: true,
            dirs: Vec::new(),
            active: vec![],
        };
        let skills =
            discover_configured_skills(&config, &project_dir, &data_dir).expect("discover");
        assert!(skills.iter().any(|s| s.id == "my-helper"));
        assert!(skills.iter().any(|s| s.id == CREATE_SKILL_ID));
        assert!(skills
            .iter()
            .any(|s| s.instructions.contains("Do the thing carefully")));
    }

    #[test]
    fn write_roundtrip_preserves_tool_policy() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let req = SkillWriteRequest {
            id: "reviewer".into(),
            name: "Code Reviewer".into(),
            description: Some("Reviews PRs".into()),
            version: Some("1.0.0".into()),
            author: Some("NAVI".into()),
            tags: vec!["code".into(), "review".into()],
            requires: vec!["socratic".into()],
            allow_tools: vec!["read_file".into(), "bash".into()],
            deny_tools: vec![],
            instructions: "Review thoroughly.".into(),
            scope: SkillWriteScope::User,
        };
        write_skill(&req, tempdir.path(), tempdir.path()).expect("write");
        let loaded = load_skill_by_id(
            &SkillsConfig {
                enabled: true,
                dirs: Vec::new(),
                active: vec![],
            },
            tempdir.path(),
            tempdir.path(),
            "reviewer",
        )
        .expect("load");
        assert_eq!(loaded.name, "Code Reviewer");
        assert_eq!(loaded.tags, vec!["code", "review"]);
        assert_eq!(loaded.requires, vec!["socratic"]);
        assert_eq!(loaded.allow_tools, vec!["read_file", "bash"]);
        assert!(loaded.instructions.contains("Review thoroughly."));
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
            path: PathBuf::from("a"),
            instructions: "a".into(),
            source: SkillSource::Store,
            scope: SkillWriteScope::User,
        };
        let b = SkillManifest {
            allow_tools: vec!["read_file".into(), "skill_save".into()],
            id: "b".into(),
            name: "B".into(),
            description: None,
            version: None,
            author: None,
            tags: vec![],
            requires: vec![],
            deny_tools: vec![],
            path: PathBuf::from("b"),
            instructions: "b".into(),
            source: SkillSource::Store,
            scope: SkillWriteScope::User,
        };
        let list = skill_tool_allowlist(&[a, b]).expect("list");
        assert_eq!(list, vec!["read_file"]);
    }

    #[test]
    fn rejects_skill_with_empty_instructions() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let skill_dir = tempdir.path().join(".navi").join("skills").join("empty");
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(skill_dir.join("SKILL.md"), "---\nname: Empty Skill\n---\n")
            .expect("write skill");

        // Empty skills are skipped during discovery (not fatal).
        let config = SkillsConfig {
            enabled: true,
            dirs: Vec::new(),
            active: Vec::new(),
        };
        let skills =
            discover_configured_skills(&config, tempdir.path(), tempdir.path()).expect("skills");
        assert!(!skills.iter().any(|s| s.id == "empty"));
        assert!(load_skill_dir(&skill_dir).is_err());
    }

    #[test]
    fn discovers_multiple_skills() {
        let tempdir = tempfile::tempdir().expect("tempdir");

        for name in &["alpha", "beta", "gamma"] {
            let skill_dir = tempdir.path().join(".navi").join("skills").join(name);
            fs::create_dir_all(&skill_dir).expect("create skill dir");
            fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: {name}\n---\n# {name}\nInstructions for {name}."),
            )
            .expect("write skill");
        }

        let config = SkillsConfig {
            enabled: true,
            dirs: Vec::new(),
            active: vec!["alpha".to_string(), "gamma".to_string()],
        };
        let skills =
            discover_configured_skills(&config, tempdir.path(), tempdir.path()).expect("skills");
        let file_count = skills.iter().filter(|s| s.source == SkillSource::File).count();
        let active = active_skills(&skills, &config.active, &[]);

        assert_eq!(file_count, 3);
        assert_eq!(active.len(), 2);
        assert!(active.iter().any(|s| s.id == "alpha"));
        assert!(active.iter().any(|s| s.id == "gamma"));
    }

    #[test]
    fn deduplicates_skills_by_id() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let project_skill = tempdir.path().join(".navi").join("skills").join("myskill");
        fs::create_dir_all(&project_skill).expect("create skill dir");
        fs::write(
            project_skill.join("SKILL.md"),
            "---\nname: My Skill\n---\n# My Skill\nProject version.",
        )
        .expect("write skill");

        let global_skill = tempdir.path().join("global-skills").join("myskill");
        fs::create_dir_all(&global_skill).expect("create skill dir");
        fs::write(
            global_skill.join("SKILL.md"),
            "---\nname: My Skill Global\n---\n# My Skill\nGlobal version.",
        )
        .expect("write skill");

        let config = SkillsConfig {
            enabled: true,
            dirs: vec![tempdir.path().join("global-skills")],
            active: Vec::new(),
        };
        let skills =
            discover_configured_skills(&config, tempdir.path(), tempdir.path()).expect("skills");

        let myskill_count = skills.iter().filter(|s| s.id == "myskill").count();
        assert_eq!(myskill_count, 1);
        let myskill = skills.iter().find(|s| s.id == "myskill").unwrap();
        assert!(myskill.name == "My Skill" || myskill.name == "My Skill Global");
    }

    #[test]
    fn returns_empty_when_disabled() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let skill_dir = tempdir.path().join(".navi").join("skills").join("test");
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: Test\n---\n# Test\nInstructions.",
        )
        .expect("write skill");

        let config = SkillsConfig {
            enabled: false,
            dirs: Vec::new(),
            active: vec!["test".to_string()],
        };
        let skills =
            discover_configured_skills(&config, tempdir.path(), tempdir.path()).expect("skills");

        assert!(skills.is_empty());
    }

    #[test]
    fn session_active_overrides_configured_active() {
        let tempdir = tempfile::tempdir().expect("tempdir");

        for name in &["skill-a", "skill-b", "skill-c"] {
            let skill_dir = tempdir.path().join(".navi").join("skills").join(name);
            fs::create_dir_all(&skill_dir).expect("create skill dir");
            fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: {name}\n---\n# {name}\nInstructions."),
            )
            .expect("write skill");
        }

        let config = SkillsConfig {
            enabled: true,
            dirs: Vec::new(),
            active: vec!["skill-a".to_string()],
        };
        let skills =
            discover_configured_skills(&config, tempdir.path(), tempdir.path()).expect("skills");

        let active = active_skills(&skills, &config.active, &[]);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "skill-a");

        let session_active = vec!["skill-b".to_string(), "skill-c".to_string()];
        let active = active_skills(&skills, &config.active, &session_active);
        assert_eq!(active.len(), 2);
        assert!(active.iter().any(|s| s.id == "skill-b"));
        assert!(active.iter().any(|s| s.id == "skill-c"));
    }

    #[test]
    fn matches_by_name_or_id() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let skill_dir = tempdir.path().join(".navi").join("skills").join("my-skill");
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: My Custom Skill\n---\n# My Custom Skill\nInstructions.",
        )
        .expect("write skill");

        let config = SkillsConfig {
            enabled: true,
            dirs: Vec::new(),
            active: Vec::new(),
        };
        let skills =
            discover_configured_skills(&config, tempdir.path(), tempdir.path()).expect("skills");

        let by_id = active_skills(&skills, &[], &["my-skill".to_string()]);
        assert_eq!(by_id.len(), 1);

        let by_name = active_skills(&skills, &[], &["My Custom Skill".to_string()]);
        assert_eq!(by_name.len(), 1);
    }

    #[test]
    fn custom_dirs_are_resolved_relative_to_project() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let custom_dir = tempdir.path().join("custom-skills").join("my-skill");
        fs::create_dir_all(&custom_dir).expect("create skill dir");
        fs::write(
            custom_dir.join("SKILL.md"),
            "---\nname: Custom\n---\n# Custom\nCustom skill.",
        )
        .expect("write skill");

        let config = SkillsConfig {
            enabled: true,
            dirs: vec!["custom-skills".into()],
            active: vec!["my-skill".to_string()],
        };
        let skills =
            discover_configured_skills(&config, tempdir.path(), tempdir.path()).expect("skills");

        assert!(skills.iter().any(|s| s.id == "my-skill"));
        assert!(skills.iter().any(|s| s.id == CREATE_SKILL_ID));
    }

    #[test]
    fn render_returns_none_for_empty_skills() {
        assert!(render_active_skills(&[]).is_none());
    }

    #[test]
    fn render_includes_description_and_instructions() {
        let skills = vec![SkillManifest {
            id: "test".to_string(),
            name: "Test Skill".to_string(),
            description: Some("A test skill".to_string()),
            version: None,
            author: None,
            tags: Vec::new(),
            requires: Vec::new(),
            allow_tools: Vec::new(),
            deny_tools: Vec::new(),
            path: PathBuf::from("/test"),
            instructions: "Do something.".to_string(),
            source: SkillSource::File,
            scope: SkillWriteScope::User,
        }];

        let rendered = render_active_skills(&skills).expect("rendered");
        assert!(rendered.contains("Test Skill"));
        assert!(rendered.contains("A test skill"));
        assert!(rendered.contains("Do something."));
    }

    #[test]
    fn render_available_skills_excludes_instructions() {
        let skills = vec![SkillManifest {
            id: "test".to_string(),
            name: "Test Skill".to_string(),
            description: Some("A test skill".to_string()),
            version: None,
            author: None,
            tags: vec!["demo".to_string()],
            requires: Vec::new(),
            allow_tools: Vec::new(),
            deny_tools: Vec::new(),
            path: PathBuf::from("/test"),
            instructions: "Do something secret until loaded.".to_string(),
            source: SkillSource::File,
            scope: SkillWriteScope::User,
        }];

        let rendered = render_available_skills(&skills).expect("rendered");
        assert!(rendered.contains("load_skill"));
        assert!(rendered.contains("Test Skill"));
        assert!(rendered.contains("A test skill"));
        assert!(rendered.contains("demo"));
        assert!(!rendered.contains("Do something secret until loaded."));
    }
}
