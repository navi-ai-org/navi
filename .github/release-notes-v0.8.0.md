## Highlights

**0.8.0 is a breaking cleanup release.** Four subsystems that carried a large
part of the build graph were removed, and the SQLite access layer was rewritten
on a pure-Rust engine:

- **Plugins are gone.** The WASM plugin runtime, marketplace, CLI commands,
  TUI panels and config keys were removed, taking `wasmtime` and `cranelift-*`
  out of the dependency tree.
- **Local ML embeddings are gone** (`candle-*`, `tokenizers`, `hf-hub`).
  Auto-memory search is textual and still works out of the box.
- **Local ONNX voice is gone** (`ort`). Voice is now remote transcription
  (OpenAI/Groq-compatible) plus recorder diagnostics.
- **Terminal inline image rendering is gone** (`ratatui-image`). Attachments and
  model vision are unaffected.
- **`rusqlite` (C SQLite) was replaced by `turso`** (pure-Rust SQLite). Same
  database files, same paths, no data migration.
- **`aws-lc-rs` was replaced by `ring`** for TLS — it was the single largest C
  build in the workspace and the source of link failures with mold.

Dev builds are the visible win: with Cranelift (dev profile only), mold and no
debug info for external dependencies, the debug binary shrank from ~385 MB to
~180 MB and incremental rebuilds are much faster.

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.7.3...v0.8.0

### Fixed

- **Interrupted turns no longer poison the next request.** A tool call left
  unanswered (Esc/cancel, model switch, crash) or kept by a rewind could make
  every later provider request fail with `assistant message with 'tool_calls'
  must be followed by tool messages`. The engine now repairs tool-call/result
  pairing before each turn (synthesizing an interrupted-result for unanswered
  calls, dropping orphan results) and the TUI applies the same repair before
  seeding the engine.

### Removed

- **WASM plugin system, in full** — the five `navi-plugin-*` crates, `navi
  plugin` commands, the TUI marketplace/panels/approval modals, the
  `plugins`/`wasm_plugins`/`plugin_marketplace` config keys,
  `security.allow_external_plugins`, and the vendored `marketplace/` catalog.
- **Local ML embeddings** — `candle-core`/`-nn`/`-transformers`, `tokenizers`,
  `hf-hub`, the `esaxx-rs` patch, the `embeddings` feature and the model
  download flow. Auto-memory keeps working via textual search.
- **Local ONNX voice** — `ort`/`ort-sys`, the `voice-onnx` feature, local model
  lifecycle and streaming routes. Remote transcription and recorder diagnostics
  remain.
- **Terminal inline image rendering** — `ratatui-image` and the Kitty/Sixel/
  iTerm2 protocols + hover lightbox. `view_image`/`inspect_image` and pasted
  attachments are unchanged.
- **`aws-lc-rs`** — TLS now uses the audited `ring` provider
  (`reqwest` with `rustls-no-provider`, provider installed by `navi-core`);
  platform certificate verification is preserved.

### Changed

- **Storage engine: `rusqlite` → `turso`** (pure Rust). Database files and
  paths are unchanged; `turso` reads existing SQLite files directly. Registry
  cleanup also removes `.tshm` sidecars.
- **Dev toolchain**: Cranelift codegen for the `dev` profile (release keeps
  LLVM), mold linker, no debug info for external dependencies. Plus the
  `[profile.dev.package."*"]` debug trimming.
- Docs/AGENTS/README updated; plugin ADRs marked Superseded.
