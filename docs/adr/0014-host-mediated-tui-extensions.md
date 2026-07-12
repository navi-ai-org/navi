# ADR 0014 — Host-Mediated TUI Extensions (`tui.json`)

## Status
Accepted

## Context
WASM plugins cannot receive a host `ratatui::Frame`. Community packages still
need a way to contribute commands, simple panels, and theme tokens without
rebuilding NAVI or loading native code.

## Decision
Packages may ship a `tui.json` next to `plugin.toml` / `plugin.wasm`:

```json
{
  "commands": [
    { "id": "pkg.cmd", "title": "Title", "description": "…", "palette_group": "Extensions" }
  ],
  "panels": [
    { "id": "pkg.panel", "title": "Panel", "kind": "info", "body": "markdown/text" }
  ],
  "theme_tokens": { "accent": "#7C5CFF" }
}
```

The engine loads these via `list_tui_extensions` / `list_tui_extension_commands`.
The TUI is the sole renderer: it maps specs to palette rows, modals, and theme
overrides. Plugins never paint directly.

## Consequences
Positive:
- Safe for marketplace (data-only)
- Same package works for TUI and future Tutor UI hosts
- Easy to validate in CI

Negative:
- Not as powerful as native widgets
- Command dispatch is host-defined (ids are namespaced strings)
