# NAVI Plugin Registry

This directory is the layout for the **plugin marketplace repository**. Publish it as a Git repo; NAVI fetches `catalog.json` from the raw URL.

## Layout

```txt
catalog.json
artifacts/<plugin-id>/<version>/plugin.toml
artifacts/<plugin-id>/<version>/plugin.wasm
```

## Catalog entry

Each plugin in `catalog.json`:

```json
{
  "id": "my-plugin",
  "name": "My Plugin",
  "description": "Short summary",
  "version": "1.0.0",
  "publisher": "gh:your-org",
  "artifact_dir": "artifacts/my-plugin/1.0.0",
  "wasm_hash": "sha256:..."
}
```

`artifact_dir` is relative to the catalog URL. Optional `wasm_hash` must match the manifest when set.

## NAVI config

Override the registry in global `~/.config/navi/config.toml`:

```toml
[plugin_marketplace]
registry_url = "https://raw.githubusercontent.com/your-org/your-registry/main/catalog.json"
```

Default: `navi-plugin-manifest::DEFAULT_REGISTRY_URL`.