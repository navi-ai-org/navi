# NAVI Plugin Manifest Specification

## Overview

The manifest (`plugin.toml`) declares plugin identity, capabilities, tools, and metadata.
The manifest is the source of truth for what a plugin can do.
The manifest MUST be signed and hashed.
The host MUST reject any plugin that lacks a valid manifest.

## Schema

All fields are REQUIRED unless explicitly marked optional.

### `[plugin]` Section

| Field           | Type   | Description                                            |
|-----------------|--------|--------------------------------------------------------|
| `id`            | string | Stable identifier. MUST match `[a-z0-9][a-z0-9-_]{1,63}`. |
| `name`          | string | Human-readable display name.                           |
| `version`       | string | Semantic version (`MAJOR.MINOR.PATCH`).                 |
| `publisher`     | string | Publisher identity (e.g., `navi:official`, `gh:username`). |
| `runtime`       | string | MUST be `"wasm-component"` for community plugins.       |
| `entry`         | string | Path to `.wasm` file relative to the manifest directory. |
| `wasm_hash`     | string | SHA-256 digest of the `.wasm` file, prefixed `sha256:`.  |
| `signature`     | string | Ed25519 signature of the canonical hash bundle, prefixed `ed25519:`. |
| `minimum_navi`  | string | Minimum NAVI version required to run this plugin.        |

```toml
[plugin]
id = "example-plugin"
name = "Example Plugin"
version = "1.0.0"
publisher = "gh:example-author"
runtime = "wasm-component"
entry = "plugin.wasm"
wasm_hash = "sha256:abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
signature = "ed25519:base64encodedsignaturehere"
minimum_navi = "0.1.0"
```

### `[[capabilities]]` Section

Each capability declares a resource the plugin is permitted to access.
Capabilities MUST be listed if any tool requires them.

| Field    | Type   | Description                                          |
|----------|--------|------------------------------------------------------|
| `id`     | string | Unique within this plugin. Matches `[a-z0-9-_]+`.    |
| `kind`   | string | One of: `filesystem`, `network`, `tui`.               |
| `reason` | string | Human-readable justification for why this capability is needed. |

#### `kind = "filesystem"`

| Field    | Type     | Description                                                |
|----------|----------|------------------------------------------------------------|
| `scope`  | string   | `"project"` (only project root) or `"workspace"` (includes linked repos). |
| `access` | string   | `"read-only"` or `"read-write"`. Community plugins MUST use `"read-only"`. |
| `paths`  | string[] | Allowed path prefixes (relative to scope root).            |

```toml
[[capabilities]]
id = "read-src"
kind = "filesystem"
scope = "project"
access = "read-only"
paths = ["src/", "lib/", "README.md"]
reason = "Needs to read source files to provide code analysis."
```

#### `kind = "network"`

| Field      | Type     | Description                                                      |
|------------|----------|------------------------------------------------------------------|
| `hosts`    | string[] | Allowed hostnames or IP literals. MUST NOT be empty.             |
| `methods`  | string[] | Allowed HTTP methods (e.g., `["GET", "POST"]`).                  |
| `https_only` | bool   | If `true` (default), plain HTTP is rejected.                     |
| `auth`     | table    | Optional auth binding (see below).                               |

```toml
[[capabilities]]
id = "call-api"
kind = "network"
hosts = ["api.example.com"]
methods = ["GET", "POST"]
https_only = true
reason = "Needs to call Example API for data retrieval."
```

#### `kind = "tui"`

| Field      | Type     | Description                                     |
|------------|----------|-------------------------------------------------|
| `components` | string[] | TUI component types the plugin may render.    |

Community plugins MUST NOT declare `tui` capabilities in the MVP.

#### `[capabilities.*.auth]` (optional)

| Field      | Type   | Description                                                    |
|------------|--------|----------------------------------------------------------------|
| `binding`  | string | Name of the secret in the credential store to inject.          |
| `inject_as` | string | Header format, e.g., `"Authorization: Bearer {secret}"`.      |

The plugin NEVER sees the raw secret value. The host injects it at request time.

```toml
[[capabilities]]
id = "call-api"
kind = "network"
hosts = ["api.example.com"]
methods = ["GET"]
https_only = true
reason = "Needs Example API access."

[capabilities.call-api.auth]
binding = "EXAMPLE_API_KEY"
inject_as = "Authorization: Bearer {secret}"
```

### `[[tools]]` Section

Each tool the plugin exposes MUST be declared here.

| Field         | Type            | Description                                              |
|---------------|-----------------|----------------------------------------------------------|
| `id`          | string          | Unique within this plugin. Matches `[a-z0-9-_]+`.         |
| `summary`     | string          | Brief description. The host generates the full tool description. |
| `risk`        | string          | One of: `read_only`, `network_read`, `network_write`, `write`. |
| `input_schema` | string or table | JSON Schema for the tool's arguments. Inline table or path to `.json` file. |
| `capabilities` | string[]       | Capability IDs this tool requires. MUST reference existing capabilities. |

```toml
[[tools]]
id = "search-docs"
summary = "Search documentation files for a query string."
risk = "read_only"
input_schema = '''
{
  "type": "object",
  "properties": {
    "query": { "type": "string", "description": "Search query" }
  },
  "required": ["query"]
}
'''
capabilities = ["read-src"]
```

## Validation Rules

The host MUST enforce these rules when loading a plugin manifest:

1. `plugin.id` MUST be stable across versions. A plugin MUST NOT change its `id` after initial publication.
2. `plugin.id` MUST match the pattern `[a-z0-9][a-z0-9-_]{1,63}`.
3. `plugin.runtime` MUST be `"wasm-component"` for community plugins.
4. `plugin.entry` MUST point to an existing `.wasm` file relative to the manifest directory.
5. `plugin.wasm_hash` MUST match the actual SHA-256 digest of the `.wasm` file at load time.
6. `plugin.signature` MUST be a valid Ed25519 signature over the canonical hash bundle for the declared publisher.
7. `plugin.minimum_navi` MUST be satisfied by the running NAVI version.
8. `tools[].id` MUST be unique within the plugin.
9. `capabilities[].id` MUST be unique within the plugin.
10. `tools[].capabilities` MUST reference capability IDs that exist in the same manifest.
11. `tools[].risk` MUST accurately reflect the most permissive operation the tool performs.
12. Community plugins MUST NOT declare `kind = "tui"` capabilities.
13. Community plugins MUST NOT declare `filesystem` capabilities with `access = "read-write"`.
14. Network `capabilities[].hosts` MUST NOT be empty when `kind = "network"`.
15. Network `capabilities[].methods` MUST NOT be empty when `kind = "network"`.
16. If `tools[].input_schema` is a string, it MUST be valid JSON Schema.

The host MUST reject the entire plugin if ANY validation rule fails.

## Hash and Signature

### Hash Computation

Three independent hashes are computed:

1. **`wasm_hash`**: SHA-256 of the raw bytes of the `.wasm` file referenced by `plugin.entry`.
2. **`capabilities_hash`**: SHA-256 of the normalized (canonical) TOML serialization of the `[[capabilities]]` array.
3. **`tools_hash`**: SHA-256 of the normalized (canonical) TOML serialization of the `[[tools]]` array.

Normalization means:
- Keys sorted alphabetically within each table.
- No trailing whitespace.
- LF line endings.
- UTF-8 encoding without BOM.

### Signature Computation

The signature covers the concatenation:

```
wasm_hash_bytes ++ capabilities_hash_bytes ++ tools_hash_bytes
```

The publisher signs this concatenation with their Ed25519 private key.
The host verifies the signature against the publisher's public key.

### Verification Algorithm

1. Compute `wasm_hash` from the `.wasm` file on disk.
2. Parse the manifest and compute `capabilities_hash` and `tools_hash` from the declared sections.
3. Concatenate the three hash digests.
4. Retrieve the publisher's Ed25519 public key from the key registry.
5. Verify the Ed25519 signature over the concatenated hashes.
6. If verification fails, the host MUST reject the plugin with an error.

## Examples

### Valid Minimal Manifest

```toml
[plugin]
id = "hello-world"
name = "Hello World"
version = "1.0.0"
publisher = "gh:example"
runtime = "wasm-component"
entry = "plugin.wasm"
wasm_hash = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
signature = "ed25519:base64signature"
minimum_navi = "0.1.0"

[[tools]]
id = "greet"
summary = "Returns a greeting message."
risk = "read_only"
input_schema = '''
{
  "type": "object",
  "properties": {
    "name": { "type": "string" }
  },
  "required": ["name"]
}
'''
capabilities = []
```

### Valid Manifest with Network + Filesystem

```toml
[plugin]
id = "code-search"
name = "Code Search"
version = "2.1.0"
publisher = "gh:example-author"
runtime = "wasm-component"
entry = "code_search.wasm"
wasm_hash = "sha256:abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
signature = "ed25519:base64encodedsignature"
minimum_navi = "0.1.0"

[[capabilities]]
id = "read-src"
kind = "filesystem"
scope = "project"
access = "read-only"
paths = ["src/", "lib/"]
reason = "Reads source files to search for code patterns."

[[capabilities]]
id = "call-search-api"
kind = "network"
hosts = ["search.example.com"]
methods = ["POST"]
https_only = true
reason = "Calls external search API for semantic code search."

[capabilities.call-search-api.auth]
binding = "SEARCH_API_KEY"
inject_as = "Authorization: Bearer {secret}"

[[tools]]
id = "semantic-search"
summary = "Search codebase using natural language queries."
risk = "network_read"
input_schema = '''
{
  "type": "object",
  "properties": {
    "query": { "type": "string", "description": "Natural language search query" }
  },
  "required": ["query"]
}
'''
capabilities = ["read-src", "call-search-api"]
```

### Invalid Manifest: Missing Required Fields

```toml
[plugin]
id = "broken"
name = "Broken Plugin"
# Missing: version, publisher, runtime, entry, wasm_hash, signature, minimum_navi

[[tools]]
id = "do-thing"
summary = "Does a thing."
risk = "read_only"
input_schema = '{}'
capabilities = []
```

This manifest MUST be rejected. The host MUST report all missing fields.

### Invalid Manifest: Forbidden Capability

```toml
[plugin]
id = "sneaky"
name = "Sneaky Plugin"
version = "1.0.0"
publisher = "gh:bad-actor"
runtime = "wasm-component"
entry = "plugin.wasm"
wasm_hash = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
signature = "ed25519:base64signature"
minimum_navi = "0.1.0"

[[capabilities]]
id = "write-anywhere"
kind = "filesystem"
scope = "project"
access = "read-write"   # FORBIDDEN for community plugins
paths = ["/"]
reason = "Needs to write files."

[[tools]]
id = "overwrite"
summary = "Overwrites project files."
risk = "write"
input_schema = '''
{
  "type": "object",
  "properties": {
    "path": { "type": "string" },
    "content": { "type": "string" }
  },
  "required": ["path", "content"]
}
'''
capabilities = ["write-anywhere"]
```

This manifest MUST be rejected. Community plugins MUST NOT declare `read-write` filesystem access.

## Security Considerations

- The manifest MUST be immutable once published. A new version MUST be a new manifest with an incremented `version`.
- The `wasm_hash` MUST be re-verified every time the plugin is loaded, not just at install time.
- The host MUST NOT trust a manifest whose signature cannot be verified against a known publisher public key.
- The host MUST NOT allow a plugin to request capabilities not declared in its manifest at runtime.
- The host MUST reject manifests with `plugin.id` values that collide with built-in tool names.
