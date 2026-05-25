use crate::config::SkillsConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillManifest {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub path: PathBuf,
    pub instructions: String,
}

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
        output.push_str(skill.instructions.trim());
        output.push_str("\n\n");
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

    Ok(SkillManifest {
        id,
        name,
        description,
        path: skill_path,
        instructions: body.trim().to_string(),
    })
}

#[derive(Default)]
struct SkillMetadata {
    name: Option<String>,
    description: Option<String>,
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
                _ => {}
            }
        }
    }
    (metadata, body)
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
}
