# NAVI Plugin System Requirements

**Status:** Draft
**Version:** 0.1.0
**Date:** 2026-06-01

Every requirement in this document has a stable ID for traceability. Requirements use RFC 2119 language: MUST, MUST NOT, SHOULD, MAY.

---

## 1. Runtime Requirements

| ID | Requirement |
|---|---|
| REQ-RUNTIME-001 | Community plugins MUST run as WASM Components. |
| REQ-RUNTIME-002 | Native in-process plugins MUST be restricted to core plugins or explicit unsafe local-dev mode. |
| REQ-RUNTIME-003 | WASM invocations MUST run with memory limits (64 MB default). |
| REQ-RUNTIME-004 | WASM invocations MUST run with execution limits (fuel). |
| REQ-RUNTIME-005 | WASM invocations MUST run with wall-clock timeout (30 s default). |
| REQ-RUNTIME-006 | Tool output MUST be size-limited (32 KB default). |
| REQ-RUNTIME-007 | The host MUST instantiate a fresh WASM module per tool invocation. |
| REQ-RUNTIME-008 | The host MUST NOT share mutable state between WASM invocations. |
| REQ-RUNTIME-009 | The WASM runtime MUST trap on fuel exhaustion. |
| REQ-RUNTIME-010 | The WASM runtime MUST trap on memory limit exceeded. |
| REQ-RUNTIME-011 | The WASM runtime MUST trap on wall-clock timeout. |
| REQ-RUNTIME-012 | The WASM runtime MUST NOT expose WASI filesystem, environment, clock, or random unless mediated by a broker. |
| REQ-RUNTIME-013 | Subprocess plugins MUST be sandboxed with Landlock and seccomp. |
| REQ-RUNTIME-014 | Subprocess plugins MUST communicate over framed JSON-RPC on stdin/stdout. |
| REQ-RUNTIME-015 | The host MUST terminate the subprocess on protocol violation or timeout. |

---

## 2. Manifest Requirements

| ID | Requirement |
|---|---|
| REQ-MANIFEST-001 | Capabilities MUST be declared in the manifest. |
| REQ-MANIFEST-002 | Capabilities MUST be scoped per tool. |
| REQ-MANIFEST-003 | `plugin.id` MUST be stable across versions. |
| REQ-MANIFEST-004 | `plugin.id` MUST match `[a-z0-9][a-z0-9-_]{1,63}`. |
| REQ-MANIFEST-005 | `runtime` MUST be `wasm-component` for community plugins. |
| REQ-MANIFEST-006 | `tools[].id` MUST be unique within the plugin. |
| REQ-MANIFEST-007 | `capabilities[].id` MUST be unique within the plugin. |
| REQ-MANIFEST-008 | `tools[].capabilities` MUST reference existing capabilities. |
| REQ-MANIFEST-009 | The manifest MUST declare `id`, `name`, `version`, `author`, and `runtime`. |
| REQ-MANIFEST-010 | The manifest SHOULD declare `description` and `license`. |
| REQ-MANIFEST-011 | The manifest MUST be valid TOML. |
| REQ-MANIFEST-012 | The host MUST reject plugins with malformed manifests. |
| REQ-MANIFEST-013 | The host MUST reject plugins with missing required fields. |
| REQ-MANIFEST-014 | The host MUST reject plugins with duplicate tool IDs. |
| REQ-MANIFEST-015 | The host MUST reject plugins with duplicate capability IDs. |
| REQ-MANIFEST-016 | The host MUST reject plugins whose tool capabilities reference non-existent capability IDs. |

---

## 3. Capability Requirements

| ID | Requirement |
|---|---|
| REQ-CAP-001 | Capabilities MUST be declared in the manifest. |
| REQ-CAP-002 | Capabilities MUST be scoped per tool. |
| REQ-CAP-003 | Capability composition MUST affect risk classification. |
| REQ-CAP-004 | The host MUST NOT grant a tool access to capabilities it did not declare. |
| REQ-CAP-005 | Each capability MUST have a severity level: LOW, MEDIUM, HIGH, or CRITICAL. |
| REQ-CAP-006 | The host MUST compute risk from the highest severity produced by any single capability or composition. |
| REQ-CAP-007 | Capabilities MUST be unique within a plugin. |
| REQ-CAP-008 | Capabilities MUST include a human-readable description. |
| REQ-CAP-009 | Capabilities MUST specify scope parameters (e.g., `paths` for fs-read, `hosts` for http). |

---

## 4. HTTP Broker Requirements

| ID | Requirement |
|---|---|
| REQ-HTTP-001 | The HTTP broker MUST enforce HTTPS by default. |
| REQ-HTTP-002 | The HTTP broker MUST reject loopback, private, and link-local IPs by default. |
| REQ-HTTP-003 | The HTTP broker MUST validate every redirect target. |
| REQ-HTTP-004 | The HTTP broker MUST validate the final URL after redirects. |
| REQ-HTTP-005 | The HTTP broker MUST reject redirects to undeclared hosts. |
| REQ-HTTP-006 | The HTTP broker MUST sanitize response headers before returning to plugins. |
| REQ-HTTP-007 | The HTTP broker MUST enforce response body size cap (1 MB default). |
| REQ-HTTP-008 | The HTTP broker MUST enforce rate limits per plugin (60 req/min default). |
| REQ-HTTP-009 | The HTTP broker MUST pin DNS resolution to prevent DNS rebinding. |
| REQ-HTTP-010 | The HTTP broker MUST limit redirects to 3 maximum. |
| REQ-HTTP-011 | The HTTP broker MUST strip `Set-Cookie` headers from responses. |
| REQ-HTTP-012 | The HTTP broker MUST strip `Authorization` headers from responses. |
| REQ-HTTP-013 | The HTTP broker MUST reject requests to hosts not declared in the capability. |
| REQ-HTTP-014 | The HTTP broker SHOULD support configurable per-plugin rate limits. |
| REQ-HTTP-015 | The HTTP broker MUST reject `http://` scheme URLs unless explicitly allowed. |

---

## 5. Filesystem Broker Requirements

| ID | Requirement |
|---|---|
| REQ-FS-001 | The FS broker MUST canonicalize paths before authorization. |
| REQ-FS-002 | The FS broker MUST reject symlink escapes outside the project root. |
| REQ-FS-003 | The FS broker MUST reject null bytes in paths. |
| REQ-FS-004 | The FS broker MUST reject sensitive files by default (`.git/`, `.env`, `*.pem`, `*.key`, `*.p12`). |
| REQ-FS-005 | The FS broker MUST enforce file size caps (1 MB per file default). |
| REQ-FS-006 | The FS broker MUST enforce total read budget per invocation (10 MB default). |
| REQ-FS-007 | The FS broker MUST NOT return raw file handles to plugins. |
| REQ-FS-008 | The FS broker MUST restrict access to paths declared in the capability. |
| REQ-FS-009 | The FS broker SHOULD support configurable sensitive file blocklists. |
| REQ-FS-010 | The FS broker MUST reject access to NAVI private storage (`~/.config/navi/`, `~/.local/share/navi/`). |

---

## 6. Git Broker Requirements

| ID | Requirement |
|---|---|
| REQ-GIT-001 | The git broker MUST restrict operations to the project root. |
| REQ-GIT-002 | The git broker MUST support `status` and `diff` in the MVP. |
| REQ-GIT-003 | The git broker MUST block all write operations in the MVP. |
| REQ-GIT-004 | The git broker MUST return structured output. |
| REQ-GIT-005 | The git broker MUST NOT return raw process handles to plugins. |
| REQ-GIT-006 | The git broker MUST execute git commands in a sandboxed subprocess. |
| REQ-GIT-007 | The git broker SHOULD support `log`, `branch`, and `remote` as read-only operations. |

---

## 7. Auth Binding Requirements

| ID | Requirement |
|---|---|
| REQ-AUTH-001 | Auth bindings MUST inject secrets into approved requests only. |
| REQ-AUTH-002 | Auth bindings MUST inject secrets into HTTP headers or environment at the broker level. |
| REQ-AUTH-003 | The plugin MUST NOT receive raw secrets. |
| REQ-AUTH-004 | The host MUST store secrets in the OS credential store or encrypted config. |
| REQ-AUTH-005 | The host MUST inject secrets only into requests matching the declared host allowlist. |
| REQ-AUTH-006 | The host MUST NOT log secrets. |
| REQ-AUTH-007 | The host MUST NOT expose secrets in error messages. |

---

## 8. Tool Registry Requirements

| ID | Requirement |
|---|---|
| REQ-TOOL-001 | Plugin tool names MUST use provenance format: `plugin__<plugin-id>__<tool-id>`. |
| REQ-TOOL-002 | Plugins MUST NOT shadow built-in tools. |
| REQ-TOOL-003 | The host MUST generate model-facing tool descriptions. |
| REQ-TOOL-004 | Plugin tool IDs MUST be namespaced. |
| REQ-TOOL-005 | Plugin tool IDs MUST NOT collide with built-in tools. |
| REQ-TOOL-006 | Plugin-provided `input_schema` descriptions MUST be sanitized for prompt injection. |
| REQ-TOOL-007 | Plugin output MUST be marked as untrusted data. |
| REQ-TOOL-008 | Plugin output MUST be truncated to size limit (32 KB default). |
| REQ-TOOL-009 | The host MUST prepend provenance and risk labels to tool descriptions. |
| REQ-TOOL-010 | The host MUST include capability summaries in generated tool descriptions. |
| REQ-TOOL-011 | The host MUST reject plugins whose tool IDs collide with built-in tools after namespacing. |
| REQ-TOOL-012 | Tool descriptions MUST include the plugin name, author, and version. |

---

## 9. TUI Requirements

| ID | Requirement |
|---|---|
| REQ-TUI-001 | Renderer plugins MUST produce declarative view models. |
| REQ-TUI-002 | The host MUST render all terminal output. |
| REQ-TUI-003 | Plugins MUST NOT access the terminal directly. |
| REQ-TUI-004 | Plugins MUST NOT write to stdout or stderr. |
| REQ-TUI-005 | Plugins MUST NOT read from stdin. |
| REQ-TUI-006 | Plugins MUST NOT access terminal capabilities (colors, cursor, raw mode). |
| REQ-TUI-007 | The host MAY reject or simplify view primitives based on terminal capabilities. |
| REQ-TUI-008 | View primitives MUST be limited to: `text`, `table`, `list`, `key-value`, `progress`, `section`. |

---

## 10. Risk Requirements

| ID | Requirement |
|---|---|
| REQ-RISK-001 | The system MUST compute risk from capability composition. |
| REQ-RISK-002 | A tool with filesystem read and network access MUST be classified as HIGH or CRITICAL. |
| REQ-RISK-003 | A tool with filesystem read and network POST MUST be classified as CRITICAL. |
| REQ-RISK-004 | Capability risk MUST be computed per tool, not per plugin. |
| REQ-RISK-005 | A tool with auth and network access MUST be classified as CRITICAL. |
| REQ-RISK-006 | A tool with filesystem write and network access MUST be classified as CRITICAL. |
| REQ-RISK-007 | Risk labels MUST be displayed in the TUI tool approval prompt. |
| REQ-RISK-008 | Risk labels MUST be included in host-generated tool descriptions. |

---

## 11. Community Plugin Requirements

| ID | Requirement |
|---|---|
| REQ-COMMUNITY-001 | Community plugins MUST NOT request model, session, approval, shell, process, env, or agent policy capabilities. |
| REQ-COMMUNITY-002 | Community plugins MUST NOT request write access in the MVP. |
| REQ-COMMUNITY-003 | Community plugins MUST run as WASM Components. |
| REQ-COMMUNITY-004 | Community plugins MUST NOT access the agent loop. |
| REQ-COMMUNITY-005 | Community plugins MUST NOT modify approval policy. |
| REQ-COMMUNITY-006 | Community plugins MUST NOT read environment variables directly. |
| REQ-COMMUNITY-007 | Community plugins MUST NOT execute shell commands. |
| REQ-COMMUNITY-008 | Community plugins MUST NOT spawn child processes. |
| REQ-COMMUNITY-009 | Community plugins MUST NOT access `.git/` metadata directly. |
| REQ-COMMUNITY-010 | Community plugins MUST NOT access credential stores. |
| REQ-COMMUNITY-011 | Community plugins MUST NOT persist state across invocations without declaration. |

---

## 12. Update Requirements

| ID | Requirement |
|---|---|
| REQ-UPDATE-001 | Updates that add capabilities MUST require reconsent. |
| REQ-UPDATE-002 | Updates that change publisher or signing key MUST be blocked by default. |
| REQ-UPDATE-003 | Updates that change tool risk classification MUST require reconsent. |
| REQ-UPDATE-004 | WASM hash changes without valid signature MUST be blocked. |
| REQ-UPDATE-005 | The host MUST compute and store a SHA-256 hash of the WASM module on install. |
| REQ-UPDATE-006 | The host MUST verify the WASM hash on every load. |
| REQ-UPDATE-007 | Hash mismatch MUST prevent loading. |
| REQ-UPDATE-008 | The host MUST verify Ed25519 signatures against the publisher's public key. |
| REQ-UPDATE-009 | The host MUST display a diff of capability changes on reconsent. |
| REQ-UPDATE-010 | The host MUST display a warning on publisher key change. |

---

## 13. Security Default Requirements

| ID | Requirement |
|---|---|
| REQ-SEC-001 | The host MUST enforce security defaults unless the user explicitly overrides them. |
| REQ-SEC-002 | The host MUST default to read-only filesystem access for all plugin categories. |
| REQ-SEC-003 | The host MUST default to HTTPS-only for HTTP broker. |
| REQ-SEC-004 | The host MUST default to blocking loopback, private, and link-local IPs. |
| REQ-SEC-005 | The host MUST default to rejecting sensitive files. |
| REQ-SEC-006 | The host MUST enforce output size limits. |
| REQ-SEC-007 | The host MUST enforce rate limits. |
| REQ-SEC-008 | The host MUST log all plugin invocations for audit. |
| REQ-SEC-009 | The host MUST log all security policy violations. |
| REQ-SEC-010 | The host MUST NOT log secrets or sensitive content. |

---

## 14. Cross-Cutting Requirements

| ID | Requirement |
|---|---|
| REQ-XC-001 | The host MUST report plugin errors to the user in a human-readable format. |
| REQ-XC-002 | The host MUST NOT crash on plugin errors. |
| REQ-XC-003 | The host MUST isolate plugin failures from each other. |
| REQ-XC-004 | The host MUST isolate plugin failures from the agent core. |
| REQ-XC-005 | The host SHOULD provide a plugin management CLI (`navi plugin list`, `navi plugin install`, `navi plugin remove`). |
| REQ-XC-006 | The host SHOULD provide a plugin health check mechanism. |
| REQ-XC-007 | The host MUST support plugin uninstallation without side effects. |
| REQ-XC-008 | The host MUST validate plugin manifests on install and on load. |
