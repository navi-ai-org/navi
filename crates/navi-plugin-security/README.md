# navi-plugin-security

A **third-party NAVI security policy and session hooks**, running in its own
process. It is not compiled into NAVI: the compose supervisor activates it as a
process component, the host installs proxies into the engine's `security` and
`hooks` slots, and the engine asks *this process* before every tool invocation.

## Contracts

```json
// navi.security@1 — host → plugin
{"op":"validate_tool","tool":{"name":"web_fetch","description":"…"},"input":{"url":"…"}}

// plugin → host
{"decision":"deny","reason":"…"}      // block the invocation
{"decision":"allow"}                  // allow it (overrides the base policy)
{"decision":"approval","risk":"command"}
{"decision":"abstain"}                // no opinion: the base policy decides

// navi.hooks@1 — host → plugin (fire and forget)
{"op":"event","event":"tool_call","tool":"read_file"}
{"op":"event","event":"session_start","session_id":"…"}
{"op":"event","event":"turn_end","session_id":"…","output_chars":123}
```

Two properties matter for a security seam:

* **`abstain` is the safe default** — a plugin adds restrictions without having
  to re-implement the host's rules (path scopes, plan mode, approval ladder).
* **A silent plugin is ignored** — if it stops answering, the engine falls back
  to its default policy. A broken security component never silently allows
  everything.

## Policy implemented here

Deny by tool **name or prefix**, from `--deny` (default `run,web_,browser`),
abstain for everything else, and log every decision. `--log` is where a test
proves the answer came from this process.

## Install and swap

```bash
cargo build -p navi-plugin-security
sed "s|@PLUGIN_BINARY@|$PWD/target/debug/navi-plugin-security|" \
    crates/navi-plugin-security/manifest.toml > /tmp/plugin-security.toml
navi components install /tmp/plugin-security.toml

# while a session runs:
navi components reload navi.plugin.security   # the plugin decides on tools
navi components reload navi.hooks.audit       # hand the hooks slot back
```

Ask the live plugin what it would decide (no credentials needed):

```bash
navi components invoke navi.plugin.security navi.security@1 \
  --input '{"op":"validate_tool","tool":{"name":"web_fetch"},"input":{}}'
# {"decision":"deny","plugin_pid":1234,"reason":"denied by navi.plugin.security (policy in another process, pid 1234)"}
```
