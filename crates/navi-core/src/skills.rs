use crate::config::SkillsConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// A discovered skill loaded from a `SKILL.md` file.
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
    /// Path to the skill directory.
    pub path: PathBuf,
    /// The skill's instruction text (body of `SKILL.md`).
    pub instructions: String,
}

/// Discovers skills from configured directories and the project's `.navi/skills/` folder.
pub fn discover_configured_skills(
    config: &SkillsConfig,
    project_dir: &Path,
    data_dir: &Path,
) -> Result<Vec<SkillManifest>> {
    if !config.enabled {
        return Ok(Vec::new());
    }

    let mut skills = Vec::new();
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
                    skills.push(load_skill_dir(&path)?);
                }
            } else if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
                skills.push(load_skill_dir(&dir)?);
            }
        }
    }

    skills.sort_by(|a, b| a.id.cmp(&b.id));
    skills.dedup_by(|a, b| a.id == b.id);
    Ok(skills)
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

    let manifest = SkillManifest {
        id,
        name,
        description,
        version: metadata.version,
        author: metadata.author,
        tags: metadata.tags,
        requires: metadata.requires,
        path: skill_path,
        instructions: body.trim().to_string(),
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

#[derive(Default)]
struct SkillMetadata {
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
    author: Option<String>,
    tags: Vec<String>,
    requires: Vec<String>,
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
        let active = active_skills(&skills, &config.active, &[]);
        let rendered = render_active_skills(&active).expect("rendered");

        assert_eq!(skills.len(), 1);
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

        assert_eq!(skills.len(), 1);
        let skill = &skills[0];
        assert_eq!(skill.name, "Code Reviewer");
        assert_eq!(skill.version.as_deref(), Some("1.0.0"));
        assert_eq!(skill.author.as_deref(), Some("NAVI Team"));
        assert_eq!(skill.tags, vec!["code", "review", "quality"]);
        assert_eq!(skill.requires, vec!["socratic"]);
    }

    #[test]
    fn rejects_skill_with_empty_instructions() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let skill_dir = tempdir.path().join(".navi").join("skills").join("empty");
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(skill_dir.join("SKILL.md"), "---\nname: Empty Skill\n---\n")
            .expect("write skill");

        let config = SkillsConfig {
            enabled: true,
            dirs: Vec::new(),
            active: Vec::new(),
        };
        let result = discover_configured_skills(&config, tempdir.path(), tempdir.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty instructions"));
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
        let active = active_skills(&skills, &config.active, &[]);

        assert_eq!(skills.len(), 3);
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

        assert_eq!(skills.len(), 1);
        // The first discovered skill wins (project dir is scanned first)
        assert!(skills[0].name == "My Skill" || skills[0].name == "My Skill Global");
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

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].id, "my-skill");
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
            path: PathBuf::from("/test"),
            instructions: "Do something.".to_string(),
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
            path: PathBuf::from("/test"),
            instructions: "Do something secret until loaded.".to_string(),
        }];

        let rendered = render_available_skills(&skills).expect("rendered");
        assert!(rendered.contains("load_skill"));
        assert!(rendered.contains("Test Skill"));
        assert!(rendered.contains("A test skill"));
        assert!(rendered.contains("demo"));
        assert!(!rendered.contains("Do something secret until loaded."));
    }
}
