## Highlights since 0.2.3

**0.2.6** is a small reliability/CI patch. If you are jumping from **0.2.3** (or earlier), most of the product delta shipped in **0.2.4** and **0.2.5**. Summary of everything in `v0.2.3…v0.2.6` (~87 commits):

### Plugins & marketplace

- WASM-only plugins via `wasmtime` + host brokers (native libloading path removed)
- Marketplace catalog, signed example packages, install side effects for skills/MCP
- Host-mediated TUI extensions (`tui.json` palette commands)
- WASM runtime on by default with end-to-end install path

### Browser

- Built-in `browser` tool (Cloak/CDP); `navi browser status|doctor|install`
- Server routes and TUI status hubs

### Voice

- Remote dictation: OpenAI, Groq, Wispr Flow
- `[voice]` config + registry transcription catalog; SDK/CLI doctor and wire-up
- Local ASR surface remains available; ONNX stays optional for portable builds

### Skills & agent tools

- Modular SQLite skill store with CRUD / manage tools (filesystem `SKILL.md` discovery removed)
- **String-replace `edit`** (multi-edit) as the preferred coding path
- **Lean Direct tool schema** — power tools via `tool_search`
- **`repo_explore`** as BM25 + symbol search (not a subagent)
- Bash file dumps (`sed`/`cat`/`rg`/`ls`) redirect to native tools
- Removed `process`, `verifier`, `branch_race_start`

### Registry & providers

- Remote canonical model catalog sync and provider base resolution
- Model-specific effort levels (no adaptive thinking / tutor mode)
- xAI Grok Build OAuth routing + weekly usage; Charm Hyper prefix-cache fixes

### Sessions & reliability

- Rewind history when editing a past user message
- Persist partial assistant output on turn error; mid-stream prefill resume
- Kill timed-out process trees (no hung “Waiting for model”)
- Subagent hardening + live progress after background spawn

### TUI

- Desktop notifications for finished unfocused jobs
- Self-update + About modal; expanded setup wizard (approvals + marketplace tip)
- Plan as modal + live progress strip; Ctrl+Down jump to latest
- Queue UX (remove / chip / draft preserve); recap capped to 3 lines
- Paste text/images while streaming; image lightbox; usage/palette/settings/MCP polish
- **0.2.6:** structured tool errors as Code/Hint (not raw JSON); Ctrl+M and global chords fixed

### Path agency (**0.2.6**)

- Absolute paths accepted for edit/write after normalization
- Project path jail forced only in **Restricted**; AcceptEdits / Auto / YOLO keep agency unless `restrict_paths_to_project` is set

### SDK / NAPI / Dart

- Full engine surface: voice, memory, MCP, skills, plugins, accounts, rewind, updates
- Docker binding verifier; platform packages for `@navi-agent/napi` **0.2.6**

### Performance & CI

- Less session bloat / SQLite thrash / streaming TUI cost
- Lean unit graphs (embeddings/onnx opt-in for product bins); release test gate without full bin link or step timeout
- CI no longer on bare `main` pushes; Windows-safe registry model filenames

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh -s -- --version 0.2.6
```

## Changelog

- Full range since 0.2.3: https://github.com/navi-ai-org/navi/compare/v0.2.3...v0.2.6
- This tag only: https://github.com/navi-ai-org/navi/compare/v0.2.5...v0.2.6
- See also [CHANGELOG.md](https://github.com/navi-ai-org/navi/blob/v0.2.6/CHANGELOG.md)
