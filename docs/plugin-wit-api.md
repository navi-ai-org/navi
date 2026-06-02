# NAVI Plugin WIT API Specification

## Overview

The WIT (WebAssembly Interface Types) interface defines the contract between the NAVI host and WASM plugin components.
Plugins can only access host resources through these imports.
The host only calls the `run-tool` export.
Plugins MUST NOT access any host resource not explicitly imported through this interface.

## WIT Definition (MVP)

```wit
package navi:plugin;

interface fs {
    /// Read a file within the project directory.
    ///
    /// The path MUST be relative to the project root.
    /// The host resolves the path, enforces access controls, and returns the content.
    read-project-file: func(path: string) -> result<string, string>;

    /// List entries in a project directory.
    ///
    /// The path MUST be relative to the project root.
    /// Returns a list of file and directory names (not full paths).
    list-project-dir: func(path: string) -> result<list<string>, string>;
}

interface http {
    /// An HTTP response returned by the host broker.
    record response {
        /// HTTP status code (e.g., 200, 404).
        status: u16,
        /// Response headers serialized as a JSON object string.
        /// Sensitive headers are stripped before this value is set.
        headers-json: string,
        /// Response body as a UTF-8 string.
        body: string,
    }

    /// Send an HTTP request through the host broker.
    ///
    /// The host validates the URL, resolves DNS, checks IP restrictions,
    /// enforces rate limits, and sanitizes the response.
    request: func(method: string, url: string, body: option<string>) -> result<response, string>;
}

interface git {
    /// Get the git status of the project.
    ///
    /// Returns output equivalent to `git status --porcelain`.
    status: func() -> result<string, string>;

    /// Get the git diff of the project.
    ///
    /// Returns output equivalent to `git diff`.
    diff: func() -> result<string, string>;
}

interface tool {
    /// Input to a tool invocation, serialized as JSON.
    record input {
        json: string,
    }

    /// Output from a tool invocation, serialized as JSON.
    record output {
        json: string,
    }
}

world plugin {
    import fs;
    import http;
    import git;
    export run-tool: func(name: string, input: tool.input) -> result<tool.output, string>;
}
```

## Import Contracts

All imports are brokered by the host. Plugins MUST NOT bypass these brokers.

### `fs.read-project-file`

**Purpose**: Read a UTF-8 file within the project directory.

**Input**:
- `path` (string): Relative path within the project root.

**Output**:
- Success: File content as a UTF-8 string.
- Error: Human-readable error string.

**Broker Obligations**:
1. The host MUST canonicalize the requested path.
2. The host MUST resolve all symlinks using `realpath()` or equivalent.
3. The host MUST verify the final resolved path is under the project root.
4. The host MUST reject paths matching the denylist (see Broker Contracts).
5. The host MUST reject files exceeding the per-file size cap (2 MB).
6. The host MUST track cumulative bytes read per invocation and reject if the budget (16 MB) is exceeded.
7. The host MUST read the file as UTF-8 and reject non-UTF-8 content.

**Error Strings**:
- `"access denied"`: Path is outside project root, matches denylist, or capability does not permit this path.
- `"not found"`: File does not exist at the resolved path.
- `"too large"`: File exceeds the per-file size cap or the invocation budget is exhausted.
- `"outside project"`: Canonicalized path escapes the project root.
- `"invalid utf-8"`: File content is not valid UTF-8.

### `fs.list-project-dir`

**Purpose**: List entries in a project directory.

**Input**:
- `path` (string): Relative path within the project root.

**Output**:
- Success: List of file and directory names (not full paths).
- Error: Human-readable error string.

**Broker Obligations**:
1. The host MUST apply the same authorization checks as `read-project-file`.
2. The host MUST return only entry names, not full paths.
3. The host MUST NOT follow symlinks that escape the project root.
4. The host MUST sort the returned list for deterministic output.

**Error Strings**:
- `"access denied"`: Path does not pass authorization checks.
- `"not found"`: Directory does not exist at the resolved path.
- `"outside project"`: Canonicalized path escapes the project root.

### `http.request`

**Purpose**: Send an HTTP request to an allowed host.

**Input**:
- `method` (string): HTTP method (e.g., `"GET"`, `"POST"`).
- `url` (string): Full URL including scheme and host.
- `body` (option<string>): Optional request body.

**Output**:
- Success: `response` record with `status`, `headers-json`, and `body`.
- Error: Human-readable error string.

**Broker Obligations**:
1. The host MUST parse and validate the URL.
2. The host MUST verify the scheme matches the capability (`https_only` check).
3. The host MUST verify the host is in the capability's `hosts` list.
4. The host MUST verify the method is in the capability's `methods` list.
5. The host MUST resolve DNS and validate resolved IPs (see Broker Contracts).
6. The host MUST pin DNS for the invocation to prevent rebinding.
7. The host MUST send the request with auto-redirect disabled.
8. The host MUST manually follow redirects (max 3), validating each hop.
9. The host MUST cap the response body at 4 MB.
10. The host MUST sanitize response headers before returning to the plugin.
11. The host MUST inject auth bindings if declared for this capability.
12. The host MUST enforce rate limits (10 requests/min, 3 concurrent per plugin).
13. The host MUST log an audit event for every request.

**Error Strings**:
- `"host not allowed"`: Host is not in the capability's `hosts` list.
- `"ip blocked"`: Resolved IP is loopback, private, link-local, or metadata.
- `"redirect denied"`: Redirect target fails validation.
- `"timeout"`: Request exceeded the timeout.
- `"too large"`: Response body exceeds the 4 MB cap.
- `"rate limited"`: Plugin has exceeded its request quota.
- `"invalid url"`: URL could not be parsed.

### `git.status`

**Purpose**: Get the git working tree status.

**Input**: None.

**Output**:
- Success: String equivalent to `git status --porcelain`.
- Error: Human-readable error string.

**Broker Obligations**:
1. The host MUST execute `git status --porcelain` scoped to the project root.
2. The host MUST enforce a timeout (5 seconds).
3. The host MUST NOT allow this call to modify the repository.

**Error Strings**:
- `"not a git repository"`: Project root is not inside a git repository.
- `"timeout"`: Command exceeded the timeout.

### `git.diff`

**Purpose**: Get the git diff of the working tree.

**Input**: None.

**Output**:
- Success: String equivalent to `git diff`.
- Error: Human-readable error string.

**Broker Obligations**:
1. The host MUST execute `git diff` scoped to the project root.
2. The host MUST enforce a timeout (5 seconds).
3. The host MUST cap output at 256 KB.
4. The host MUST NOT allow this call to modify the repository.

**Error Strings**:
- `"not a git repository"`: Project root is not inside a git repository.
- `"timeout"`: Command exceeded the timeout.
- `"too large"`: Diff output exceeds 256 KB.

## Export Contract

### `run-tool`

**Purpose**: Execute a registered tool and return its result.

**Signature**:
```wit
export run-tool: func(name: string, input: tool.input) -> result<tool.output, string>;
```

**Input**:
- `name` (string): The tool ID. MUST match a tool ID declared in the plugin's manifest.
- `input` (tool.input): JSON-encoded arguments conforming to the tool's `input_schema`.

**Output**:
- Success: `tool.output` containing JSON-encoded result.
- Error: Human-readable error string.

**Host Obligations**:
1. The host MUST verify `name` matches a tool ID registered for this plugin.
2. The host MUST validate `input.json` against the tool's declared `input_schema`.
3. The host MUST enforce a fuel limit for the WASM execution.
4. The host MUST enforce a memory limit for the WASM instance.
5. The host MUST enforce a wall-clock timeout for the execution.
6. The host MUST enforce capability checks: the tool can only call imports allowed by its declared capabilities.
7. The host MUST log an audit event for every tool invocation.

**Error Strings**:
- `"tool not found"`: `name` does not match any registered tool ID for this plugin.
- `"invalid input"`: `input.json` does not validate against the tool's `input_schema`.
- `"timeout"`: Execution exceeded the wall-clock timeout.
- `"resource limit exceeded"`: Fuel or memory limit was exceeded.
- `"capability denied"`: Tool attempted an import not permitted by its capabilities.

## What is NOT in MVP

The following interfaces and features are explicitly excluded from the MVP.
Plugins MUST NOT attempt to use them; the host MUST reject any plugin that declares or calls them.

| Feature | Reason for Exclusion |
|---------|----------------------|
| `env.get` | Secrets are handled by auth bindings in the manifest. Direct env access would leak secrets. |
| `write-project-file` | Write access requires an approval flow that is not yet defined for WASM plugins. |
| `shell` / process execution | Arbitrary command execution is too dangerous for community plugins. |
| `tui.render` | Declarative UI requires a component model that is not yet specified. |
| `metadata()` export | Plugin metadata is fully declared in the manifest; a runtime export is redundant. |
| `model.context.inject` | Allowing plugins to inject context into the model conversation requires careful scoping. |
| `secrets.get` | Direct secret access is not permitted; use auth bindings instead. |
| `fs.list-project-dir` recursive | Deep listing risks excessive memory usage; plugins should iterate. |
| `http.request` with streaming body | Streaming request bodies add complexity; use full body for MVP. |

## Future Interfaces (Post-MVP)

The following interfaces are planned for future versions and MAY appear in subsequent WIT definitions.

### `fs.write-project-file`

Write a file to the project directory. Will require:
- An approval flow through the host (user must confirm).
- `access = "read-write"` capability in the manifest.
- Write-path validation (no `.git`, no secrets, no dotfiles).
- Atomic write (write to temp, then rename).

### `tui.get-view-state`

Return a declarative UI description for rendering in the TUI. Will provide:
- A component tree (panels, text, lists, tables).
- Event hooks for user interaction.
- Scoped to the `tui` capability.

### `secrets.get-handle`

Retrieve a one-time handle for a secret, usable in specific contexts where auth bindings are insufficient. Will require:
- Explicit user approval per handle.
- Scoped to specific host operations.
- Handle expiration.

### `git.commit` (with approval)

Create a git commit. Will require:
- An approval flow through the host.
- Commit message validation.
- Pre-commit hook execution.

### `http.request-stream`

Stream large request/response bodies. Will provide:
- Chunked transfer for large payloads.
- Backpressure signaling.
- Size caps still enforced.

## Versioning

The WIT interface is versioned alongside NAVI.

- The MVP corresponds to `navi:plugin` version `0.1.0`.
- Breaking changes to the WIT interface require a new minor or major version.
- The `plugin.minimum_navi` field in the manifest determines which WIT version the plugin expects.
- The host MUST reject plugins that require a WIT version newer than what it supports.

## Security Invariants

1. Plugins MUST NOT be able to access any host resource not explicitly imported.
2. All imports MUST be brokered by the host with validation and authorization.
3. Plugin execution MUST be bounded by fuel, memory, and time limits.
4. Plugin errors MUST be propagated as `result::err` strings, never as panics that crash the host.
5. The host MUST isolate plugins from each other; one plugin MUST NOT access another plugin's state.
6. The host MUST sanitize all data returned from imports before passing it to the plugin.
