## New Features

- **TUI polish** — colored write/edit diffs, plain process/patch tool streams, boxed markdown tables
- **Jump-to-latest** control when scrolled up; hover **context %** chip on the usage meter
- **Session spend** — USD list-rate estimates + Hypercredits for Charm Hyper; persisted with the session
- **Session recap** + per-turn context token meter
- **Image hover previews** (Kitty / Sixel / iTerm2) and `[Image N]` chips in the composer
- **Local voice ASR** surface for desktop clients (`navi-voice`; ONNX optional via `--features voice-onnx`)

## Bug Fixes

- **Prompt cache / quota** — stop double system prompt on Chat Completions; stable tool schema order for prefix caching; cache-aware Hyper credit estimates
- **Multimodal Grok / xAI** — unknown SKUs inherit provider vision defaults; **Ctrl+R sync** enriches new model ids (e.g. `grok-4.5`) from defaults + family siblings instead of bare `NULL` rows
- **Registry** — `grok-4.5` / `grok-4.20` with `supports_images` in the xAI snapshot
- **Context meter** — include cached tokens from aggregator usage reports (no more bogus ~430 / 1M)
- **Charm Hyper** — credits balance reporting + embedded pricing fallback when SQLite pricing rows are missing
- **sudo detection** — only treat `sudo` as a command word, not a plain argument
- **Portable builds** — ONNX Runtime voice is optional so musl Linux and other hosts link without prebuilt `ort-sys`

## CI / Release

- CI runs on **push to main**, **PRs**, and **version tags** (`fmt` · `check` · `clippy` · tests)
- Release workflow **gates on tests** before multi-platform builds and publish
- Release notes taken from `.github/release-notes-v*.md` / `CHANGELOG.md` (no more stub body)

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh -s -- --version 0.2.3
```

## Changelog

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.2...v0.2.3
