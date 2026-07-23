use crate::NaviConfig;
use crate::context::{ContextPacket, render_context_packets};
use crate::harness::{build_system_prompt_with_manifest_text, tool_prompt_manifest};
use crate::model::ModelMessage;
use crate::skills::{CatalogEntries, SkillManifest, SkillPool, render_catalog_entries};
use crate::tool::ToolDefinition;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

#[derive(Debug, Default)]
pub struct PromptCache {
    files: Mutex<HashMap<PathBuf, CachedFile>>,
    rendered: Mutex<HashMap<RenderedPromptKey, String>>,
    disk_reads: AtomicUsize,
}

#[derive(Debug, Clone)]
struct CachedFile {
    content: String,
    modified: Option<SystemTime>,
    len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum RenderedPromptKey {
    ToolManifest(u64),
}

impl PromptCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn read_file(&self, path: &Path) -> Result<String> {
        let metadata =
            fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
        let modified = metadata.modified().ok();
        let len = metadata.len();
        let canonical = normalize_cache_path(path);

        if let Some(cached) = self
            .files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&canonical)
            && cached.modified == modified
            && cached.len == len
        {
            return Ok(cached.content.clone());
        }

        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        self.disk_reads.fetch_add(1, Ordering::Relaxed);
        self.files.lock().unwrap_or_else(|e| e.into_inner()).insert(
            canonical,
            CachedFile {
                content: content.clone(),
                modified,
                len,
            },
        );
        Ok(content)
    }

    pub fn render_tool_manifest(&self, tools: &[ToolDefinition]) -> String {
        let key = RenderedPromptKey::ToolManifest(tool_definitions_hash(tools));
        if let Some(cached) = self
            .rendered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return cached.clone();
        }
        let rendered = tool_prompt_manifest(tools);
        self.rendered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, rendered.clone());
        rendered
    }

    pub fn disk_read_count(&self) -> usize {
        self.disk_reads.load(Ordering::Relaxed)
    }
}

/// The result of rendering a system prompt: a stable base `instructions`
/// string (sent in the provider's `instructions` field or as the first
/// system message) and a list of dynamic `developer_messages` (injected
/// as separate messages so that changes to context blocks don't
/// invalidate the provider's prompt cache for the base prefix).
#[derive(Debug, Clone, Default)]
pub struct RenderedPrompt {
    /// Stable base instructions for the `instructions` field of the
    /// provider request. Kept identical across turns when config, cwd,
    /// and tool set are unchanged.
    pub instructions: String,
    /// Dynamic context blocks injected as separate developer-role
    /// messages after the base instructions. Each block can change
    /// independently without invalidating the cache for `instructions`.
    pub developer_messages: Vec<ModelMessage>,
}

#[derive(Clone)]
pub struct SystemPromptRenderer {
    cache: std::sync::Arc<PromptCache>,
}

impl SystemPromptRenderer {
    pub fn new(cache: std::sync::Arc<PromptCache>) -> Self {
        Self { cache }
    }

    pub fn render(&self, input: SystemPromptInput) -> RenderedPrompt {
        let manifest = if input.include_tool_prompt_manifest && !input.tools.is_empty() {
            Some(self.cache.render_tool_manifest(&input.tools))
        } else {
            None
        };

        // Stable base: identity, workflow, tool rules, code tools, sprint
        // contract, auto-memory instructions, and tool manifest. Does NOT
        // include AGENTS.md, context packets, skills, or memory injection.
        let instructions = build_system_prompt_with_manifest_text(
            &input.config,
            &input.project_dir,
            None,
            manifest.as_deref(),
        );

        let mut developer_messages = Vec::new();

        // Global user instructions (~/.config/navi/AGENTS.md).
        if let Ok(dirs) = crate::config::persistence::navi_dirs() {
            let global_agents_path = dirs.config_dir().join("AGENTS.md");
            if let Ok(global_agents) = self.cache.read_file(&global_agents_path)
                && !global_agents.trim().is_empty()
            {
                developer_messages.push(ModelMessage::developer(format!(
                    "=== Global User Instructions (AGENTS.md) ===\n{global_agents}"
                )));
            }
        }

        // Project-level AGENTS.md (omit entirely when absent — no placeholder noise).
        if let Ok(project_agents) = self.cache.read_file(&input.project_dir.join("AGENTS.md"))
            && !project_agents.trim().is_empty()
        {
            developer_messages.push(ModelMessage::developer(format!(
                "=== AGENTS.md / Project Instructions ===\n{project_agents}"
            )));
        }

        // Context packets (external context from clients).
        if let Some(context) = render_context_packets(&input.context_packets) {
            developer_messages.push(ModelMessage::developer(context));
        }

        // Catalog of root skills + pools (metadata only; open pool / load_skill for more).
        let catalog = CatalogEntries {
            root_skills: input.available_skills.clone(),
            pools: input.skill_pools.clone(),
        };
        if let Some(skills) = render_catalog_entries(&catalog) {
            developer_messages.push(ModelMessage::developer(skills));
        }

        // Active harness pack cards (loop/graph soft policy).
        if let Some(card) = &input.harness_card
            && !card.trim().is_empty()
        {
            developer_messages.push(ModelMessage::developer(card.clone()));
        }

        // Memory injection (auto-memory index + session memory).
        if let Some(memory) = &input.memory_injection
            && !memory.trim().is_empty()
        {
            developer_messages.push(ModelMessage::developer(memory.clone()));
        }

        RenderedPrompt {
            instructions,
            developer_messages,
        }
    }
}

pub struct SystemPromptInput {
    pub config: NaviConfig,
    pub project_dir: PathBuf,
    pub memory_injection: Option<String>,
    pub tools: Vec<ToolDefinition>,
    pub include_tool_prompt_manifest: bool,
    pub context_packets: Vec<ContextPacket>,
    pub available_skills: Vec<SkillManifest>,
    pub active_skills: Vec<SkillManifest>,
    /// Skill pools (folders) shown next to root skills in the catalog.
    pub skill_pools: Vec<SkillPool>,
    /// Optional harness pack developer card (loop/graph soft policy).
    pub harness_card: Option<String>,
}

fn normalize_cache_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn tool_definitions_hash(tools: &[ToolDefinition]) -> u64 {
    let mut tools = tools.to_vec();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let serialized = serde_json::to_string(&tools).unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    serialized.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HarnessProfile, ToolKind};
    use serde_json::json;

    #[test]
    fn prompt_cache_reuses_unchanged_file() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("AGENTS.md");
        std::fs::write(&path, "instructions").expect("write");
        let cache = PromptCache::new();

        assert_eq!(cache.read_file(&path).unwrap(), "instructions");
        assert_eq!(cache.read_file(&path).unwrap(), "instructions");
        assert_eq!(cache.disk_read_count(), 1);
    }

    #[test]
    fn prompt_cache_invalidates_when_file_changes() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("AGENTS.md");
        std::fs::write(&path, "one").expect("write one");
        let cache = PromptCache::new();

        assert_eq!(cache.read_file(&path).unwrap(), "one");
        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&path, "two-two").expect("write two");
        assert_eq!(cache.read_file(&path).unwrap(), "two-two");
        assert_eq!(cache.disk_read_count(), 2);
    }

    #[test]
    fn system_prompt_renderer_uses_cached_agents_md() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        std::fs::write(tempdir.path().join("AGENTS.md"), "project rules").expect("write");
        let cache = std::sync::Arc::new(PromptCache::new());
        let renderer = SystemPromptRenderer::new(cache.clone());
        let input = || SystemPromptInput {
            config: crate::NaviConfig {
                harness: crate::config::HarnessConfig {
                    profile: HarnessProfile::Small,
                    ..Default::default()
                },
                ..Default::default()
            },
            project_dir: tempdir.path().to_path_buf(),
            memory_injection: None,
            tools: vec![ToolDefinition {
                name: "read_file".to_string(),
                description: "read".to_string(),
                kind: ToolKind::Read,
                input_schema: json!({"type":"object"}),
                ..Default::default()
            }],
            include_tool_prompt_manifest: true,
            context_packets: Vec::new(),
            available_skills: Vec::new(),
            active_skills: Vec::new(),
            skill_pools: Vec::new(),
            harness_card: None,
        };

        let first = renderer.render(input());
        let second = renderer.render(input());
        assert!(
            first
                .developer_messages
                .iter()
                .any(|m| m.content.contains("project rules"))
        );
        assert_eq!(first.instructions, second.instructions);
        assert_eq!(
            first.developer_messages.len(),
            second.developer_messages.len()
        );
        for (a, b) in first
            .developer_messages
            .iter()
            .zip(second.developer_messages.iter())
        {
            assert_eq!(a.content, b.content);
        }
        assert_eq!(cache.disk_read_count(), 1);
    }
}
