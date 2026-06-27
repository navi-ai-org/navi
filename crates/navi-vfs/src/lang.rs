use std::path::Path;

/// Supported languages for VFS operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LangId {
    // Tier 1
    Rust,
    Go,
    C,
    Cpp,
    JavaScript,
    TypeScript,
    Tsx,
    Python,
    Java,
    // Tier 2
    Ruby,
    Php,
    Bash,
    Html,
    Css,
    Json,
    CSharp,
}

impl LangId {
    /// Returns the canonical name used in config and logging.
    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Go => "go",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::Python => "python",
            Self::Java => "java",
            Self::Ruby => "ruby",
            Self::Php => "php",
            Self::Bash => "bash",
            Self::Html => "html",
            Self::Css => "css",
            Self::Json => "json",
            Self::CSharp => "csharp",
        }
    }

    /// Name of the compiled `.so` library (without `lib` prefix / extension).
    pub(crate) fn so_name(self) -> &'static str {
        match self {
            Self::Rust => "tree_sitter_rust",
            Self::Go => "tree_sitter_go",
            Self::C => "tree_sitter_c",
            Self::Cpp => "tree_sitter_cpp",
            Self::JavaScript => "tree_sitter_javascript",
            Self::TypeScript => "tree_sitter_typescript",
            Self::Tsx => "tree_sitter_tsx",
            Self::Python => "tree_sitter_python",
            Self::Java => "tree_sitter_java",
            Self::Ruby => "tree_sitter_ruby",
            Self::Php => "tree_sitter_php",
            Self::Bash => "tree_sitter_bash",
            Self::Html => "tree_sitter_html",
            Self::Css => "tree_sitter_css",
            Self::Json => "tree_sitter_json",
            Self::CSharp => "tree_sitter_c_sharp",
        }
    }

    /// Exported C symbol name inside the `.so`.
    pub(crate) fn symbol_name(self) -> &'static str {
        // For regular grammars the symbol matches the so_name.
        self.so_name()
    }

    /// Load and return the tree-sitter `Language` for this language.
    ///
    /// The grammar shared library is loaded on first access and cached.
    pub fn tree_sitter_language(self) -> anyhow::Result<tree_sitter::Language> {
        crate::dynamic::load_language(self)
    }

    /// All supported languages.
    pub fn all() -> &'static [LangId] {
        &[
            // Tier 1
            Self::Rust,
            Self::Go,
            Self::C,
            Self::Cpp,
            Self::JavaScript,
            Self::TypeScript,
            Self::Tsx,
            Self::Python,
            Self::Java,
            // Tier 2
            Self::Ruby,
            Self::Php,
            Self::Bash,
            Self::Html,
            Self::Css,
            Self::Json,
            Self::CSharp,
        ]
    }

    /// Parse from config string name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "rust" | "rs" => Some(Self::Rust),
            "go" | "golang" => Some(Self::Go),
            "c" => Some(Self::C),
            "cpp" | "c++" | "cxx" | "cc" => Some(Self::Cpp),
            "javascript" | "js" => Some(Self::JavaScript),
            "typescript" | "ts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "python" | "py" => Some(Self::Python),
            "java" => Some(Self::Java),
            "ruby" | "rb" => Some(Self::Ruby),
            "php" => Some(Self::Php),
            "bash" | "sh" | "shell" | "zsh" => Some(Self::Bash),
            "html" | "htm" => Some(Self::Html),
            "css" | "scss" => Some(Self::Css),
            "json" | "jsonc" => Some(Self::Json),
            "csharp" | "c#" | "cs" => Some(Self::CSharp),
            _ => None,
        }
    }

    /// Whether this language needs a trailing newline after line comments.
    pub fn needs_line_comment_newline(self) -> bool {
        matches!(
            self,
            Self::Rust
                | Self::Go
                | Self::C
                | Self::Cpp
                | Self::JavaScript
                | Self::TypeScript
                | Self::Tsx
                | Self::Python
                | Self::Java
                | Self::Ruby
                | Self::Php
                | Self::Bash
                | Self::CSharp
        )
    }

    /// Whether this language uses newlines as statement separators
    /// (like Go's auto-semicolon, Swift, Kotlin).
    pub fn newline_is_separator(self) -> bool {
        matches!(self, Self::Go | Self::Ruby | Self::Bash)
    }
}

/// Detect language from a file path's extension.
pub fn detect_language(path: &Path) -> Option<LangId> {
    let ext = path.extension()?.to_str()?;
    match ext {
        // Tier 1
        "rs" => Some(LangId::Rust),
        "go" => Some(LangId::Go),
        "c" | "h" => Some(LangId::C),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" | "c++" => Some(LangId::Cpp),
        "js" | "mjs" | "cjs" | "jsx" => Some(LangId::JavaScript),
        "ts" | "mts" | "cts" => Some(LangId::TypeScript),
        "tsx" => Some(LangId::Tsx),
        "py" | "pyi" => Some(LangId::Python),
        "java" => Some(LangId::Java),
        // Tier 2
        "rb" | "rake" | "gemspec" => Some(LangId::Ruby),
        "php" | "phtml" | "php3" | "php4" | "php5" | "phps" => Some(LangId::Php),
        "sh" | "bash" | "zsh" | "ksh" | "bashrc" => Some(LangId::Bash),
        "html" | "htm" => Some(LangId::Html),
        "css" => Some(LangId::Css),
        "json" | "jsonc" => Some(LangId::Json),
        "cs" => Some(LangId::CSharp),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tier 1
    #[test]
    fn detect_rust() {
        assert_eq!(detect_language(Path::new("main.rs")), Some(LangId::Rust));
    }

    #[test]
    fn detect_go() {
        assert_eq!(detect_language(Path::new("main.go")), Some(LangId::Go));
    }

    #[test]
    fn detect_c_cpp_variants() {
        assert_eq!(detect_language(Path::new("foo.c")), Some(LangId::C));
        assert_eq!(detect_language(Path::new("foo.h")), Some(LangId::C));
        assert_eq!(detect_language(Path::new("foo.cpp")), Some(LangId::Cpp));
        assert_eq!(detect_language(Path::new("foo.cc")), Some(LangId::Cpp));
        assert_eq!(detect_language(Path::new("foo.hpp")), Some(LangId::Cpp));
        assert_eq!(detect_language(Path::new("foo.cxx")), Some(LangId::Cpp));
    }

    #[test]
    fn detect_js_ts() {
        assert_eq!(
            detect_language(Path::new("app.js")),
            Some(LangId::JavaScript)
        );
        assert_eq!(
            detect_language(Path::new("app.mjs")),
            Some(LangId::JavaScript)
        );
        assert_eq!(
            detect_language(Path::new("app.ts")),
            Some(LangId::TypeScript)
        );
        assert_eq!(detect_language(Path::new("app.tsx")), Some(LangId::Tsx));
    }

    #[test]
    fn detect_python() {
        assert_eq!(detect_language(Path::new("main.py")), Some(LangId::Python));
        assert_eq!(detect_language(Path::new("main.pyi")), Some(LangId::Python));
    }

    #[test]
    fn detect_java() {
        assert_eq!(detect_language(Path::new("Main.java")), Some(LangId::Java));
    }

    // Tier 2
    #[test]
    fn detect_ruby() {
        assert_eq!(detect_language(Path::new("app.rb")), Some(LangId::Ruby));
        assert_eq!(
            detect_language(Path::new("Rakefile.rb")),
            Some(LangId::Ruby)
        );
        assert_eq!(
            detect_language(Path::new("foo.gemspec")),
            Some(LangId::Ruby)
        );
    }

    #[test]
    fn detect_php() {
        assert_eq!(detect_language(Path::new("index.php")), Some(LangId::Php));
        assert_eq!(detect_language(Path::new("view.phtml")), Some(LangId::Php));
    }

    #[test]
    fn detect_bash() {
        assert_eq!(detect_language(Path::new("run.sh")), Some(LangId::Bash));
        assert_eq!(detect_language(Path::new("setup.bash")), Some(LangId::Bash));
        assert_eq!(detect_language(Path::new("env.zsh")), Some(LangId::Bash));
    }

    #[test]
    fn detect_html() {
        assert_eq!(detect_language(Path::new("index.html")), Some(LangId::Html));
        assert_eq!(detect_language(Path::new("page.htm")), Some(LangId::Html));
    }

    #[test]
    fn detect_css() {
        assert_eq!(detect_language(Path::new("style.css")), Some(LangId::Css));
    }

    #[test]
    fn detect_json() {
        assert_eq!(
            detect_language(Path::new("config.json")),
            Some(LangId::Json)
        );
        assert_eq!(
            detect_language(Path::new("tsconfig.jsonc")),
            Some(LangId::Json)
        );
    }

    #[test]
    fn detect_csharp() {
        assert_eq!(
            detect_language(Path::new("Program.cs")),
            Some(LangId::CSharp)
        );
    }

    #[test]
    fn detect_unknown() {
        assert_eq!(detect_language(Path::new("file.xyz")), None);
        assert_eq!(detect_language(Path::new("Makefile")), None);
    }

    #[test]
    fn from_name_variants() {
        // Tier 1
        assert_eq!(LangId::from_name("rust"), Some(LangId::Rust));
        assert_eq!(LangId::from_name("Rust"), Some(LangId::Rust));
        assert_eq!(LangId::from_name("rs"), Some(LangId::Rust));
        assert_eq!(LangId::from_name("golang"), Some(LangId::Go));
        assert_eq!(LangId::from_name("cpp"), Some(LangId::Cpp));
        assert_eq!(LangId::from_name("c++"), Some(LangId::Cpp));
        assert_eq!(LangId::from_name("python"), Some(LangId::Python));
        assert_eq!(LangId::from_name("py"), Some(LangId::Python));
        // Tier 2
        assert_eq!(LangId::from_name("ruby"), Some(LangId::Ruby));
        assert_eq!(LangId::from_name("rb"), Some(LangId::Ruby));
        assert_eq!(LangId::from_name("php"), Some(LangId::Php));
        assert_eq!(LangId::from_name("bash"), Some(LangId::Bash));
        assert_eq!(LangId::from_name("sh"), Some(LangId::Bash));
        assert_eq!(LangId::from_name("shell"), Some(LangId::Bash));
        assert_eq!(LangId::from_name("html"), Some(LangId::Html));
        assert_eq!(LangId::from_name("css"), Some(LangId::Css));
        assert_eq!(LangId::from_name("json"), Some(LangId::Json));
        assert_eq!(LangId::from_name("csharp"), Some(LangId::CSharp));
        assert_eq!(LangId::from_name("c#"), Some(LangId::CSharp));
        assert_eq!(LangId::from_name("cs"), Some(LangId::CSharp));
        // Unknown
        assert_eq!(LangId::from_name("unknown"), None);
    }
}
