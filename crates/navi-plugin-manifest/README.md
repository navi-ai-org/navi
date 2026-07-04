# navi-plugin-manifest

[![Crates.io](https://img.shields.io/crates/v/navi-plugin-manifest)](https://crates.io/crates/navi-plugin-manifest)
[![License](https://img.shields.io/crates/l/navi-plugin-manifest)](../LICENSE)

Plugin manifest parsing, validation, signing, and marketplace catalog for [NAVI](https://github.com/navi-ai-org/navi).

This crate is the **single source of truth** for plugin metadata: it defines the manifest format, validates integrity, manages lockfiles, and interfaces with the plugin marketplace registry.

## Modules

| Module | Purpose |
|--------|---------|
| `types` | `PluginManifest`, `ManifestTool`, `ManifestCapability`, and core types |
| `parser` | Parse TOML/JSON manifests into validated structs |
| `validator` | Schema and semantic validation with error reporting |
| `hash` | Content hashing (`SHA-256`) for WASM binaries and manifests |
| `signature` | Ed25519 manifest signing and verification |
| `risk` | Risk assessment and `RiskLevel` classification |
| `classifier` | Tool risk classification by kind and capability |
| `lockfile` | Per-plugin and aggregate lockfile management |
| `store` | Installed plugin directory layout and lock entry helpers |
| `registry` | `ToolRegistry` for mapping manifest tools to engine definitions |
| `marketplace` | Marketplace catalog fetch, search, and staging |
| `approval` | Capability approval verification |
| `defaults` | Sensible defaults for manifests |

## Manifest format

```toml
[plugin]
name = "my-plugin"
version = "0.1.0"
api_version = 2
author = "Example"

[[tools]]
name = "deploy"
description = "Deploy to production"
kind = "command"
```

## Part of the NAVI workspace

**Full project:** <https://github.com/navi-ai-org/navi>
**License:** Apache-2.0
