# navi-plugin-api

[![Crates.io](https://img.shields.io/crates/v/navi-plugin-api)](https://crates.io/crates/navi-plugin-api)
[![License](https://img.shields.io/crates/l/navi-plugin-api)](../LICENSE)

Stable plugin ABI for [NAVI](https://github.com/navi-ai-org/navi) — the trait definitions and types that plugin authors implement.

This crate is intentionally minimal: it has **no dependency on `navi-core`** and defines only the types needed across the plugin boundary. The host adapts `PluginTool` into `navi_core::Tool` via [`navi-plugin-host`](https://crates.io/crates/navi-plugin-host).

## Plugin ABI

```rust
use navi_plugin_api::{NaviPlugin, PluginRegistry, PluginTool, PluginToolDefinition, PluginToolKind};

struct MyPlugin;

impl NaviPlugin for MyPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            name: "my-plugin".into(),
            version: "0.1.0".into(),
            api_version: NAVI_PLUGIN_API_VERSION,
            ..Default::default()
        }
    }

    fn register(&self, registry: &mut dyn PluginRegistry) -> Result<(), String> {
        registry.register_tool(Arc::new(MyTool));
        Ok(())
    }
}
```

## Key types

| Type | Description |
|------|-------------|
| `NaviPlugin` | Top-level plugin trait — metadata + tool registration |
| `PluginRegistry` | Registration interface for tools, policies, and components |
| `PluginTool` | Self-contained tool trait (definition + invoke) |
| `PluginToolDefinition` | Name, description, kind, and JSON schema |
| `PluginToolKind` | `Read`, `Write`, `Command`, or `Custom` |
| `PluginToolInvocation` | Invocation id, tool name, and JSON input |
| `PluginToolResult` | Invocation id, success flag, and JSON output |

## ABI versioning

The constant `NAVI_PLUGIN_API_VERSION` (currently `2`) is checked at load time. The host rejects plugins with incompatible versions.

## Writing a plugin

1. Depend on `navi-plugin-api` (not `navi-core`)
2. Implement `NaviPlugin` and one or more `PluginTool`s
3. Export `navi_plugin_entrypoint` as a `#[no_mangle]` function
4. Build as a `cdylib` (`.so` / `.dylib` / `.dll`)

```toml
[lib]
crate-type = ["cdylib"]
```

## Part of the NAVI workspace

**Full project:** <https://github.com/navi-ai-org/navi>
**License:** Apache-2.0
