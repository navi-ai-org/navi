# navi-plugin-host

[![Crates.io](https://img.shields.io/crates/v/navi-plugin-host)](https://crates.io/crates/navi-plugin-host)
[![License](https://img.shields.io/crates/l/navi-plugin-host)](../LICENSE)

Dynamic native plugin loader for [NAVI](https://github.com/navi-ai-org/navi).

`navi-plugin-host` loads `.so` / `.dylib` plugin libraries at runtime via [`libloading`](https://crates.io/crates/libloading), adapts their `PluginTool` implementations into NAVI's `Tool` trait, and registers them with the engine's `ToolExecutor`.

## How it works

1. **Load** — opens the shared library and looks up the `navi_plugin_entrypoint` symbol
2. **Version check** — rejects plugins with incompatible `NAVI_PLUGIN_API_VERSION`
3. **Register** — calls `NaviPlugin::register()` with a host-side `PluginRegistry`
4. **Adapt** — wraps each `PluginTool` in a `PluginToolAdapter` that implements `navi_core::Tool`
5. **Secure** — validates every tool invocation through `SecurityPolicy` before forwarding

## Security

Plugin tool invocations go through the same `SecurityPolicy` as built-in tools:

- `Custom` tools require approval by default
- Per-tool allow/ask/deny rules apply
- Trusted plugin locations are enforced unless `allow_external_plugins = true`
- Optional filesystem sandboxing via Landlock (Linux)

## Optional features

| Feature | Description |
|---------|-------------|
| `landlock` | Enable Landlock filesystem sandboxing on Linux |

## Part of the NAVI workspace

This crate depends on [`navi-core`](https://crates.io/crates/navi-core), [`navi-plugin-api`](https://crates.io/crates/navi-plugin-api), and [`libloading`](https://crates.io/crates/libloading).

**Full project:** <https://github.com/navi-ai-org/navi>
**License:** Apache-2.0
