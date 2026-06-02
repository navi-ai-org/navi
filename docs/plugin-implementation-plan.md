# NAVI Plugin System Implementation Plan

## M0 — Documentation Baseline
Goal: Create all plugin system docs and freeze MVP scope.
Deliverables: All docs in docs/plugin-*.md, ADRs, AGENT_IMPLEMENTATION_PROTOCOL.md
Exit criteria: Docs reviewed, requirement IDs stable, MVP scope frozen.

## M1 — Manifest + Lockfile
Goal: Parse and validate plugin manifests.
Deliverables: navi-plugin-manifest crate, manifest parser, validator, lockfile structs, hash calculation
Tests: valid manifest parses, invalid manifest fails, duplicate tool ID fails, unknown capability reference fails
Exit criteria: REQ-MANIFEST-* verified.

## M2 — Risk Composition Classifier
Goal: Implement compound capability risk analysis.
Deliverables: risk_classifier.rs, severity enum, compound risk table, warning text generation
Tests: all combinations from risk-composition.md tested, per-tool analysis
Exit criteria: REQ-RISK-* verified.

## M3 — Security Defaults Config
Goal: Define all security limits as a configuration struct.
Deliverables: SecurityDefaults struct, all limits from plugin-security-defaults.md
Tests: defaults match spec, no limit can be set to zero/unlimited
Exit criteria: All security defaults loadable.

## M4 — Tool Registry + Namespacing
Goal: Register plugin tools with namespaced IDs and host-generated descriptions.
Deliverables: tool_registry.rs, ID namespacing, description generator, schema sanitizer
Tests: namespaced format correct, collision denied, description host-generated, schema sanitized
Exit criteria: REQ-TOOL-* verified.

## M5 — Wasmtime Runtime with Limits
Goal: Load WASM components with mandatory resource limits.
Deliverables: wasm_runtime.rs, Wasmtime configuration, fuel/memory/timeout, run-tool execution
Tests: echo plugin loads, runs, returns result; timeout kills; memory limit enforced; fuel limit enforced
Exit criteria: REQ-RUNTIME-* verified.

## M6 — FS Broker
Goal: Mediate filesystem access with full validation.
Deliverables: fs_broker.rs, canonicalization, symlink resolution, denylist, size caps
Tests: path traversal denied, symlink escape denied, .env denied, null byte denied, normal read allowed
Exit criteria: REQ-FS-* verified.

## M7 — HTTP Broker
Goal: Mediate network access with full validation.
Deliverables: http_broker.rs, URL validation, DNS resolution, IP blocking, redirect validation, header sanitization, rate limiting
Tests: localhost denied, private IP denied, redirect to metadata denied, redirect to undeclared denied, DNS rebinding denied, header sanitized
Exit criteria: REQ-HTTP-* verified.

## M8 — Git Broker
Goal: Provide read-only git access.
Deliverables: git_broker.rs, status, diff, project-scoped
Tests: status works, diff works, write unavailable
Exit criteria: REQ-GIT-* verified.

## M9 — Output Sanitizer
Goal: Sanitize and truncate plugin tool output.
Deliverables: output_sanitizer.rs, truncation, untrusted marking, instruction pattern stripping
Tests: output truncated at 32KB, marked as untrusted, system instructions sanitized
Exit criteria: REQ-TOOL-007, REQ-TOOL-008 verified.

## M10 — Install Approval + Reconsent
Goal: Show capabilities at install, require reconsent on changes.
Deliverables: install UI, severity labels, update diff, reconsent policy
Tests: HIGH risk shown, CRITICAL risk requires explicit consent, update with new capability blocked
Exit criteria: REQ-UPDATE-* verified.

## M11 — Red-Team Fixtures
Goal: All 10 red-team plugins tested and passing.
Deliverables: 10 test plugin WASM fixtures, all tests from plugin-redteam-suite.md
Tests: all red-team tests pass
Exit criteria: No red-team test fails.

## Dependencies
M2 depends on M1 (needs manifest types)
M4 depends on M1 (needs manifest types)
M5 depends on M3 (needs security defaults)
M6 depends on M3 (needs security defaults)
M7 depends on M3 (needs security defaults)
M9 depends on M5 (needs runtime)
M10 depends on M2 (needs risk classifier)
M11 depends on all previous milestones.
