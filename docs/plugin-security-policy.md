# NAVI Plugin Security Policy

## Principles

1. **Default-deny**: Nothing works until declared and approved. A capability
   that is not explicitly granted MUST be denied.
2. **Least privilege**: Capabilities are minimal per tool. A tool MUST NOT
   receive broader access than required for its stated purpose.
3. **Defense in depth**: Multiple layers of enforcement. A failure in one
   layer MUST NOT expose the system.
4. **No ambient authority**: A plugin gets nothing for free. Every capability
   MUST be explicitly requested, classified, and approved.
5. **Auditability**: Every broker call MUST be logged with sufficient detail
   for post-hoc review.

## Plugin Trust Levels

### Core

| Property | Value |
|---|---|
| Runtime | Native in-process |
| Publisher | NAVI team |
| Capabilities | Unrestricted |
| Use case | Built-in tools, core features |

Core plugins are part of the NAVI distribution. They run as native code inside
the host process and are subject to the same safety policies as built-in tools.

### Signed

| Property | Value |
|---|---|
| Runtime | WASM Component or subprocess |
| Publisher | Verified identity with signing key |
| Capabilities | Declared and approved |
| Use case | Official partners, verified community |

Signed plugins MUST carry a valid signature from a recognized publisher. The
host MUST verify the signature before loading. Capabilities MUST be declared in
the plugin manifest and approved by the user at install time.

### Community

| Property | Value |
|---|---|
| Runtime | WASM Component ONLY |
| Publisher | Anyone |
| Capabilities | Restricted to safe set |
| Use case | Community plugins |

Community plugins MUST run inside a sandboxed WASM runtime. They MUST NOT
request capabilities outside the community allowlist. The host MUST reject
any community plugin that declares forbidden capabilities.

### Local/Dev

| Property | Value |
|---|---|
| Runtime | Native in-process (UNSAFE flag required) |
| Publisher | Local developer |
| Capabilities | Unrestricted |
| Use case | Plugin development only |

Local/Dev plugins load native libraries for development convenience. They
MUST NOT be loaded unless the `--unsafe-plugins` flag is set. The host MUST
display a confirmation prompt before loading.

## Capability Classification

### LOW

- `filesystem: project/read-only` — Read files within the project root.
- `tui: render passive widget` — Display non-interactive content in the TUI.
- `git: status, diff (read-only)` — Query git state without modification.

### MEDIUM

- `filesystem: project/read-write` — Modify files within the project root.
  (Post-MVP.)
- `network: specific hosts, GET only` — Make HTTP GET requests to declared
  hosts.
- `tui: interactive widget` — Receive user input through TUI elements.

### HIGH

- `network: specific hosts, POST` — Make HTTP POST requests to declared hosts.
- `filesystem: read + network` — Read files and access the network. Compound
  risk: exfiltration.
- `env: specific vars (via auth binding)` — Access specific environment
  variables through the auth binding mechanism.

### CRITICAL

- `filesystem: read + network POST` — Read files and send data to external
  servers.
- `filesystem: read + auth binding` — Read files and authenticate to external
  services.
- `filesystem: write outside project` — Modify files outside the project root.
- `network: wildcard` — Make requests to any host.
- `shell: execution` — Execute shell commands.
- `process: spawn` — Spawn child processes.

### FORBIDDEN (Community Plugins)

Community plugins MUST NOT request any of the following:

- Shell execution
- Process spawn
- `env.get` (raw access)
- Model context injection
- Agent policy mutation
- Native in-process execution

The host MUST reject any community plugin that declares a forbidden capability
during manifest validation, before any code is loaded.

## Compound Risk Rules

| Combination | Risk |
|---|---|
| `fs_project_read` | MEDIUM |
| `network_GET_fixed` | MEDIUM |
| `network_POST_fixed` | HIGH |
| `fs_read` + `network_GET` | HIGH |
| `fs_read` + `network_POST` | CRITICAL |
| `fs_read` + `auth_binding` | HIGH |
| `fs_read` + `auth_binding` + `POST` | CRITICAL |
| `write_project` | HIGH |
| `write_project` + `network` | CRITICAL |
| `process_exec` | FORBIDDEN |
| `shell_string` | FORBIDDEN |

Compound risk MUST be computed per tool, not per plugin. A plugin with two
separate tools that each have MEDIUM risk is safer than a single tool that
combines both capabilities.

## Approval Model

### Install Time

At install time the host MUST display:

- Plugin name, version, and publisher.
- Full list of capabilities with their risk levels.
- Compound risk warnings for HIGH and CRITICAL combinations.

The user MUST explicitly approve each CRITICAL capability. The host MUST NOT
auto-approve CRITICAL capabilities under any configuration.

### Runtime (Optional Permissions)

A plugin MAY request additional capabilities at runtime if the manifest
declares them as optional. When this occurs:

- The host MUST show an approval prompt.
- The user MAY grant once, grant for the session, or deny.
- The host MUST NOT auto-grant optional capabilities in headless mode.

### Update Time

When a plugin update is available:

- If capabilities have changed, the host MUST require reconsent.
- If the overall risk level has increased, the host MUST require reconsent.
- If the publisher or signing key has changed, the host MUST block the update
  by default and require explicit user action.

## Reconsent Rules

| Change | Action |
|---|---|
| Code changes, capabilities same | Allow with hash update |
| Capability added | Block until reconsent |
| Capability removed | Allow (reduces privilege) |
| Publisher changes | Block by default |
| Signing key changes | Block by default |
| Tool risk increases | Block until reconsent |
| Tool schema changes | Show diff, allow |
| `minimum_navi` increases | Warn |

When reconsent is required the host MUST:

1. Display a diff of the old and new manifests.
2. Highlight capabilities that have been added, removed, or changed.
3. Block execution until the user explicitly approves.

## Unsafe Mode

Unsafe mode exists for local plugin development only.

Requirements:

- The host MUST require the explicit flag `--unsafe-plugins`.
- The host MUST display a confirmation prompt listing the risks.
- The host MUST load native `.so`/`.dylib` libraries in-process.
- No capability restrictions are enforced.
- No broker mediation is applied.

Restrictions:

- Unsafe mode MUST NOT be used in production.
- Unsafe mode MUST NOT be enabled by default.
- The host MUST log a warning on every invocation when unsafe mode is active.
- The host MUST NOT allow unsafe mode when the `NAVI_ENV` variable is set to
  `production`.

## Audit Trail

Every broker call MUST produce an audit log entry containing:

- Timestamp (ISO 8601)
- Plugin ID
- Tool ID
- Capability ID
- Operation performed
- Target (file path, URL, etc.)
- Result (allow / deny)
- Reason (if denied)

Audit logs MUST be stored in a location accessible to the user. Audit logs
MUST NOT be deleted by plugins.
