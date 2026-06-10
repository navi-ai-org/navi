# ADR 0009 — Plugin Marketplace and Distribution

## Status
Accepted

## Context
NAVI needs a mechanism for discovering, fetching, and installing community plugins.
Blind installation from arbitrary URLs is unsafe. The system needs a catalog-based
distribution model that integrates with the signing and verification infrastructure.

## Decision
Plugins are distributed through a registry-backed marketplace. The default registry
is a Git repository containing a `catalog.json` file and plugin artifact directories.

### Catalog Format

The registry serves a `PluginCatalog` JSON file:

```json
{
  "version": 1,
  "updated_at": "2026-06-01T00:00:00Z",
  "plugins": [
    {
      "id": "my-plugin",
      "name": "My Plugin",
      "description": "A useful plugin",
      "version": "1.0.0",
      "publisher": "gh:username",
      "artifact_dir": "artifacts/my-plugin",
      "wasm_hash": "sha256:..."
    }
  ]
}
```

### Registry Protocol

- Default registry: `https://raw.githubusercontent.com/navi-engine/plugin-registry/main/catalog.json`
- Supports `https://` (HTTP fetch) and `file://` (local filesystem) registry URLs
- Artifact directories are resolved relative to the catalog URL or as absolute URLs
- Each artifact directory contains `plugin.toml` (manifest) and the `.wasm` entry file
- The registry URL is configurable (`registry_url` in config)

### Supply-Chain Safety

The install pipeline enforces multiple integrity checks:

1. **Catalog-level WASM hash** (optional): `PluginCatalogEntry.wasm_hash` can declare
   the expected hash. If present, it is compared against the manifest's `wasm_hash`.
2. **Manifest-level WASM hash**: `plugin.wasm_hash` is verified against the actual
   downloaded `.wasm` bytes.
3. **Signature verification**: The manifest's Ed25519 signature is verified per ADR 0008.
4. **Manifest validation**: The manifest is validated against the `Community` trust level
   (WASM runtime only, no native code, no `ReadWrite` filesystem, etc.).
5. **Staging directory**: Artifacts are downloaded to a `.staging/<id>/` directory before
   being promoted to the installed location. This prevents partial installs.

### Search

`search_catalog` performs case-insensitive substring matching on `id`, `name`, and
`description` fields.

## Consequences
Positive:
- Single source of truth for installable plugins (the registry catalog)
- Multi-layer integrity verification (catalog hash, manifest hash, signature)
- Staging directory prevents partial/corrupt installs
- Local `file://` registry enables offline/private plugin development

Negative:
- Default registry is a Git repository — no dedicated registry server yet
- No dependency resolution between plugins
- No version constraint solving (single version per plugin in catalog)
- Registry availability depends on GitHub (for the default registry)
- No plugin signing infrastructure for publishers (manual key management)
