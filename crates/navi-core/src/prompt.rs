use crate::NaviConfig;
use crate::context::{ContextPacket, render_context_packets};
use crate::harness::{build_system_prompt_with_manifest_text, tool_prompt_manifest};
use crate::skills::{SkillManifest, render_active_skills};
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

const DEFAULT_AGENTS_INSTRUCTIONS: &str = "Default NAVI base instructions";

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

#[derive(Clone)]
pub struct SystemPromptRenderer {
    cache: std::sync::Arc<PromptCache>,
}

impl SystemPromptRenderer {
    pub fn new(cache: std::sync::Arc<PromptCache>) -> Self {
        Self { cache }
    }

    pub fn render(&self, input: SystemPromptInput) -> String {
        let agents = self
            .cache
            .read_file(&input.project_dir.join("AGENTS.md"))
            .unwrap_or_else(|_| DEFAULT_AGENTS_INSTRUCTIONS.to_string());
        let manifest = if input.include_tool_prompt_manifest && !input.tools.is_empty() {
            Some(self.cache.render_tool_manifest(&input.tools))
        } else {
            None
        };
        let mut system_content = format!(
            "{}\n\n=== AGENTS.md / Project Instructions ===\n{}",
            build_system_prompt_with_manifest_text(
                &input.config,
                &input.project_dir,
                input.memory_injection.as_deref(),
                manifest.as_deref(),
            ),
            agents
        );
        if let Some(context) = render_context_packets(&input.context_packets) {
            system_content.push_str("\n\n");
            system_content.push_str(&context);
        }
        if let Some(skills) = render_active_skills(&input.active_skills) {
            system_content.push_str("\n\n");
            system_content.push_str(&skills);
        }
        system_content
    }
}

pub struct SystemPromptInput {
    pub config: NaviConfig,
    pub project_dir: PathBuf,
    pub memory_injection: Option<String>,
    pub tools: Vec<ToolDefinition>,
    pub include_tool_prompt_manifest: bool,
    pub context_packets: Vec<ContextPacket>,
    pub active_skills: Vec<SkillManifest>,
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
            active_skills: Vec::new(),
        };

        let first = renderer.render(input());
        let second = renderer.render(input());
        assert!(first.contains("project rules"));
        assert_eq!(first, second);
        assert_eq!(cache.disk_read_count(), 1);
    }
}
