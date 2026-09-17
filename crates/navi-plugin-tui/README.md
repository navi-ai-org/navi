# navi-plugin-tui

A **third-party NAVI frontend** — another TUI, running in its own process.

It is not compiled into NAVI. The compose supervisor activates it as a `kind = ui`
component and spawns it with the terminal inherited; the plugin owns raw mode and
renders with crossterm only (no ratatui), which is the point: *a frontend is any
program that owns the terminal and speaks the contract*.

```
host (navi)                                   plugin (this binary)
├── spawns with inherited stdio ────────────► terminal: raw mode + render
├── listens on compose/ui/<id>-<gen>.sock ◄── connects back
├── {"type":"event", …} ───────────────────► event line in the UI
├── {"type":"quiesce"} ────────────────────► restore terminal, exit 0
└── {"type":"command","text":…} ◄─────────── user submitted a message (Enter)
```

Commands are admitted by the **kernel** (generation-checked): a message typed into
this TUI goes host → `UiCommandSink` → `Kernel::invoke` → engine, exactly like the
built-in frontends.

## Install and swap

```bash
cargo build -p navi-plugin-tui
sed "s|@PLUGIN_BINARY@|$PWD/target/debug/navi-plugin-tui|" \
    crates/navi-plugin-tui/manifest.toml > /tmp/plugin-tui.toml
navi components install /tmp/plugin-tui.toml

# while a session runs:
navi components reload navi.plugin.tui    # hand the terminal to this plugin
navi components reload navi.ui.tui        # hand it back to the built-in TUI
```

Keys: `Enter` sends the input line, `Esc` (or Ctrl+C) quits the frontend, which
ends the composed session.

## Notes

- The host keeps the exclusive terminal lease as *bookkeeping* for this tier (the
  child performs the OS effect); the successor can only start after this process
  exits, which the supervisor waits for (quiesce → exit → lease release).
- The plugin exits when its control socket closes, so a dead host does not leave
  it behind.
- `navi-plugin-tui` is deliberately tiny: ~300 lines, no ratatui, no tokio.
