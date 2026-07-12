# NAVI marketplace (vendored layout)

This directory mirrors the official marketplace repository:

**https://github.com/navi-ai-org/navi-marketplace**

Default catalog URL:

```text
https://raw.githubusercontent.com/navi-ai-org/navi-marketplace/main/catalog.json
```

## Runtime rule

**All packages install as WASM plugins.** There is no native (`.so` / `.dylib`)
marketplace path. Local development uses the same WASM format with `LocalDev`
trust (`navi plugin install ./path` — signature optional).

## Package kinds

| Kind | Catalog field | What install does |
|------|---------------|-------------------|
| `plugin` | `"kind": "plugin"` | WASM tools → `ToolExecutor` on next session |
| `skill` | `"kind": "skill"` | WASM package; activate skills in session as needed |
| `mcp` | `"kind": "mcp"` | WASM package; merge optional `mcp.json` into global MCP config |
| `integration` | `"kind": "integration"` | WASM package (bots may need env secrets) |

UX is unified (`navi plugin install-marketplace <id>`); runtime is always WASM.
Kind is stored in the lockfile for list/search display and install hints.

## Local layout

```txt
catalog.json
artifacts/<plugin-id>/<version>/plugin.toml
artifacts/<plugin-id>/<version>/plugin.wasm
artifacts/<plugin-id>/<version>/mcp.json   # optional, kind=mcp
```

Example skeleton (unsigned LocalDev fixture — not for marketplace publish):

```txt
examples/hello-echo/plugin.toml
```

## Config override

```toml
[plugin_marketplace]
registry_url = "https://raw.githubusercontent.com/navi-ai-org/navi-marketplace/main/catalog.json"
```

Publish to **navi-ai-org/navi-marketplace**, not only this vendored copy.
