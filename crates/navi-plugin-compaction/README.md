# navi-plugin-compaction

A **third-party NAVI engine part** — a compaction strategy that runs in its own
process. It is not compiled into NAVI: the compose supervisor activates it as a
`kind = compaction` component, the Matrix process host spawns it, and the host
installs a proxy into the engine's compaction slot. The engine then asks *this
process* what to compact.

## Contract (`navi.compaction@1`)

The plugin decides, the host applies — so the host keeps every field it owns
(timestamps, thinking blocks, multimodal parts) exactly as it was:

```json
// host → plugin
{"messages": [{"role":"tool","content":"…","tool_name":"read_file"}],
 "timestamps": [1758000000000], "gap_threshold_minutes": 60}

// plugin → host
{"drop": [1], "cleared": [2]}
```

* `drop` — indices to remove entirely;
* `cleared` — indices whose content is emptied (the message and its turn stay).

`auto_compact` / `force_compact` stay with the built-in strategy: a plugin owns
the *micro* policy, and the engine keeps a working full-compaction path.

## Policy implemented here ("window")

* the newest `WINDOW = 4` messages are never touched;
* stale tool results (older than the gap) are **dropped**;
* stale assistant narration is **cleared** (the turn stays, the prose goes);
* system messages are never touched.

## Install and swap

```bash
cargo build -p navi-plugin-compaction
sed "s|@PLUGIN_BINARY@|$PWD/target/debug/navi-plugin-compaction|" \
    crates/navi-plugin-compaction/manifest.toml > /tmp/plugin-compaction.toml
navi components install /tmp/plugin-compaction.toml

# while a session runs:
navi components reload navi.plugin.compaction   # the plugin owns the slot
navi components reload navi.compaction.default  # hand it back (new generation)
```

Ask the live plugin what it would do (no credentials needed):

```bash
navi components invoke navi.plugin.compaction navi.compaction@1 \
  --input '{"messages":[{"role":"tool","content":"…","tool_name":"read_file"}],
            "timestamps":[0],"gap_threshold_minutes":60}'
# {"cleared": [], "drop": [0], "plugin_pid": 1234, "policy": "window", "window": 4}
```

`navi components invoke` routes the call through the **kernel** to that exact
component instance (an authenticated instance grant, so a stale generation can
never be reached by name).
