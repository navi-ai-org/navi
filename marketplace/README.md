# NAVI marketplace (vendored layout)

Mirrors **https://github.com/navi-ai-org/navi-marketplace**.

Default catalog:

```text
https://raw.githubusercontent.com/navi-ai-org/navi-marketplace/main/catalog.json
```

## Runtime

**WASM-only.** No native `.so` packages. Local path installs use `LocalDev`
trust (signature optional). Marketplace installs require Community signatures.

## Package kinds

| Kind | Catalog | Install side effects |
|------|---------|----------------------|
| `plugin` | `"kind": "plugin"` | WASM tools load on session |
| `skill` | `"kind": "skill"` | + import `SKILL.md` / `skill.toml` → `skills.sqlite` |
| `mcp` | `"kind": "mcp"` | + merge `mcp.json` into global MCP config |
| `integration` | `"kind": "integration"` | WASM + optional env checklist |

Optional host UI: `tui.json` (commands / panels / theme tokens).

## Layout

```txt
catalog.json
scripts/validate_catalog.py
.github/workflows/validate.yml
artifacts/<id>/<version>/plugin.toml
artifacts/<id>/<version>/plugin.wasm
artifacts/<id>/<version>/SKILL.md      # skill
artifacts/<id>/<version>/mcp.json      # mcp
artifacts/<id>/<version>/tui.json      # optional TUI protocol
examples/…                             # LocalDev skeletons (unsigned)
```

## Packages in catalog

| Id | Kind | Notes |
|----|------|--------|
| `hello-echo` | `plugin` | Signed WASM echo tool |
| `discord` | `mcp` | `@iqai/mcp-discord` + bot token setup (`INSTALL.md`) |

## Local catalog (E2E)

Signed demos live under `artifacts/`.

```bash
# Point NAVI at the local catalog (global config):
# [plugin_marketplace]
# registry_url = "file:///ABS/PATH/to/navi/marketplace/catalog.json"

navi plugin search echo
navi plugin install-marketplace hello-echo --yes
# new session → tool plugin__hello-echo__echo (or namespaced equivalent)

# Or path install (LocalDev, signature optional):
navi plugin install marketplace/artifacts/hello-echo/0.1.0 --yes
```

Re-sign after editing WASM:

```bash
cargo run -p navi-plugin-manifest --example sign_local_package -- \
  marketplace/artifacts/hello-echo/0.1.0
```

## Validate

```bash
python marketplace/scripts/validate_catalog.py
```

## Config

```toml
[plugin_marketplace]
registry_url = "https://raw.githubusercontent.com/navi-ai-org/navi-marketplace/main/catalog.json"
```

Publish to **navi-ai-org/navi-marketplace**, not only this vendored copy.
