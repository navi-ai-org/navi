# NAVI Plugin System Architecture

**Status:** Draft
**Version:** 0.1.0
**Date:** 2026-06-01

---

## 1. System Diagram

```txt
+------------------------------------------------------------------+
|                        NAVI Host Process                         |
|                                                                  |
|  +-------------------+    +-------------------+                  |
|  |   Agent Core      |    |   TuiApp          |                  |
|  |  (not extensible)  |    |  (renders views)  |                  |
|  +--------+----------+    +--------+----------+                  |
|           |                        |                             |
|           v                        v                             |
|  +--------+------------------------+----------+                  |
|  |              ToolExecutor                   |                  |
|  |  (built-in tools + plugin tools)            |                  |
|  +--------+-------------------+---+-----------+                  |
|           |                   |                                  |
|  +--------v----------+  +-----v------------+                    |
|  |   Tool Registry   |  | Security Policy  |                    |
|  |  (namespacing,    |  | (risk, approval) |                    |
|  |   descriptions)   |  +-----+------------+                    |
|  +--------+----------+        |                                  |
|           |                    |                                  |
|  +--------v--------------------v----------+                      |
|  |          Capability Manager             |                      |
|  |  (manifest, risk, consent)              |                      |
|  +--------+-------------------------------+                      |
|           |                                                      |
|  +--------v-------------------------------+                      |
|  |          Plugin Registry                |                      |
|  |  (install, hash, signature, lifecycle)  |                      |
|  +--------+-------------------------------+                      |
|           |                                                      |
|  +--------v-------------------------------+                      |
|  |          Wasmtime Runtime               |                      |
|  |  (WASM Component execution)             |                      |
|  |  (fuel, memory, timeout)                |                      |
|  +--------+-------------------------------+                      |
|           |                                                      |
|  +--------v-------------------------------+                      |
|  |          Host Brokers                   |                      |
|  |  +----------+ +-------+ +----------+   |                      |
|  |  | FS Broker| | HTTP  | | Git Broker|   |                      |
|  |  | (read)   | | Broker| | (read)   |   |                      |
|  |  +----------+ +-------+ +----------+   |                      |
|  |  +----------+                          |                      |
|  |  | Auth     |                          |                      |
|  |  | Bindings |                          |                      |
|  |  +----------+                          |                      |
|  +----------------------------------------+                      |
+------------------------------------------------------------------+
```

---

## 2. Component Responsibilities

### 2.1 navi-plugin-manifest

**Crate:** `navi-plugin-manifest`
**Role:** Manifest parsing, validation, and normalization.

Responsibilities:

- Parse `plugin.toml` into typed Rust structs.
- Validate all required fields are present and well-formed.
- Validate `plugin.id` matches `[a-z0-9][a-z0-9-_]{1,63}`.
- Validate `plugin.runtime` is a known runtime type.
- Validate `tools[].capabilities` reference existing capability IDs.
- Validate `capabilities[].severity` is a known level.
- Normalize the manifest into a canonical form.
- Compute `capabilities_hash` (SHA-256 of normalized capabilities).
- Compute `tools_hash` (SHA-256 of normalized tool definitions).
- Reject manifests with duplicate tool IDs or capability IDs.

This crate MUST NOT execute plugins. It is a pure data processing crate.

**Key Types:**

```rust
pub struct PluginManifest {
    pub plugin: PluginMeta,
    pub capabilities: Vec<Capability>,
    pub tools: Vec<ToolDefinition>,
}

pub struct PluginMeta {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub runtime: RuntimeKind,
    pub description: Option<String>,
    pub license: Option<String>,
}

pub enum RuntimeKind {
    WasmComponent,
    Subprocess,
    Native,
}

pub struct Capability {
    pub id: String,
    pub kind: CapabilityKind,
    pub severity: Severity,
    pub description: String,
    pub scope: CapabilityScope,
}

pub enum CapabilityKind {
    FsRead,
    FsWrite,
    Http,
    Git,
    Auth,
}

pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

pub struct ToolDefinition {
    pub id: String,
    pub description: String,
    pub capabilities: Vec<String>,
    pub input_schema: serde_json::Value,
}
```

---

### 2.2 navi-plugin-runtime

**Crate:** `navi-plugin-runtime`
**Role:** WASM Component loading and execution via Wasmtime.

Responsibilities:

- Load WASM Component modules from disk.
- Configure Wasmtime `Engine` with:
  - Fuel limit (default: 1 billion instructions).
  - Memory limit (default: 64 MB linear memory).
  - Wall-clock timeout (default: 30 seconds).
  - Stack size (default: 1 MB).
- Instantiate a fresh `Store` per tool invocation.
- Call the `run-tool` exported function with serialized tool input.
- Trap on fuel exhaustion, memory limit, or timeout.
- Return serialized tool output or a structured error.

This crate MUST NOT validate policy. It trusts the caller to have performed authorization before invocation.

**Key Types:**

```rust
pub struct WasmRuntime {
    engine: wasmtime::Engine,
    config: WasmRuntimeConfig,
}

pub struct WasmRuntimeConfig {
    pub fuel_limit: u64,
    pub memory_bytes: usize,
    pub timeout_ms: u64,
    pub stack_bytes: usize,
    pub output_max_bytes: usize,
}

pub struct InvocationResult {
    pub output: Vec<u8>,
    pub fuel_consumed: u64,
    pub duration_ms: u64,
}

pub enum InvocationError {
    FuelExhausted,
    MemoryLimitExceeded,
    Timeout,
    Trap(String),
    InvalidOutput,
}
```

---

### 2.3 navi-plugin-broker

**Crate:** `navi-plugin-broker`
**Role:** Host-mediated access to sensitive resources.

#### FS Broker

Responsibilities:

- Canonicalize requested paths using `std::fs::canonicalize`.
- Resolve all symlinks and verify the final path is within the allowed root.
- Reject null bytes in paths.
- Reject sensitive files (`.git/`, `.env`, `*.pem`, `*.key`, `*.p12`) by default.
- Restrict access to paths declared in the capability scope.
- Enforce per-file size cap (default: 1 MB).
- Enforce total read budget per invocation (default: 10 MB).
- Return file content as bytes; NEVER return raw file handles.
- Reject access to NAVI private storage (`~/.config/navi/`, `~/.local/share/navi/`).

#### HTTP Broker

Responsibilities:

- Validate URL scheme is `https`.
- Resolve DNS and reject:
  - Loopback: `127.0.0.0/8`, `::1`
  - Private: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`
  - Link-local: `169.254.0.0/16`, `fe80::/10`
- Pin DNS resolution to the resolved address for the duration of the request.
- Validate every redirect target against the declared host allowlist.
- Limit redirects to 3 maximum.
- Validate the final URL after all redirects.
- Sanitize response headers: strip `Set-Cookie`, `Authorization`, and other sensitive headers.
- Enforce response body size cap (default: 1 MB).
- Enforce per-plugin rate limits (default: 60 req/min).
- Reject requests to hosts not declared in the capability.

#### Git Broker

Responsibilities:

- Validate the working directory is the project root.
- Execute git commands in a sandboxed subprocess.
- Support `status` and `diff` in the MVP.
- Return structured output (parsed JSON or text).
- Block all write operations in the MVP.
- NOT return raw process handles.

#### Auth Bindings

Responsibilities:

- Store secrets in the OS credential store or encrypted config.
- Inject secrets into HTTP request headers at the broker level.
- Inject secrets only into requests matching the declared host allowlist.
- NEVER expose raw secrets to plugin code.
- NEVER log secrets.
- NEVER include secrets in error messages.

---

### 2.4 navi-plugin-registry

**Crate:** `navi-plugin-registry`
**Role:** Plugin lifecycle management and tool registration.

Responsibilities:

- Register plugin tools into the ToolExecutor alongside built-in tools.
- Namespace tool IDs: `plugin__<plugin-id>__<tool-id>`.
- Generate model-facing tool descriptions from plugin metadata, capability summaries, risk labels, and provenance tags.
- Track installed plugins: ID, version, hash, signature, publisher key.
- Verify WASM module hash (SHA-256) on every load.
- Verify Ed25519 signature on every load.
- Reject plugins with hash mismatch or invalid signature.
- Block updates that change publisher or signing key.
- Trigger reconsent when capabilities or risk classification change.

**Key Types:**

```rust
pub struct PluginEntry {
    pub manifest: PluginManifest,
    pub wasm_hash: [u8; 32],
    pub signature: Option<Ed25519Signature>,
    pub publisher_key: Option<Ed25519PublicKey>,
    pub installed_at: chrono::DateTime<chrono::Utc>,
    pub consent_record: ConsentRecord,
}

pub struct RegisteredTool {
    pub namespaced_id: String,
    pub plugin_id: String,
    pub tool_id: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub risk_label: RiskLabel,
    pub capabilities: Vec<String>,
}

pub enum RiskLabel {
    Low,
    Medium,
    High,
    Critical,
}
```

---

### 2.5 navi-plugin-security

**Crate:** `navi-plugin-security`
**Role:** Risk classification, security defaults enforcement, and audit logging.

Responsibilities:

- Compute risk classification for each tool from its capability set.
- Apply compound risk rules:
  - `fs-read` + `http` (GET) = HIGH
  - `fs-read` + `http` (POST) = CRITICAL
  - `fs-write` + `http` = CRITICAL
  - `auth` + `http` = CRITICAL
- Enforce security defaults from `plugin-security-defaults.md`.
- Block capabilities not declared in the manifest.
- Block community plugins from requesting forbidden capabilities (model, session, approval, shell, process, env, agent policy).
- Generate audit log entries for all security-relevant events.
- NEVER log secret values or full file contents.

---

### 2.6 navi-plugin-tests

**Crate:** `navi-plugin-tests`
**Role:** Security test harness and red-team fixtures.

Responsibilities:

- Provide red-team WASM fixtures that attempt known attack vectors.
- Test symlink escape, path traversal, null byte injection.
- Test DNS rebinding, redirect abuse, private IP access.
- Test resource exhaustion (fuel, memory, time).
- Test capability escalation and undeclared access.
- Test prompt injection via `input_schema` descriptions.
- Test output size limit enforcement.
- Provide a test harness for broker integration tests.

---

## 3. Data Flow

### 3.1 Tool Invocation Flow

```txt
Model         Agent Core    ToolExecutor    Tool Registry    Broker         WASM Runtime
  |               |              |               |              |               |
  | tool_call     |              |               |              |               |
  |-------------->|              |               |              |               |
  |               | invoke       |               |              |               |
  |               |------------->|               |              |               |
  |               |              | resolve       |              |               |
  |               |              |-------------->|              |               |
  |               |              | tool info     |              |               |
  |               |              |<--------------|              |               |
  |               |              |               |              |               |
  |               |              | authorize     |              |               |
  |               |              |----------------------------->|               |
  |               |              |               |              | allowed/denied |
  |               |              |<-----------------------------|               |
  |               |              |               |              |               |
  |               |              | execute       |              |               |
  |               |              |-------------------------------------------->|
  |               |              |               |              |               |
  |               |              |               |              | broker_call    |
  |               |              |               |              |<--------------|
  |               |              |               |              | result         |
  |               |              |               |              |-------------->|
  |               |              |               |              |               |
  |               |              | result        |              |               |
  |               |              |<--------------------------------------------|
  |               | result       |               |              |               |
  |               |<-------------|               |              |               |
  | tool_result   |              |               |              |               |
  |<--------------|              |               |              |               |
```

### 3.2 Plugin Installation Flow

```txt
User            Plugin Registry    Manifest Parser    Security Module
 |                   |                   |                   |
 | install plugin    |                   |                   |
 |------------------>|                   |                   |
 |                   | parse manifest    |                   |
 |                   |------------------>|                   |
 |                   | validated manifest|                   |
 |                   |<------------------|                   |
 |                   |                   |                   |
 |                   | compute risk      |                   |
 |                   |-------------------------------------->|
 |                   | risk classification                   |
 |                   |<--------------------------------------|
 |                   |                   |                   |
 |                   | verify signature  |                   |
 |                   | (signed only)     |                   |
 |                   |                   |                   |
 |                   | prompt reconsent  |                   |
 |<------------------| (if capabilities  |                   |
 | consent/deny      |  changed)         |                   |
 |------------------>|                   |                   |
 |                   | register tools    |                   |
 |                   | (namespaced)      |                   |
 |                   |                   |                   |
 | installed         |                   |                   |
 |<------------------|                   |                   |
```

---

## 4. Crate Dependencies

```txt
navi-cli
  +-- navi-sdk
        +-- navi-core
              |   navi-plugin-manifest
              |   navi-plugin-registry
              |       +-- navi-plugin-manifest
              |       +-- navi-plugin-security
              |   navi-plugin-runtime
              |       +-- wasmtime (external)
              |   navi-plugin-broker
              |       +-- navi-plugin-manifest
              |       +-- navi-plugin-security
              |   navi-plugin-security
              |       +-- navi-plugin-manifest

navi-plugin-tests
  +-- navi-plugin-runtime
  +-- navi-plugin-broker
  +-- navi-plugin-registry
  +-- navi-plugin-security
```

---

## 5. Integration with Existing NAVI Components

### 5.1 ToolExecutor

Plugin tools are registered alongside built-in tools in the `ToolExecutor`. The executor routes tool calls to the appropriate handler based on the tool's provenance:

- Built-in tools: direct execution.
- Plugin tools: broker authorization, then WASM execution.

The `ToolExecutor` MUST treat plugin tool output as untrusted data.

### 5.2 SecurityPolicy

The plugin broker layer uses `SecurityPolicy` for path validation and command blocking. The broker delegates path authorization decisions to the same policy used by built-in file tools.

The `SecurityPolicy` MUST NOT be modified by plugins. It is an immutable component of the agent core.

### 5.3 AgentRuntime

The `AgentRuntime` manages the plugin lifecycle:

- **Load:** Parse manifest, verify hash/signature, register tools.
- **Invoke:** Route tool calls through the broker and runtime.
- **Unload:** Remove plugin tools from the registry.

The runtime MUST track installed plugins and their hashes for integrity verification.

### 5.4 TuiApp

The `TuiApp` receives declarative view models from renderer plugins and renders them using its own layout engine. The TUI MUST NOT expose terminal access, keyboard input, or mouse events to plugins.

Renderer plugin view models are treated as data. The TUI decides how to lay out and render them.

---

## 6. WASM Component Interface

### 6.1 Plugin Exports

Every WASM plugin MUST export:

- `run-tool(tool-id: string, input: string) -> result<string, string>`: Execute a tool by ID with JSON input. Returns JSON output or error.

### 6.2 Host Imports

The host provides the following imports to the WASM module:

- `navi:broker/fs.read(path: string) -> result<bytes, string>`: Read a file through the FS broker.
- `navi:broker/http.request(method: string, url: string, headers: list<tuple<string, string>>, body: option<bytes>) -> result<http-response, string>`: Make an HTTP request through the HTTP broker.
- `navi:broker/git.status() -> result<string, string>`: Get git status through the git broker.
- `navi:broker/git.diff(ref: string) -> result<string, string>`: Get git diff through the git broker.
- `navi:broker/git.log(count: u32) -> result<string, string>`: Get git log through the git broker.

The host MUST NOT provide filesystem, network, shell, process, or environment imports directly.

### 6.3 Resource Limits Configuration

```rust
// Pseudocode for Wasmtime configuration
let mut config = wasmtime::Config::new();
config.consume_fuel(true);
config.max_wasm_stack(1 << 20); // 1 MB stack

let engine = wasmtime::Engine::new(&config)?;

// Per-invocation limits
let fuel_limit = 1_000_000; // fuel units
let memory_limit = 64 << 20; // 64 MB
let timeout = Duration::from_secs(30);
let output_limit = 32 << 10; // 32 KB
```

---

## 7. Manifest Hashing

The manifest hash is computed over the normalized manifest:

1. Parse the TOML manifest into a canonical Rust struct.
2. Serialize the struct to a deterministic format (e.g., canonical JSON or bincode).
3. Compute SHA-256 over the serialized bytes.

This hash is used for:

- **Integrity verification:** The host verifies the manifest hash before loading.
- **Change detection:** The host detects manifest changes between versions.
- **Reconsent triggers:** Manifest hash changes that affect capabilities or tools trigger reconsent.

---

## 8. Signature Scheme

Signed plugins use Ed25519 signatures:

1. The publisher signs the WASM artifact hash (SHA-256) with their Ed25519 private key.
2. The signature is stored alongside the artifact.
3. The host verifies the signature against the publisher's registered public key.
4. Public key changes are blocked by default.

The signature file format:

```toml
[signature]
algorithm = "ed25519"
public_key = "<base64-encoded public key>"
signature = "<base64-encoded signature>"
content_hash = "<hex-encoded SHA-256 of the WASM artifact>"
```

---

## 9. Future Considerations

The following are explicitly out of scope for the initial implementation but MAY be considered in future versions:

- **Write access for signed plugins:** Signed plugins MAY be granted filesystem write access with explicit user consent.
- **Plugin storage:** Persistent key-value storage scoped to the plugin.
- **Inter-plugin communication:** Mediated by the host, not direct IPC.
- **Plugin marketplace:** A registry service for discovering and installing plugins.
- **Hot reload:** Reloading plugins without restarting the session.
