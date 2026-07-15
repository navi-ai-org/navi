## Bug Fixes

- **Edit/write path agency** — absolute paths are accepted (including after security path normalization). Project path jail is forced only in **Restricted** mode; AcceptEdits / Auto / YOLO keep agency unless `restrict_paths_to_project` is explicitly enabled
- **Tool error cards** — structured failures (`error`, `error_code`, `hint`) render as plain `Code:` / `Hint:` text instead of dumping the whole envelope as ` ```json `
- **Global shortcuts** — Ctrl+letter chords work across terminal encodings (e.g. Ctrl+M as CR / empty Ctrl+Enter opens the model picker)

## Performance

- Unit/CI graphs no longer pull **candle embeddings** by default (`navi-cli` / `navi-napi` opt in for product builds)
- **`navi-voice` onnx** is opt-in (no ort-sys on ordinary test builds)
- Release test gate: no step timeout; skip full `navi` bin link; ignore real package-manager process tests; `run_pkg` 30s cap

## Bindings

- `@navi-agent/napi` **0.2.6** and platform packages
- Workspace crate versions bumped to **0.2.6**

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh -s -- --version 0.2.6
```

## Changelog

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.5...v0.2.6
