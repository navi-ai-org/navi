## New Features

- **Desktop notifications** — OS toasts when a turn/goal finishes while the terminal is unfocused (`tui.desktop_notifications`, default on)
- **SDK / NAPI / Dart surface sync** — remaining `NaviEngine` methods bound for Tutor and mobile clients (TUI extensions, plugin install-with-meta, accounts, routing models, `memory_update`, voice events, session rewind, updates)
- **Registry** — remote canonical model catalog sync and provider base resolution
- **`repo_explore`** — BM25 + symbol search as a real tool (not a subagent)
- **Model-specific effort options** exposed to SDK/NAPI clients

## Bug Fixes

- **Tool timeouts no longer hang the agent** — kill the full process group on bash/process deadline, mark timed-out background tasks `ok=false`, and stop the turn from sitting on “Waiting for model”
- **Duplicate tool error text** — failed tool cards no longer show the same error in both the header and body
- **xAI / Grok OAuth** — match Grok CLI routing headers for subscription billing
- **Charm Hyper prompt cache** — stop isolating cache keys per session (restore prefix hits)
- **CI** — workflows no longer run on bare pushes to `main` (tags, PRs, and manual dispatch only)
- TUI: usage/command palette/settings polish, MCP status, diffs, paste while streaming, subagent progress, plan progress strip

## Bindings

- **`@navi-agent/napi` `0.2.4`** — full engine surface + `loadedConfig().tui.desktopNotifications`
- **`navi-dart` `0.2.4`** — C ABI gap-fill for accounts, memory update, voice events, plugins, updates
- Docker binding harness: `scripts/test-bindings-docker.sh`

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh -s -- --version 0.2.4
```

## Changelog

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.3...v0.2.4
