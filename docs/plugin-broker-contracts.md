# NAVI Plugin Broker Contracts

## Principle

All sensitive access goes through host brokers. Plugins never access resources directly.
Brokers enforce authorization, validation, and resource limits.
Every broker call is auditable. Every denial is logged with a reason.

A broker MUST be the sole point of contact between a plugin and any external resource.
A plugin MUST NOT be able to bypass, disable, or replace a broker.

## FS Broker

The FS broker mediates all filesystem access for plugins.

### `read-project-file`

**Authorization Algorithm**:

1. Receive: `plugin_id`, `tool_id`, `capability_id`, `requested_path`.
2. Check: path MUST NOT contain null bytes (`\0`).
3. Check: path MUST NOT contain backslashes (non-Windows).
4. Check: path MUST NOT have leading or trailing whitespace.
5. Resolve the project root from the active session configuration.
6. If `requested_path` is relative, join it with the project root.
7. Canonicalize the final path using `realpath()` or equivalent (resolves symlinks and `..`).
8. Check: final resolved path MUST be under the project root.
9. Check: final resolved path MUST NOT match any denylist pattern.
10. Check: final resolved path MUST NOT contain `..` components after canonicalization.
11. Check: if allowed prefixes are configured, path must be within one of them.
12. Check: file size MUST NOT exceed the per-file limit (2 MB).
13. Check: total bytes read in this invocation MUST NOT exceed the invocation budget (16 MB).
14. Read file content as UTF-8.
15. Return content or error.
16. Log audit event.

**Denylist (always blocked)**:

The following paths and patterns MUST be rejected regardless of capability declarations:

| Pattern | Reason |
|---------|--------|
| `.git/` | Git metadata must not be read by plugins. |
| `.env`, `.env.*` | Environment files may contain secrets. |
| `*.pem`, `*.key`, `*.p12`, `*.pfx` | Private keys and certificates. |
| `.kube/config` | Kubernetes credentials. |
| `.npmrc`, `.pypirc`, `.netrc` | Package manager credentials. |
| `.ssh/`, `.aws/`, `.gpg/` | SSH, AWS, and GPG credentials. |
| `node_modules/`, `target/`, `.venv/`, `venv/`, `dist/`, `build/`, `.cache/` | Build artifacts and dependencies. Plugin should not depend on these. |
| `~/.config/navi/`, `~/.local/share/navi/` | NAVI private storage. |

The denylist is applied AFTER symlink resolution. A symlink pointing to a denylisted path MUST be rejected.

**Symlink Resolution Rules**:

1. The broker MUST use `realpath()` or equivalent to resolve all symlinks.
2. The broker MUST check the final resolved path, not the initial path.
3. The broker MUST reject if the resolved path escapes the project root.
4. The broker MUST reject symlink chains that eventually escape the project root.

**Path Traversal Prevention**:

1. The broker MUST reject `..` components after canonicalization.
2. The broker MUST reject null bytes (`\0`) anywhere in the path.
3. The broker MUST reject paths with leading or trailing whitespace.
4. The broker MUST reject paths with backslashes on non-Windows systems.

**Error Responses**:

| Error | Condition |
|-------|-----------|
| `"access denied"` | Path is outside project root, matches denylist, or capability does not cover this path. |
| `"not found"` | File does not exist at the resolved path. |
| `"too large"` | File exceeds 2 MB or invocation budget (16 MB) is exhausted. |
| `"outside project"` | Canonicalized path escapes the project root. |
| `"invalid utf-8"` | File content is not valid UTF-8. |

### `list-project-dir`

**Authorization Algorithm**:

Same as `read-project-file` steps 1-11, with the following additions:

1. The resolved path MUST be a directory.
2. The broker MUST return only entry names (not full paths).
3. The broker MUST sort entries for deterministic output.
4. The broker MUST filter out entries whose resolved paths match the denylist.

**Error Responses**:

| Error | Condition |
|-------|-----------|
| `"access denied"` | Path does not pass authorization checks. |
| `"not found"` | Directory does not exist at the resolved path. |
| `"outside project"` | Canonicalized path escapes the project root. |

## HTTP Broker

The HTTP broker mediates all network access for plugins.

### `request`

**Authorization Algorithm**:

1. Receive: `plugin_id`, `tool_id`, `capability_id`, `method`, `url`, `body`.
2. Parse the URL. If parsing fails, return `"invalid url"`.
3. Check: scheme MUST be `https` unless the capability explicitly allows `http` (capability `https_only = false`).
4. Check: host MUST be in the capability's `hosts` list (or `*` for wildcard).
5. Check: method MUST be in the capability's `methods` list.
6. Resolve DNS for the host.
7. Check: resolved IP MUST NOT be in any blocked range (see IP Validation Rules below).
8. Pin the DNS resolution: store the resolved IP for this host for this invocation.
9. If an auth binding is declared for this capability, retrieve the secret and inject it into the request.
10. Send the request with auto-redirect DISABLED.
11. For each redirect (maximum 3 hops):
    a. Parse the `Location` header.
    b. Resolve relative URLs against the current URL.
    c. Validate the scheme (same rules as step 3).
    d. Validate the host against the capability's `hosts` list (same rules as step 4).
    e. Validate the redirect URL against the capability.
    f. Continue the request to the new URL.
12. Cap the response body at 4 MB.
13. Sanitize response headers (see below).
14. Return the response (`status`, `headers-json`, `body`).
15. Log an audit event.

**DNS Rebinding Prevention**:

1. After the first DNS resolution for a host within an invocation, the broker MUST pin the resolved IP.
2. If a subsequent resolution returns a different IP for the same host, the broker MUST reject the request.
3. The pin cache is per-invocation only; it MUST NOT persist across invocations.

**IP Validation Rules**:

The following IP ranges MUST be rejected for all outbound requests:

| Range | Reason |
|-------|--------|
| `127.0.0.0/8` | IPv4 loopback. |
| `::1` | IPv6 loopback. |
| `10.0.0.0/8` | Private network (RFC 1918). |
| `172.16.0.0/12` | Private network (RFC 1918). |
| `192.168.0.0/16` | Private network (RFC 1918). |
| `169.254.0.0/16` | Link-local (RFC 3927). |
| `fe80::/10` | IPv6 link-local. |
| `169.254.169.254` | AWS/GCP/Azure metadata service. |
| `fd00:ec2::254` | AWS metadata (IPv6). |
| `0.0.0.0/8` | "This" network. |
| `100.64.0.0/10` | Carrier-grade NAT (RFC 6598). |
| `192.0.0.0/24` | IETF protocol assignments. |
| `192.0.2.0/24` | Documentation (TEST-NET-1). |
| `198.51.100.0/24` | Documentation (TEST-NET-2). |
| `203.0.113.0/24` | Documentation (TEST-NET-3). |
| `224.0.0.0/4` | Multicast. |
| `fc00::/7` | IPv6 unique local. |

**Rate Limiting**:

1. Maximum 10 HTTP requests per plugin per invocation minute.
2. Maximum 3 concurrent HTTP requests per plugin.
3. If the limit is exceeded, the broker MUST return `"rate limited"` and log an audit event.

**Response Header Sanitization**:

The broker MUST remove the following headers from the response before returning it to the plugin:

| Header | Reason |
|--------|--------|
| `Authorization` | May contain credentials echoed by the server. |
| `Cookie` | May contain session tokens. |
| `Set-Cookie` | May contain session tokens. |
| `Proxy-Authorization` | May contain proxy credentials. |
| `X-Api-Key` | May contain API keys. |
| Any header matching `*-Token` | Case-insensitive. May contain tokens. |
| Any header matching `*-Secret` | Case-insensitive. May contain secrets. |
| Any header matching `*-Key` | Case-insensitive. May contain keys. |

The broker MUST apply this sanitization before setting the `headers-json` field of the response.

**Error Responses**:

| Error | Condition |
|-------|-----------|
| `"host not allowed"` | Host is not in the capability's `hosts` list. |
| `"ip blocked"` | Resolved IP matches a forbidden range. |
| `"redirect denied"` | Redirect target fails validation. |
| `"timeout"` | Request exceeded the timeout (default 30 seconds). |
| `"too large"` | Response body exceeds the 4 MB cap. |
| `"rate limited"` | Plugin has exceeded its request quota. |
| `"invalid url"` | URL could not be parsed. |
| `"dns pin mismatch"` | DNS rebinding attempt detected. |

## Git Broker

The git broker mediates read-only git operations.

### `status`

**Authorization**:

1. The operation is read-only. No modifications to the repository are permitted.
2. The operation is project-scoped. The broker MUST execute `git` with the working directory set to the project root.
3. The broker MUST verify the project root is a git repository (`.git` directory exists).
4. The broker MUST enforce a wall-clock timeout of 5 seconds.
5. The broker MUST return the output of `git status --porcelain`.

**Error Responses**:

| Error | Condition |
|-------|-----------|
| `"not a git repository"` | Project root is not inside a git repository. |
| `"timeout"` | Command exceeded the 5-second timeout. |
| `"git error"` | Git exited with a non-zero status. |

### `diff`

**Authorization**:

1. The operation is read-only. No modifications to the repository are permitted.
2. The operation is project-scoped. The broker MUST execute `git` with the working directory set to the project root.
3. The broker MUST enforce a wall-clock timeout of 5 seconds.
4. The broker MUST cap output at 256 KB. If output exceeds this, the broker MUST return `"too large"`.
5. The broker MUST return the output of `git diff`.

**Error Responses**:

| Error | Condition |
|-------|-----------|
| `"not a git repository"` | Project root is not inside a git repository. |
| `"timeout"` | Command exceeded the 5-second timeout. |
| `"too large"` | Diff output exceeds 256 KB. |
| `"git error"` | Git exited with a non-zero status. |

### `log`

**Authorization**:

1. The operation is read-only.
2. The operation is project-scoped.
3. The broker MUST enforce a wall-clock timeout of 5 seconds.
4. The broker MUST return the output of `git log -N --oneline`.

**Error Responses**:

| Error | Condition |
|-------|-----------|
| `"not a git repository"` | Project root is not inside a git repository. |
| `"timeout"` | Command exceeded the 5-second timeout. |
| `"git error"` | Git exited with a non-zero status. |

### `branch`

**Authorization**:

1. The operation is read-only.
2. The operation is project-scoped.
3. The broker MUST return the output of `git branch --show-current`.

### `remote`

**Authorization**:

1. The operation is read-only.
2. The operation is project-scoped.
3. The broker MUST return the output of `git remote -v`.

## Auth Bindings

Auth bindings allow plugins to use secrets without ever seeing the secret values.

### Manifest Declaration

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

### How They Work

1. The manifest declares an auth binding on a network capability.
2. The binding specifies: the secret name in the credential store (`binding`), and the injection format (`inject_as`).
3. When the HTTP broker sends a request matching the capability:
   a. The host retrieves the secret from the credential store.
   b. The host injects the secret into the request (e.g., as an `Authorization` header).
   c. The plugin NEVER sees the secret value.
4. Auth binding is scoped to: `plugin_id` + `capability_id` + `host` + `method`.

### Implementation Status

Auth bindings are declared in the manifest schema (`AuthBinding` struct with `binding` and `inject_as` fields). The broker contract is defined above. Actual credential store integration is in progress.

### Scoping Rules

1. An auth binding MUST be scoped to the specific `plugin_id`, `capability_id`, `host`, and `method`.
2. An auth binding MUST NOT be shared across plugins.
3. An auth binding MUST NOT apply to hosts not listed in the capability's `hosts` list.

### Injection Rules

1. The host MUST inject the secret using the format specified in `inject_as`.
2. The `inject_as` format uses `{secret}` as a placeholder for the secret value.
3. Example: `"Authorization: Bearer {secret}"` becomes `"Authorization: Bearer sk-abc123..."`.
4. The host MUST inject the secret AFTER all URL validation and DNS checks pass.
5. The host MUST NOT include the secret value in any log output.

### Error Handling

1. If the secret cannot be retrieved from the credential store, the broker MUST return `"auth injection failed"`.
2. The broker MUST NOT send the request without the auth header if auth injection was expected.

## Audit Logging

Every broker call MUST produce an audit log entry. Audit logs are the primary mechanism for detecting misuse.

### Implementation

Audit logs are stored in-memory as `Vec<AuditEntry>` per broker instance. Each entry contains:

```rust
pub struct AuditEntry {
    pub plugin_id: String,
    pub tool_id: String,
    pub capability_id: String,
    pub operation: String,
    pub target: String,
    pub result: AuditResult,  // Allow or Deny
    pub reason: Option<String>,
}
```

### Required Fields

Every audit entry MUST include:

| Field | Type | Description |
|-------|------|-------------|
| `plugin_id` | string | The plugin making the call. |
| `tool_id` | string | The tool making the call. |
| `capability_id` | string | The capability being exercised. |
| `operation` | string | Operation name (e.g., `read`, `list`, `request`, `status`). |
| `target` | string | Path or URL being accessed. |
| `result` | string | `allow` or `deny`. |
| `reason` | string | If denied, the reason for denial. Empty if allowed. |

### Example Audit Entries

```json
{
  "plugin_id": "code-search",
  "tool_id": "semantic-search",
  "capability_id": "read-src",
  "operation": "read",
  "target": "src/main.rs",
  "result": "allow",
  "reason": null
}
```

```json
{
  "plugin_id": "code-search",
  "tool_id": "semantic-search",
  "capability_id": "read-src",
  "operation": "read",
  "target": ".env",
  "result": "deny",
  "reason": "path matches sensitive pattern: .env"
}
```

### Audit Configuration

Audit logging is controlled by `AuditDefaults`:

```rust
pub struct AuditDefaults {
    pub enabled: bool,              // default: true
    pub log_level_normal: String,   // default: "debug"
    pub log_level_high_risk: String, // default: "info"
}
```

## Resource Limits Summary

| Resource | Limit | Scope |
|----------|-------|-------|
| File size | 2 MB | Per file |
| Bytes read (FS) | 16 MB | Per invocation |
| HTTP response body | 4 MB | Per request |
| HTTP requests | 10/min | Per plugin |
| HTTP concurrent | 3 | Per plugin |
| HTTP redirects | 3 | Per request |
| Git diff output | 256 KB | Per call |
| Git command timeout | 5 seconds | Per call |
| WASM fuel | 10,000,000 | Per tool invocation |
| WASM memory | 64 MB | Per plugin instance |
| WASM timeout | 30 seconds | Per tool invocation |
| WASM stack | 1 MB | Per plugin instance |
| WASM output | 32 KB | Per tool invocation |
| Schema description | 200 chars | Per field |

All limits MUST be enforced by the host. Plugins MUST NOT be able to override or negotiate these limits.
