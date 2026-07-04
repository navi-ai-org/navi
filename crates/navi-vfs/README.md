# navi-vfs

[![Crates.io](https://img.shields.io/crates/v/navi-vfs)](https://crates.io/crates/navi-vfs)
[![License](https://img.shields.io/crates/l/navi-vfs)](../LICENSE)

Virtual File System engine for [NAVI](https://github.com/navi-ai-org/navi) — tree-sitter-powered code minification and formatting.

`navi-vfs` reduces token usage by minifying source files on read (stripping comments, whitespace, and non-essential syntax) while preserving semantic structure, and formatting them back on write.

## What's inside

| Module | Purpose |
|--------|---------|
| `code` | Core code analysis — symbol extraction, structure detection |
| `lang` | Language identification and tree-sitter grammar loading |
| `minify` | Source minification — comment stripping, whitespace collapse |
| `format` | Source formatting — reconstruct readable code from minified form |
| `validate` | Semantic validation — ensure minified output preserves meaning |
| `dynamic` | Dynamic grammar loading for languages |

## Supported languages

Tree-sitter grammars are bundled at build time. Tier 1 languages include:

Rust, TypeScript, JavaScript, Python, Go, Java, C, C++, C#, Ruby, PHP, CSS, HTML, Bash, JSON

## Configuration

```rust
use navi_vfs::{VfsConfig, VfsEngine, LangId};

let config = VfsConfig {
    enabled: true,
    keep_comments: false,
    languages: vec![LangId::Rust, LangId::TypeScript],
};
let engine = VfsEngine::new(config);
```

When `languages` is empty, all Tier 1 languages are enabled.

## Part of the NAVI workspace

**Full project:** <https://github.com/navi-ai-org/navi>
**License:** Apache-2.0
