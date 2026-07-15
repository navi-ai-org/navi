## New Features

- **String-replace `edit` tool** — preferred coding edit path, with multi-edit via `edits[]`
- **Lean Direct tool schema** — small core tool surface; power tools discovered with `tool_search`
- **Message queue UX** — remove items, clickable/hoverable `N queued` chip, preserve input draft when draining
- **Session recap cap** — hard limit of 3 lines (no full-file dumps in recap)

## Changes

- Remove `process`, `verifier`, and `branch_race_start` agent tools
- Redirect common bash file dumps (`sed` / `cat` / `rg` / `ls` / …) to native tools (`read_file` / `search`)
- Drop rustquty quality-metrics tooling from the repo

## Bindings

- `@navi-agent/napi` **0.2.5** and platform packages
- Workspace crate versions bumped to **0.2.5**

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh -s -- --version 0.2.5
```

## Changelog

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.4...v0.2.5
