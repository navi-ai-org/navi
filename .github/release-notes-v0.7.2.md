## Highlights

**0.7.2** fixes the TUI blinking when the pointer moves over the terminal:
animation redraws are now paced instead of firing on every loop iteration,
so mouse-motion events can no longer flood the screen with synchronized
updates (148 → 19 redraws on the same motion sweep). It also refreshes the
OpenCode Zen/Go catalogs from the live APIs and restores the TUI snapshot
goldens that had gone stale.

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.7.1...v0.7.2

### Fixed

- **TUI blinking on mouse motion** — the draw gate was
  `needs_draw || … || idle_animating`, and `idle_animating` is true whenever
  the app is idle with a provider configured, so *every* loop iteration
  painted. With free-motion mouse reporting (`?1003`, enabled so drag-select
  works on terminals that only report motion there), a pointer sweep turned
  each mouse event into a full redraw. Animation redraws are now paced
  (≈30fps active, ≈10fps idle); input and state changes still paint
  immediately, `advance_tick` still runs on every candidate frame, and the
  loop draws one final frame on exit.
- **Stale TUI snapshot goldens** — 14 welcome/stream snapshots still
  described the pre-kaomoji layout, so the screenshot tests failed on any
  machine that reached them (CI never did: an earlier navi-core step fails
  first on Ubuntu). Regenerated and verified byte-identical to the intended
  baselines.

### Changed

- **OpenCode Zen/Go registry sync** — embedded snapshot bumped to
  navi-registry `0ece2a3`: catalogs refreshed from the live
  `zen/v1/models` and `zen/go/v1/models` endpoints, new canonical models
  (Claude Fable 5.1, Gemini 3.8 Flash, GPT 6 Astra, GLM 5.3 Flash, Kimi K3,
  LongCat 2.0, Qwen3.8 Flash, Muse Spark 1.3, Union Alpha, Hy4 preview, …),
  prices from the official Zen/Go tables, and the daily registry probe now
  covers the OpenCode endpoints so this stays current. The free-model
  allowlist gained `muse-spark-1.3-contributor(-free)` and
  `ling-3.0-flash-fin-free`.

### Upgrade

```sh
curl -fsSL https://github.com/navi-ai-org/navi/raw/refs/heads/main/scripts/install.sh | sh
```

Or pin the version:

```sh
curl -fsSL https://github.com/navi-ai-org/navi/raw/refs/heads/main/scripts/install.sh | sh -s -- --version v0.7.2
```

### Verification

- `navi-tui`: 463 lib tests + 23 screenshot goldens pass.
- `navi-core`: OpenCode/Zen catalog tests pass; `cargo fmt --check` clean.
- Motion-sweep pty measurement: 148 → 19 synchronized redraws
  (remaining draws are real hover changes).
