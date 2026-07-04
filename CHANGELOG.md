# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.2] - 2026-07-04

### Added

- Added multimodal `ContentPart` support for images, audio, video, and documents across the engine and SDK-facing APIs.
- Added per-modality attachment fallback model configuration for image, audio, video, and document analysis.
- Added the `analyze_attachment` SDK host tool so the chat model can delegate unsupported attachments to configured specialist models.
- Added N-API typings/config support for structured content parts and attachment fallback models.
- Added registry attachment metadata with provider-level `defaults.attachments` and per-model `attachments` overrides.

### Changed

- The turn builder now rewrites unsupported attachments into model-readable tool instructions instead of sending unsupported media directly.
- Gemini requests now serialize image, audio, video, and document content as native inline data parts.
- Anthropic requests now support native image and PDF document parts where available.
- TUI root layout now clips the footer/composer stack inside small terminal viewports to avoid overlap with chat content.

### Fixed

- Restored compact image indicators when rendering user messages with image content.
- Avoided spawning registry background tasks without an active Tokio runtime.

## [0.1.0] - 2025-XX-XX

### Added

- Initial open-source release
- Interactive TUI with chat, command palette, model picker, thinking controls
- Multi-provider support (OpenAI, Anthropic, Gemini, OpenRouter, Groq, xAI, GitHub Copilot, Gitlawb)
- Agent modes: Plan, Edit, Review, Tutor, Socratic, Recall, Focus
- Built-in tools: read_file, write_file, apply_patch, grep, bash
- Specialized tools: test_runner, build_runner, fs_browser, package_manager
- Security policy with path restrictions, command blocking, secret redaction
- Session persistence with auto-compaction
- MCP client integration
- ACP stdio server mode
- Headless mode for scripted use
- Native plugin system
- SDK for embedding NAVI in other applications
- Comprehensive documentation

### Security

- Implemented secret redaction in session persistence
- Added path restrictions and command blocking
- Protected .git metadata from writes

[Unreleased]: https://github.com/navi-ai-org/navi/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/navi-ai-org/navi/compare/v0.1.0...v0.1.2
[0.1.0]: https://github.com/navi-ai-org/navi/releases/tag/v0.1.0
