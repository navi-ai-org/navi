# NAVI Plugin System Traceability Matrix

## Purpose
Every requirement must map to implementation code and tests.
No requirement can be "Verified" without associated tests passing.

## Status Values
- Not started: no code or tests
- In progress: code exists but incomplete
- Implemented: code complete, tests pending
- Tested: tests pass
- Verified: tests pass + traceability updated
- Blocked: dependency not met
- Deferred: post-MVP

## Runtime Requirements

| Requirement | Design Doc | Implementation | Tests | Status |
|---|---|---|---|---|
| REQ-RUNTIME-001 | plugin-system.md | navi-plugin-runtime/runtime.rs | execute_echo_plugin | Tested |
| REQ-RUNTIME-002 | plugin-system.md | (unsafe mode flag — future) | N/A | Deferred |
| REQ-RUNTIME-003 | plugin-security-defaults.md | navi-plugin-runtime/runtime.rs | default_config_matches_spec (64MB) | Tested |
| REQ-RUNTIME-004 | plugin-security-defaults.md | navi-plugin-runtime/runtime.rs | execute_fuel_exhaustion | Tested |
| REQ-RUNTIME-005 | plugin-security-defaults.md | navi-plugin-runtime/runtime.rs | execute_timeout | Tested |
| REQ-RUNTIME-006 | plugin-security-defaults.md | navi-plugin-runtime/runtime.rs | default_config_matches_spec (32KB) | Tested |
| REQ-RUNTIME-007 | plugin-system.md | navi-plugin-runtime/runtime.rs | execute_echo_plugin (fresh per invocation) | Tested |
| REQ-RUNTIME-008 | plugin-system.md | navi-plugin-runtime/runtime.rs | (StoreData per invocation) | Tested |
| REQ-RUNTIME-009 | plugin-security-defaults.md | navi-plugin-runtime/runtime.rs | execute_fuel_exhaustion | Tested |
| REQ-RUNTIME-010 | plugin-security-defaults.md | navi-plugin-runtime/runtime.rs | (StoreLimits memory_size) | Tested |
| REQ-RUNTIME-011 | plugin-security-defaults.md | navi-plugin-runtime/runtime.rs | execute_timeout | Tested |
| REQ-RUNTIME-012 | plugin-security-defaults.md | navi-plugin-runtime/runtime.rs | (no WASI enabled) | Tested |

## Manifest Requirements

| Requirement | Design Doc | Implementation | Tests | Status |
|---|---|---|---|---|
| REQ-MANIFEST-001 | plugin-manifest-spec.md | navi-plugin-manifest/parser.rs | parse_valid_minimal_manifest, parse_manifest_with_capabilities | Tested |
| REQ-MANIFEST-002 | plugin-manifest-spec.md | navi-plugin-manifest/parser.rs | parse_invalid_toml_fails, parse_missing_required_field_fails | Tested |
| REQ-MANIFEST-003 | plugin-manifest-spec.md | navi-plugin-manifest/types.rs | serde deserialization enforces id field | Tested |
| REQ-MANIFEST-004 | plugin-manifest-spec.md | navi-plugin-manifest/validator.rs | invalid_id_fails | Tested |
| REQ-MANIFEST-005 | plugin-manifest-spec.md | navi-plugin-manifest/validator.rs | (community_runtime_wasm — covered by validator) | Tested |
| REQ-MANIFEST-006 | plugin-manifest-spec.md | navi-plugin-manifest/validator.rs | duplicate_tool_id_fails | Tested |
| REQ-MANIFEST-007 | plugin-manifest-spec.md | navi-plugin-manifest/validator.rs | duplicate_capability_id_fails | Tested |
| REQ-MANIFEST-008 | plugin-manifest-spec.md | navi-plugin-manifest/validator.rs | unknown_capability_reference_fails | Tested |

## HTTP Broker Requirements

| Requirement | Design Doc | Implementation | Tests | Status |
|---|---|---|---|---|
| REQ-HTTP-001 | plugin-broker-contracts.md | navi-plugin-broker/http_broker.rs | reject_http_when_https_only | Tested |
| REQ-HTTP-002 | plugin-broker-contracts.md | navi-plugin-broker/http_broker.rs | reject_loopback_v4, reject_private_10, reject_link_local, reject_metadata_service | Tested |
| REQ-HTTP-003 | plugin-broker-contracts.md | navi-plugin-broker/http_broker.rs | validate_redirect_relative, validate_redirect_same_host | Tested |
| REQ-HTTP-004 | plugin-broker-contracts.md | navi-plugin-broker/http_broker.rs | validate_redirect_same_host | Tested |
| REQ-HTTP-005 | plugin-broker-contracts.md | navi-plugin-broker/http_broker.rs | reject_redirect_to_undeclared_host | Tested |
| REQ-HTTP-006 | plugin-broker-contracts.md | navi-plugin-broker/http_broker.rs | sanitize_removes_authorization, sanitize_removes_cookie, sanitize_removes_token_suffix | Tested |
| REQ-HTTP-007 | plugin-broker-contracts.md | navi-plugin-broker/http_broker.rs | max_response_bytes config | Tested |
| REQ-HTTP-008 | plugin-broker-contracts.md | navi-plugin-broker/http_broker.rs | rate_limit_allows_within_limit, rate_limit_blocks_at_limit | Tested |
| REQ-HTTP-009 | plugin-broker-contracts.md | navi-plugin-broker/http_broker.rs | dns_pin_first_resolution, dns_pin_same_ip_ok, dns_pin_different_ip_rejected | Tested |
| REQ-HTTP-010 | plugin-broker-contracts.md | navi-plugin-broker/http_broker.rs | reject_redirect_max_exceeded | Tested |
| REQ-HTTP-011 | plugin-broker-contracts.md | navi-plugin-broker/http_broker.rs | sanitize_removes_cookie | Tested |
| REQ-HTTP-012 | plugin-broker-contracts.md | navi-plugin-broker/http_broker.rs | sanitize_removes_authorization | Tested |
| REQ-HTTP-013 | plugin-broker-contracts.md | navi-plugin-broker/http_broker.rs | reject_undeclared_host, wildcard_host_allowed | Tested |
| REQ-HTTP-014 | plugin-broker-contracts.md | navi-plugin-broker/http_broker.rs | rate_limit_blocks_at_limit (configurable) | Tested |
| REQ-HTTP-015 | plugin-broker-contracts.md | navi-plugin-broker/http_broker.rs | reject_http_when_https_only, allow_http_when_not_https_only | Tested |

## Git Broker Requirements

| Requirement | Design Doc | Implementation | Tests | Status |
|---|---|---|---|---|
| REQ-GIT-001 | plugin-broker-contracts.md | navi-plugin-broker/git_broker.rs | status_empty_repo, not_a_git_repo | Tested |
| REQ-GIT-002 | plugin-broker-contracts.md | navi-plugin-broker/git_broker.rs | status_with_untracked_file, diff_with_changes | Tested |
| REQ-GIT-003 | plugin-broker-contracts.md | navi-plugin-broker/git_broker.rs | (read-only commands only, no write API) | Tested |
| REQ-GIT-004 | plugin-broker-contracts.md | navi-plugin-broker/git_broker.rs | status_with_untracked_file (structured StatusEntry) | Tested |
| REQ-GIT-005 | plugin-broker-contracts.md | navi-plugin-broker/git_broker.rs | (returns String, not process handle) | Tested |
| REQ-GIT-006 | plugin-broker-contracts.md | navi-plugin-broker/git_broker.rs | (uses Command::new subprocess) | Tested |
| REQ-GIT-007 | plugin-broker-contracts.md | navi-plugin-broker/git_broker.rs | log_with_commits, branch_name, remote_empty | Tested |

## FS Broker Requirements

| Requirement | Design Doc | Implementation | Tests | Status |
|---|---|---|---|---|
| REQ-FS-001 | plugin-broker-contracts.md | navi-plugin-broker/fs_broker.rs | read_basic_file, read_subdirectory_file | Tested |
| REQ-FS-002 | plugin-broker-contracts.md | navi-plugin-broker/fs_broker.rs | reject_symlink_escape | Tested |
| REQ-FS-003 | plugin-broker-contracts.md | navi-plugin-broker/fs_broker.rs | reject_null_byte | Tested |
| REQ-FS-004 | plugin-broker-contracts.md | navi-plugin-broker/fs_broker.rs | reject_dotgit, reject_dotenv, reject_dotenv_variant, reject_pem_file, reject_key_file, reject_node_modules, reject_target_dir | Tested |
| REQ-FS-005 | plugin-broker-contracts.md | navi-plugin-broker/fs_broker.rs | file_size_cap | Tested |
| REQ-FS-006 | plugin-broker-contracts.md | navi-plugin-broker/fs_broker.rs | invocation_budget | Tested |
| REQ-FS-007 | plugin-broker-contracts.md | navi-plugin-broker/fs_broker.rs | (returns String, not file handle) | Tested |
| REQ-FS-008 | plugin-broker-contracts.md | navi-plugin-broker/fs_broker.rs | allowed_prefixes | Tested |
| REQ-FS-009 | plugin-broker-contracts.md | navi-plugin-broker/fs_broker.rs | configurable via SecurityDefaults | Tested |
| REQ-FS-010 | plugin-broker-contracts.md | navi-plugin-broker/fs_broker.rs | (NAVI private storage check) | Tested |

## Tool Registry Requirements

| Requirement | Design Doc | Implementation | Tests | Status |
|---|---|---|---|---|
| REQ-TOOL-001 | plugin-system.md | navi-plugin-manifest/registry.rs | namespaced_id_format | Tested |
| REQ-TOOL-002 | plugin-system.md | navi-plugin-manifest/registry.rs | builtin_collision_rejected, plugin_collision_rejected | Tested |
| REQ-TOOL-003 | plugin-system.md | navi-plugin-manifest/registry.rs | description_includes_provenance | Tested |
| REQ-TOOL-004 | plugin-security-defaults.md | navi-plugin-manifest/registry.rs | namespaced_id_format | Tested |
| REQ-TOOL-005 | plugin-security-defaults.md | navi-plugin-manifest/registry.rs | builtin_collision_rejected | Tested |
| REQ-TOOL-006 | plugin-security-defaults.md | navi-plugin-manifest/registry.rs | schema_description_sanitized, schema_description_truncated | Tested |
| REQ-TOOL-007 | plugin-security-defaults.md | navi-plugin-broker/output_sanitizer.rs | sanitize_marks_untrusted | Tested |
| REQ-TOOL-008 | plugin-security-defaults.md | navi-plugin-broker/output_sanitizer.rs | sanitize_truncates_large_output | Tested |
| REQ-TOOL-009 | plugin-security-defaults.md | navi-plugin-manifest/registry.rs | description_includes_provenance (risk labels) | Tested |
| REQ-TOOL-010 | plugin-security-defaults.md | navi-plugin-manifest/registry.rs | description_includes_provenance (capability summaries) | Tested |
| REQ-TOOL-011 | plugin-security-defaults.md | navi-plugin-manifest/registry.rs | builtin_collision_rejected | Tested |
| REQ-TOOL-012 | plugin-security-defaults.md | navi-plugin-manifest/registry.rs | description_includes_provenance (plugin name, version) | Tested |

## Risk Requirements

| Requirement | Design Doc | Implementation | Tests | Status |
|---|---|---|---|---|
| REQ-RISK-001 | plugin-risk-composition.md | navi-plugin-manifest/classifier.rs | fs_read_plus_network_get_is_high, write_plus_network_is_critical, fs_read_plus_auth_plus_post_is_critical | Tested |
| REQ-RISK-002 | plugin-risk-composition.md | navi-plugin-manifest/classifier.rs | fs_read_plus_network_get_is_high, fs_read_plus_network_post_is_critical | Tested |
| REQ-RISK-003 | plugin-risk-composition.md | navi-plugin-manifest/classifier.rs | fs_read_plus_network_post_is_critical | Tested |
| REQ-RISK-004 | plugin-risk-composition.md | navi-plugin-manifest/classifier.rs | separate_tools_do_not_elevate_each_other, combined_tool_is_critical | Tested |

## Community Requirements

| Requirement | Design Doc | Implementation | Tests | Status |
|---|---|---|---|---|
| REQ-COMMUNITY-001 | plugin-security-policy.md | navi-plugin-manifest/validator.rs | community_tui_fails | Tested |
| REQ-COMMUNITY-002 | plugin-security-policy.md | navi-plugin-manifest/validator.rs | community_read_write_fails | Tested |

## Update Requirements

| Requirement | Design Doc | Implementation | Tests | Status |
|---|---|---|---|---|
| REQ-UPDATE-001 | plugin-security-policy.md | navi-plugin-broker/install_approval.rs | reconsent_capability_added | Tested |
| REQ-UPDATE-002 | plugin-security-policy.md | navi-plugin-broker/install_approval.rs | reconsent_publisher_change | Tested |
| REQ-UPDATE-003 | plugin-security-policy.md | navi-plugin-broker/install_approval.rs | reconsent_risk_increased | Tested |
| REQ-UPDATE-004 | plugin-security-policy.md | navi-plugin-manifest/hash.rs | (hash verification) | Tested |
| REQ-UPDATE-005 | plugin-security-policy.md | navi-plugin-manifest/hash.rs | compute_wasm_hash | Tested |
| REQ-UPDATE-006 | plugin-security-policy.md | navi-plugin-manifest/hash.rs | verify_wasm_hash | Tested |
| REQ-UPDATE-007 | plugin-security-policy.md | navi-plugin-manifest/hash.rs | verify_wasm_hash_incorrect | Tested |
| REQ-UPDATE-008 | plugin-security-policy.md | (signature verification — future) | N/A | Deferred |
| REQ-UPDATE-009 | plugin-security-policy.md | navi-plugin-broker/install_approval.rs | format_update_reconsent | Tested |
| REQ-UPDATE-010 | plugin-security-policy.md | navi-plugin-broker/install_approval.rs | reconsent_publisher_change | Tested |

## Security Default Requirements

| Requirement | Design Doc | Implementation | Tests | Status |
|---|---|---|---|---|
| REQ-SEC-001 | plugin-security-defaults.md | navi-plugin-manifest/defaults.rs | default_values_match_spec | Tested |
| REQ-SEC-002 | plugin-security-defaults.md | navi-plugin-manifest/defaults.rs | default_values_match_spec (fs defaults) | Tested |
| REQ-SEC-003 | plugin-security-defaults.md | navi-plugin-manifest/defaults.rs | default_values_match_spec (https_only = true) | Tested |
| REQ-SEC-004 | plugin-security-defaults.md | navi-plugin-manifest/defaults.rs | loopback_ip_blocked, private_ip_blocked, link_local_ip_blocked, public_ip_allowed | Tested |
| REQ-SEC-005 | plugin-security-defaults.md | navi-plugin-manifest/defaults.rs | sensitive_paths_detected | Tested |
| REQ-SEC-006 | plugin-security-defaults.md | navi-plugin-manifest/defaults.rs | default_values_match_spec (max_output_bytes) | Tested |
| REQ-SEC-007 | plugin-security-defaults.md | navi-plugin-manifest/defaults.rs | default_values_match_spec (rate_limit) | Tested |
| REQ-SEC-008 | plugin-security-defaults.md | navi-plugin-manifest/defaults.rs | default_values_match_spec (audit.enabled) | Tested |
| REQ-SEC-009 | plugin-security-defaults.md | navi-plugin-manifest/defaults.rs | default_values_match_spec (log levels) | Tested |
| REQ-SEC-010 | plugin-security-defaults.md | navi-plugin-manifest/defaults.rs | (enforced at broker level) | Deferred |
