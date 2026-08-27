## Highlights

**0.7.0** ships a complete web frontend for `navi-server` — a Svelte 5 + Vite SPA embedded at compile time via rust-embed, with a mobile-first agent workspace UI including streaming chat, tool timelines, model picker, session sidebar with activity monitoring, and full runtime event coverage. This release also adds TUI kaomoji activity animations, fixes XDG config loading on Windows, preserves reasoning content across session restore, and stops final-word loss on provider stream end.

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.6.0...v0.7.0

### Web frontend

- **Embedded SPA** — `navi-server` now serves a Svelte 5 + Vite + TypeScript
  web frontend embedded via `rust-embed`, making the binary self-contained
  with the UI at compile time. `--web-dir` overrides with filesystem assets.
- **Mobile agent workspace UI** — model picker modal with connected-provider
  filtering, provider groups, search, current-model state, and recent-model
  history; mobile-friendly tool timeline with semantic actions, live status,
  and expandable details; streaming thinking panel; session sidebar with
  activity monitoring, workspace grouping, search, cached loading, inline
  approvals, title expansion, and long-press rename.
- **Full runtime event coverage** — `ChatView` handles all 19
  `RuntimeEventKind` variants (was 8): thinking deltas, approval / question /
  plan-review / sudo cards, token usage, auto-compact notifications, and
  agent-mode changes.
- **Session and tool state reconciliation** — transient tool states are
  reconciled on turn completion, session save, cancellation, response
  completion, and errors so stale spinners never remain visible.
- **Session history restore** — saved sessions load with full message history
  by parsing `snapshot.events`; active sessions fetch
  `/sessions/:id/snapshot` when switching. `RuntimeEventKind` types corrected
  to match Rust's externally-tagged enum serialization.
- **Markdown rendering** for assistant messages via `marked` with dark-theme
  styling, copy buttons on code blocks, and styled tables/blockquotes.

### TUI

- **Narrative kaomoji activity animations** — fixed-width animations for idle,
  thinking, streaming, and tool activity in the composer status line, with
  longer success and error transitions that return to idle after the turn.
- **Compact four-state indicator** — idle (sleeping), working (pencil
  sliding), success (celebration held 3s), error (table flip). All loading
  states share one unified writing animation; the text label reflects the
  real phase.
- **Alt+punctuation fix** — Alt+comma and Alt+period no longer consumed as
  camel-hump navigation shortcuts, so compose-key sequences (e.g. Alt+, → ç
  on US International) finally type the intended character.

### Fixed

- **XDG config on Windows** — `NaviConfig::load` now checks
  `~/.config/navi/config.toml` (and `$XDG_CONFIG_HOME/navi/`) before falling
  back to the platform-native `ProjectDirs` path. Previously, Windows users
  who placed their config at `~/.config/navi/config.toml` (as documented in
  AGENTS.md) had it silently ignored.
- **Final-word loss on stream end** — provider stream now flushes pending
  text before emitting `ModelStreamEvent::Done`, so text held back as a
  potential tool-call marker prefix is emitted when the provider closes the
  HTTP body without a `[DONE]` sentinel. `ThinkTagSplitter::drain_pending`
  no longer drops partial `</think` tag prefixes on stream end.
- **Reasoning content across session restore** — providers requiring
  `reasoning_content` (e.g. DeepSeek thinking mode) no longer reject rebuilt
  history after session restore. A new persisted `ToolTurnThinking` event
  carries the reasoning trace of tool-call steps through reload.

### Bindings

- `@navi-agent/napi` **0.7.0** and platform packages
- `@navi-agent/navi` **0.7.0** CLI packages
- Workspace crate versions bumped to **0.7.0**
- `index.d.ts` synced with `setGoalForHostTurn`, `buildHostSetGoalUserPrompt`,
  `agentMode`, `enterPlanMode`, `exitPlanMode`, `acpServer` /
  `NaviNapiAcpServer`, and computer-use methods.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh -s -- --version 0.7.0
```

```bash
npm install -g @navi-agent/navi@0.7.0
npm install @navi-agent/napi@0.7.0
```

## Changelog

- Tag range: https://github.com/navi-ai-org/navi/compare/v0.6.0...v0.7.0
- See [CHANGELOG.md](https://github.com/navi-ai-org/navi/blob/v0.7.0/CHANGELOG.md)
