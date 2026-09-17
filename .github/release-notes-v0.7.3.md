## Highlights

**0.7.3** fixes two TUI regressions that only showed up in Ghostty. Alt-tabbing
away from NAVI and back could freeze the screen blank and leave every later
update flickering, because focus recovery issued a blocking cursor-position
query *inside* a synchronized-update bracket (classic DSR-inside-sync
deadlock). And moving the pointer sped the animations up without bound, because
the frame pacer had coupled the animation clock to input events.

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.7.2...v0.7.3

### Fixed

- **Ghostty: blank freeze and flicker after returning focus** — `FocusGained`
  called `Terminal::clear()`, which issues a cursor-position query (DSR,
  `ESC [ 6 n`) and **blocks reading the reply from stdin**. It ran inside a
  `?2026` synchronized-update bracket, so a terminal that defers the reply
  while buffering never answers: the app waited for a reply that would only
  arrive after the `?2026l` it was itself holding open. The frozen frame is the
  blank screen, and the recovery attempts produced the per-update flicker.
  Focus recovery now clears the viewport and resets the diff buffers with plain
  writes (`Terminal::resize` path) and repaints every cell in a **single** sync
  bracket — no cursor query anywhere on the focus path. Verified with a pty
  harness: zero `ESC [ 6 n` on focus-out/in cycles, clear paired with a full
  repaint in the same frame.

- **Animations speeding up with pointer movement** — the frame pacer moved
  `advance_tick` out of the draw block, so every loop iteration (including each
  mouse-motion event) advanced the animation clock. Render code treats one tick
  as ~80ms (`app.tick().saturating_mul(80)`), so hovering the pointer ran
  tick-driven animations (idle kaomoji, background cards) hundreds of times per
  second. The tick now advances on a fixed 80ms wall-clock cadence, decoupled
  from both input volume and the paced draw loop. Regression test: 128 scripted
  motion events advance the clock at most a step or two.

### Changed

- **Synchronized output stays on for every terminal, Ghostty included.** An
  earlier attempt in this cycle skipped the `?2026` bracket on Ghostty;
  presenting frames without sync made its eager renderer show partial frames
  (constant tearing), which is worse than the sync-buffer quirks. The bracket
  now closes through a drop guard, so a panicking draw cannot leave the
  terminal buffering updates, and the restore path closes it before leaving the
  alternate screen. `NAVI_SYNCHRONIZED_OUTPUT=0/1` (or `true`/`false`,
  `on`/`off`) remains as a manual override.

### Upgrade

```sh
curl -fsSL https://github.com/navi-ai-org/navi/raw/refs/heads/main/scripts/install.sh | sh
```

Or pin the version:

```sh
curl -fsSL https://github.com/navi-ai-org/navi/raw/refs/heads/main/scripts/install.sh | sh -s -- --version v0.7.3
```

### Verification

- `navi-tui`: 466 lib tests pass, including the new animation-clock regression
  test.
- pty harness on focus-out/in cycles: 0 `ESC [ 6 n`, clear + 2.3 KB full
  repaint inside one bracket, nothing outside a bracket.
- Owner-verified in Ghostty: focus cycle clean, mouse motion no longer
  accelerates animations.
