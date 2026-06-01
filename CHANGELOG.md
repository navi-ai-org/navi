# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial open-source release
- Interactive TUI with chat, command palette, model picker, thinking controls
- Multi-provider support (OpenAI, Anthropic, Gemini, OpenRouter, Groq, xAI, GitHub Copilot, Gitlawb)
- Agent modes: Plan, Edit, Review, Tutor, Socratic, Recall, Focus
- Built-in tools: read_file, write_file, apply_patch, grep, bash
- Specialized tools: test_runner, build_runner, fs_browser, git_ops, package_manager
- Security policy with path restrictions, command blocking, secret redaction
- Session persistence with auto-compaction
- MCP client integration
- ACP stdio server mode
- Headless mode for scripted use
- Native plugin system
- SDK for embedding NAVI in other applications
- Comprehensive documentation

### Changed
- N/A (initial release)

### Deprecated
- N/A (initial release)

### Removed
- N/A (initial release)

### Fixed
- N/A (initial release)

### Security
- Implemented secret redaction in session persistence
- Added path restrictions and command blocking
- Protected .git metadata from writes

## [0.1.0] - 2025-XX-XX

### Added
- Initial release

[Unreleased]: https://github.com/your-username/navi/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/your-username/navi/releases/tag/v0.1.0
