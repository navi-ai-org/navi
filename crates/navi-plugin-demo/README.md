# navi-plugin-demo

A **third-party NAVI component** — a tool plugin that runs *out of process*.

This binary is not compiled into NAVI. It speaks `matrix.component` v0.1 with the
NAVI process host (`matrix-host`): the host spawns it with a launch token and a
Unix socket, and NAVI reaches it through the kernel like any other component.

| Capability | Meaning |
|---|---|
| `navi.plugin.info@1` | id, pid, generation, session — proof the work happens in another process |
| `navi.tool.roll_dice.definition@1` | the tool definition NAVI exposes to the model |
| `navi.tool.roll_dice@1` | the tool itself (deterministic dice roller) |

## Install

```bash
# 1. build the plugin
cargo build -p navi-plugin-demo

# 2. materialize a manifest with the absolute binary path
sed "s|@PLUGIN_BINARY@|$PWD/target/debug/navi-plugin-demo|" \
    crates/navi-plugin-demo/manifest.toml > /tmp/roll-dice.toml

# 3. install it into the compose home
navi components install /tmp/roll-dice.toml

# 4. start a session: the plugin is activated, its tool joins the session
navi --ui navi.ui.tui
```

`navi components status` then shows the plugin as an `Active` component with its
generation, and the tool `roll_dice` is offered to the model for the session.

Swap the plugin implementation at runtime (a new generation = a fresh process):

```bash
navi components reload navi.plugin.roll_dice
```

## Manifest

`manifest.toml` in this directory is the install template. The `args` carry the
host markers `{sock}` and `{id}`, expanded per activation by the process host.

## Trust

Process components are the **explicitly trusted** tier (Matrix `CHARTER.md` #6):
the plugin runs in its own process, with a launch token, an empty environment and
no terminal. NAVI's in-process components are first-party code; third-party code
belongs here. A sandboxed profile (`bwrap` + seccomp via `matrix-guard`) is
available in Matrix for operators who want stronger isolation.

## Writing your own

```rust
use matrix_component::{CallCtx, CallOutcome, Component, Handler};

struct Echo;
impl Handler for Echo {
    fn on_call(&self, _ctx: &CallCtx, _ticket: &str, _cap: &str, input: &serde_json::Value, _cancel: &std::sync::atomic::AtomicBool) -> CallOutcome {
        CallOutcome::Ok(input.clone())
    }
}

let component = Component::connect(std::path::Path::new(&sock), "navi.plugin.echo")?;
component.serve(Echo)?;
```

Conventions NAVI expects:

- component id: `navi.plugin.<name>`;
- a tool capability `navi.tool.<tool>@1` plus its definition
  `navi.tool.<tool>.definition@1` (one definition per tool, so several plugins
  coexist without capability ambiguity);
- the definition response: `{name, description, capability, input_schema}`.
