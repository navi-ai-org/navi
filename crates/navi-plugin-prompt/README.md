# navi-plugin-prompt

A **third-party NAVI system-prompt builder**, running in its own process. It is
not compiled into NAVI: the compose supervisor activates it, the host installs a
proxy into the engine's prompt slot, and the engine asks *this process* to render
the base instructions for every turn.

## Contract (`navi.prompt@1`)

`PromptBuilder::build` receives a `SystemPromptInput` that carries live objects
(the prompt cache, the whole config), so the host sends a **projection** — the
data a builder needs — instead of the struct:

```json
// host → plugin
{"op":"build",
 "project_dir":"/path/to/project",
 "memory":null,
 "harness_card":null,
 "system_prompt":null,
 "tools":["read_file","edit"],
 "context_packets":[…],
 "available_skills":[…],
 "active_skills":[…]}

// plugin → host
{"instructions":"…",
 "developer_messages":[{"role":"developer","content":"…"}]}
```

* `instructions` becomes the base system prompt;
* `developer_messages` are injected as separate developer-role messages (the
  engine's cache-friendly path);
* an empty or unreadable answer makes the engine fall back to its own builder —
  a broken prompt component degrades instead of breaking the session.

## What this example does

Renders a distinct base identity (naming itself and its pid, the project, and how
many tools are available), appends remembered context when the host sends it,
honours a host-supplied `system_prompt` override, and emits developer blocks with
the active skills and the project root. Every build is logged with the generation
it served.

## Install and swap

```bash
cargo build -p navi-plugin-prompt
sed "s|@PLUGIN_BINARY@|$PWD/target/debug/navi-plugin-prompt|" \
    crates/navi-plugin-prompt/manifest.toml > /tmp/plugin-prompt.toml
navi components install /tmp/plugin-prompt.toml

# while a session runs:
navi components reload navi.plugin.prompt    # the plugin renders the prompt
navi components reload navi.prompt.branded   # hand the slot back
```

Ask the live plugin what it would render:

```bash
navi components invoke navi.plugin.prompt navi.prompt@1 \
  --input '{"op":"build","project_dir":"/tmp","tools":["read_file"],"active_skills":[]}'
```
