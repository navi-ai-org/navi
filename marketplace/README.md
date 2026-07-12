# NAVI marketplace (vendored layout)

This directory mirrors the official marketplace repository:

**https://github.com/navi-ai-org/navi-marketplace**

NAVI’s default catalog URL:

```text
https://raw.githubusercontent.com/navi-ai-org/navi-marketplace/main/catalog.json
```

All marketplace packages (plugins, skills, MCP adapters, messaging bots) install
as **WASM plugin packages**. They are **not** hardcoded in the NAVI binary.

## Local layout

```txt
catalog.json
artifacts/<plugin-id>/<version>/plugin.toml
artifacts/<plugin-id>/<version>/plugin.wasm
```

Publish changes to **navi-ai-org/navi-marketplace**, not only this vendored copy.

## Config override

```toml
[plugin_marketplace]
registry_url = "https://raw.githubusercontent.com/navi-ai-org/navi-marketplace/main/catalog.json"
```

See the marketplace repo README for publish guidelines and package kinds
(`plugin` | `skill` | `mcp` | `integration`).
