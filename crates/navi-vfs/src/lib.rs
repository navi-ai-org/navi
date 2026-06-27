pub mod code;
pub(crate) mod dynamic;
pub mod format;
pub mod lang;
pub mod minify;
pub mod validate;

use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

use crate::lang::LangId;

/// Configuration for the Virtual File System engine.
#[derive(Debug, Clone)]
pub struct VfsConfig {
    /// Enable VFS minification on read and formatting on write.
    pub enabled: bool,
    /// Keep comments in minified output.
    pub keep_comments: bool,
    /// Languages to enable VFS for (empty = all Tier 1).
    pub languages: Vec<LangId>,
}

impl Default for VfsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            keep_comments: false,
            languages: Vec::new(),
        }
    }
}

impl VfsConfig {
    /// Whether VFS is active for the given language.
    pub fn is_active_for(&self, lang: LangId) -> bool {
        if !self.enabled {
            return false;
        }
        if self.languages.is_empty() {
            return true;
        }
        self.languages.contains(&lang)
            || (lang == LangId::Tsx && self.languages.contains(&LangId::TypeScript))
    }
}

/// The VFS engine — handles minification on read and formatting on write.
///
/// Thread-safe; can be shared across tool invocations via `Arc<VfsEngine>`.
pub struct VfsEngine {
    config: VfsConfig,
}

impl VfsEngine {
    /// Create a new VFS engine with the given configuration.
    pub fn new(config: VfsConfig) -> Self {
        Self { config }
    }

    /// Detect the language of a file from its path extension.
    pub fn detect_language(&self, path: &Path) -> Option<LangId> {
        lang::detect_language(path)
    }

    /// Minify source code for sending to the LLM.
    ///
    /// Returns `Ok(None)` if VFS is not active for this language or if
    /// validation fails (falls back to original).
    pub fn minify(&self, path: &Path, source: &str) -> Option<String> {
        let lang = self.detect_language(path)?;
        if !self.config.is_active_for(lang) {
            return None;
        }
        if source.is_empty() {
            return None;
        }

        let minified = match minify::minify(source, lang, self.config.keep_comments) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(lang = %lang.name(), error = %e, "VFS minify failed");
                return None;
            }
        };

        // If minification didn't reduce size, skip it.
        if minified.len() >= source.len() {
            return None;
        }

        // Validate the minified output.
        if let Err(e) = validate::validate_minified(&minified, source, lang) {
            tracing::warn!(lang = %lang.name(), error = %e, "VFS validation failed; keeping original");
            return None;
        }

        Some(minified)
    }

    /// Format a file on disk after a VFS write (restore readable formatting).
    ///
    /// Returns `Ok(())` on success, or an error if the formatter fails.
    /// If VFS is not active for this language, this is a no-op.
    pub fn format_after_write(&self, path: &Path) -> Result<()> {
        let Some(lang) = self.detect_language(path) else {
            return Ok(());
        };
        if !self.config.is_active_for(lang) {
            return Ok(());
        }
        format::format_file(path, lang)
    }

    /// Format multiple files after a patch operation.
    pub fn format_after_patch(&self, paths: &[&Path]) {
        for path in paths {
            if let Err(e) = self.format_after_write(path) {
                tracing::warn!(path = %path.display(), error = %e, "VFS post-patch format failed");
            }
        }
    }

    /// Check if the formatter for a given path's language is available.
    pub fn formatter_available(&self, path: &Path) -> bool {
        let Some(lang) = self.detect_language(path) else {
            return false;
        };
        format::formatter_available(lang)
    }

    /// Returns the current VFS configuration.
    pub fn config(&self) -> &VfsConfig {
        &self.config
    }
}

/// Build an `Arc<VfsEngine>` from a config, for sharing across tool invocations.
pub fn build_vfs_engine(config: VfsConfig) -> Arc<VfsEngine> {
    Arc::new(VfsEngine::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minify_and_format_roundtrip() {
        let engine = VfsEngine::new(VfsConfig::default());

        let src = "fn main() {\n    let x = 1;\n    println!(\"{}\", x);\n}\n";
        let minified = engine.minify(Path::new("main.rs"), src);
        assert!(minified.is_some());
        let minified = minified.unwrap();
        assert!(minified.len() < src.len());
        assert!(minified.contains("fn main(){"));
    }

    #[test]
    fn unsupported_language_returns_none() {
        let engine = VfsEngine::new(VfsConfig::default());
        assert!(engine.minify(Path::new("file.xyz"), "content").is_none());
    }

    #[test]
    fn disabled_vfs_returns_none() {
        let engine = VfsEngine::new(VfsConfig {
            enabled: false,
            ..Default::default()
        });
        assert!(
            engine
                .minify(Path::new("main.rs"), "fn main() {}")
                .is_none()
        );
    }

    #[test]
    fn language_filter_respected() {
        let engine = VfsEngine::new(VfsConfig {
            enabled: true,
            languages: vec![LangId::Go],
            ..Default::default()
        });
        // Go should work.
        assert!(
            engine
                .minify(Path::new("main.go"), "package main\n")
                .is_some()
        );
        // Rust should be filtered out.
        assert!(
            engine
                .minify(Path::new("main.rs"), "fn main() {}")
                .is_none()
        );
    }

    #[test]
    fn typescript_filter_includes_tsx() {
        let engine = VfsEngine::new(VfsConfig {
            enabled: true,
            languages: vec![LangId::TypeScript],
            ..Default::default()
        });

        assert!(
            engine
                .minify(
                    Path::new("View.tsx"),
                    "export const View = () => <div>{'hello'}</div>;\n",
                )
                .is_some()
        );
    }

    #[test]
    fn empty_source_returns_none() {
        let engine = VfsEngine::new(VfsConfig::default());
        assert!(engine.minify(Path::new("main.rs"), "").is_none());
    }

    #[test]
    fn detect_language_works() {
        let engine = VfsEngine::new(VfsConfig::default());
        assert_eq!(
            engine.detect_language(Path::new("main.rs")),
            Some(LangId::Rust)
        );
        assert_eq!(engine.detect_language(Path::new("file.xyz")), None);
    }

    #[test]
    fn format_after_write_noop_for_unknown() {
        let engine = VfsEngine::new(VfsConfig::default());
        // Should not error for unknown language.
        assert!(
            engine
                .format_after_write(Path::new("/tmp/file.xyz"))
                .is_ok()
        );
    }
}
