//! Filesystem store for harness packs under `{data_dir}/harnesses/<id>/`.

use super::types::{GraphSpec, HarnessPack, LoopSpec, VerifierSpec};

#[cfg(test)]
use super::types::VerifierKind;
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

/// Returns `{data_dir}/harnesses`.
pub fn harnesses_root(data_dir: &Path) -> PathBuf {
    data_dir.join("harnesses")
}

/// Returns `{data_dir}/harnesses/<id>`.
pub fn harness_pack_dir(data_dir: &Path, id: &str) -> PathBuf {
    harnesses_root(data_dir).join(sanitize_id(id))
}

fn sanitize_id(id: &str) -> String {
    let s = id.trim().to_ascii_lowercase();
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if matches!(ch, '-' | '_' | ' ' | '/') && !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "harness".into()
    } else {
        out
    }
}

/// Lists harness pack ids (directory names under harnesses/).
pub fn list_harness_ids(data_dir: &Path) -> Result<Vec<String>> {
    let root = harnesses_root(data_dir);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    for entry in fs::read_dir(&root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && let Some(name) = entry.file_name().to_str()
        {
            if name.starts_with('.') {
                continue;
            }
            // Prefer packs that have loop.toml
            if entry.path().join("loop.toml").is_file() {
                ids.push(name.to_string());
            }
        }
    }
    ids.sort();
    Ok(ids)
}

/// Loads a harness pack by id. Returns `None` if the directory or loop.toml is missing.
pub fn load_pack(data_dir: &Path, id: &str) -> Result<Option<HarnessPack>> {
    let root = harness_pack_dir(data_dir, id);
    let loop_path = root.join("loop.toml");
    if !loop_path.is_file() {
        return Ok(None);
    }
    let loop_raw =
        fs::read_to_string(&loop_path).with_context(|| format!("read {}", loop_path.display()))?;
    let mut loop_spec: LoopSpec =
        toml::from_str(&loop_raw).with_context(|| format!("parse {}", loop_path.display()))?;
    if loop_spec.id.trim().is_empty() {
        loop_spec.id = sanitize_id(id);
    }

    let graph = {
        let graph_path = root.join("graph.toml");
        if graph_path.is_file() {
            let raw = fs::read_to_string(&graph_path)
                .with_context(|| format!("read {}", graph_path.display()))?;
            Some(
                toml::from_str::<GraphSpec>(&raw)
                    .with_context(|| format!("parse {}", graph_path.display()))?,
            )
        } else {
            None
        }
    };

    let verifiers = load_verifiers_dir(&root.join("verifiers"))?;

    let capability_md = read_optional_utf8(&root.join("CAPABILITY.md"))?;
    let skill_md = read_optional_utf8(&root.join("SKILL.md"))?;

    Ok(Some(HarnessPack {
        id: sanitize_id(id),
        root,
        loop_spec,
        graph,
        verifiers,
        capability_md,
        skill_md,
    }))
}

fn read_optional_utf8(path: &Path) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(
        fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?,
    ))
}

fn load_verifiers_dir(dir: &Path) -> Result<Vec<VerifierSpec>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let spec: VerifierSpec =
            toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
        out.push(spec);
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Writes a full pack to disk (creates directories). Overwrites existing files.
pub fn write_pack(data_dir: &Path, pack: &HarnessPack) -> Result<PathBuf> {
    let id = sanitize_id(&pack.id);
    if id.is_empty() {
        bail!("harness pack id is required");
    }
    let root = harness_pack_dir(data_dir, &id);
    fs::create_dir_all(&root).with_context(|| format!("create {}", root.display()))?;
    fs::create_dir_all(root.join("verifiers"))
        .with_context(|| format!("create {}/verifiers", root.display()))?;
    fs::create_dir_all(root.join("runs"))
        .with_context(|| format!("create {}/runs", root.display()))?;

    let mut loop_spec = pack.loop_spec.clone();
    loop_spec.id = id.clone();
    let loop_toml = toml::to_string_pretty(&loop_spec).context("serialize loop.toml")?;
    fs::write(root.join("loop.toml"), loop_toml)
        .with_context(|| format!("write {}/loop.toml", root.display()))?;

    if let Some(graph) = &pack.graph {
        let graph_toml = toml::to_string_pretty(graph).context("serialize graph.toml")?;
        fs::write(root.join("graph.toml"), graph_toml)
            .with_context(|| format!("write {}/graph.toml", root.display()))?;
    }

    for v in &pack.verifiers {
        let name = if v.id.is_empty() {
            "verifier.toml".into()
        } else {
            format!("{}.toml", sanitize_id(&v.id))
        };
        let body = toml::to_string_pretty(v).context("serialize verifier")?;
        fs::write(root.join("verifiers").join(&name), body)
            .with_context(|| format!("write verifier {name}"))?;
    }

    if let Some(cap) = &pack.capability_md {
        fs::write(root.join("CAPABILITY.md"), cap)
            .with_context(|| "write CAPABILITY.md".to_string())?;
    }
    if let Some(skill) = &pack.skill_md {
        fs::write(root.join("SKILL.md"), skill).with_context(|| "write SKILL.md".to_string())?;
    }

    // Seed CHANGELOG if missing
    let changelog = root.join("CHANGELOG.md");
    if !changelog.is_file() {
        fs::write(
            &changelog,
            format!("# Harness `{id}`\n\n- Initial materialize\n"),
        )?;
    }

    Ok(root)
}

/// Thin facade used by CLI/SDK.
pub struct HarnessPackStore {
    data_dir: PathBuf,
}

impl HarnessPackStore {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn list(&self) -> Result<Vec<String>> {
        list_harness_ids(&self.data_dir)
    }

    pub fn load(&self, id: &str) -> Result<Option<HarnessPack>> {
        load_pack(&self.data_dir, id)
    }

    pub fn write(&self, pack: &HarnessPack) -> Result<PathBuf> {
        write_pack(&self.data_dir, pack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness_pack::types::{GraphEdge, GraphNode};

    #[test]
    fn write_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut pack = HarnessPack {
            id: "Design Loop".into(),
            root: PathBuf::new(),
            loop_spec: LoopSpec {
                id: "design-loop".into(),
                max_turns: 12,
                token_budget: Some(50_000),
                stop: vec!["goal.complete".into(), "budget".into()],
            },
            graph: Some(GraphSpec {
                entry: "explore".into(),
                nodes: vec![
                    GraphNode {
                        id: "explore".into(),
                        role: "read_only".into(),
                        allow_tools: vec!["search".into(), "read_file".into()],
                        verifiers: vec![],
                    },
                    GraphNode {
                        id: "implement".into(),
                        role: "write".into(),
                        allow_tools: vec!["edit".into(), "write_file".into()],
                        verifiers: vec![],
                    },
                ],
                edges: vec![GraphEdge {
                    from: "explore".into(),
                    to: "implement".into(),
                    when: None,
                }],
            }),
            verifiers: vec![VerifierSpec {
                id: "smoke".into(),
                kind: VerifierKind::Bash,
                recipe: "cargo test -q".into(),
            }],
            capability_md: Some("browser: yes\n".into()),
            skill_md: Some("# Design Loop\n".into()),
        };
        let root = write_pack(dir.path(), &pack).unwrap();
        assert!(root.join("loop.toml").is_file());
        assert!(root.join("graph.toml").is_file());
        assert!(root.join("verifiers/smoke.toml").is_file());

        let loaded = load_pack(dir.path(), "design-loop").unwrap().unwrap();
        assert_eq!(loaded.id, "design-loop");
        assert_eq!(loaded.loop_spec.max_turns, 12);
        assert_eq!(loaded.loop_spec.token_budget, Some(50_000));
        let graph = loaded.graph.as_ref().unwrap();
        assert_eq!(graph.entry, "explore");
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(loaded.verifiers.len(), 1);
        assert_eq!(loaded.verifiers[0].recipe, "cargo test -q");
        assert!(loaded.capability_md.unwrap().contains("browser"));
        pack.root = root;
    }

    #[test]
    fn list_ids_sorted() {
        let dir = tempfile::tempdir().unwrap();
        for id in ["z-pack", "a-pack"] {
            let pack = HarnessPack {
                id: id.into(),
                root: PathBuf::new(),
                loop_spec: LoopSpec {
                    id: id.into(),
                    ..LoopSpec::default()
                },
                graph: None,
                verifiers: vec![],
                capability_md: None,
                skill_md: None,
            };
            write_pack(dir.path(), &pack).unwrap();
        }
        let ids = list_harness_ids(dir.path()).unwrap();
        assert_eq!(ids, vec!["a-pack".to_string(), "z-pack".to_string()]);
    }
}
