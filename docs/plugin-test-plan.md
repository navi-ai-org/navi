# NAVI Plugin System Test Plan

## Unit Tests

### Manifest Parser
- valid minimal manifest parses
- valid manifest with all fields parses
- missing required field fails
- invalid plugin.id format fails
- duplicate tool ID fails
- duplicate capability ID fails
- tool references nonexistent capability fails
- community plugin with forbidden capability fails

### Risk Classifier
- fs_read only = MEDIUM
- network_GET only = MEDIUM
- network_POST only = HIGH
- fs_read + network_GET = HIGH
- fs_read + network_POST = CRITICAL
- fs_read + auth_binding = HIGH
- fs_read + auth_binding + POST = CRITICAL
- write + network = CRITICAL
- per-tool risk computed correctly

### Path Canonicalizer
- relative path resolved
- absolute path rejected if outside project
- symlink resolved to real path
- symlink escape rejected
- null byte rejected
- path traversal rejected
- unicode normalized

### URL Validator
- https allowed
- http rejected by default
- undeclared host rejected
- declared host allowed
- private IP rejected
- loopback IP rejected
- link-local IP rejected
- metadata IP rejected

### Tool ID Namespacing
- format: plugin__{id}__{tool}
- builtin collision rejected
- plugin collision handled

### Schema Sanitizer
- description truncated
- instruction-like text stripped
- default values validated

### Output Sanitizer
- output truncated at 32KB
- marked as untrusted
- system-like instructions stripped

## Integration Tests

### Plugin Loading
- load valid WASM plugin
- reject invalid WASM
- register plugin tools
- execute run-tool

### FS Broker
- read allowed project file
- deny symlink escape
- deny .env read
- deny .git/ read
- deny path traversal
- allow normal file

### HTTP Broker
- request to declared host succeeds
- request to undeclared host denied
- redirect to private IP denied
- redirect to undeclared host denied
- response headers sanitized
- response body capped
- rate limit enforced

### Git Broker
- git status works
- git diff works

### Install Approval
- capabilities displayed
- HIGH risk warning shown
- CRITICAL risk requires explicit consent

### Update Reconsent
- same capabilities: allowed
- added capability: blocked
- removed capability: allowed
- publisher change: blocked

## Security Tests (Blocking)

See plugin-redteam-suite.md for the full list.

Critical blocking tests:
- test_http_redirect_to_metadata_ip_denied
- test_fs_symlink_escape_denied
- test_compound_fs_read_network_post_is_critical
- test_wasm_infinite_loop_killed
- test_tool_id_shadow_builtin_denied
- test_output_truncated_and_marked

## Test Commands
```bash
cargo test -p navi-plugin-manifest
cargo test -p navi-plugin-security
cargo test -p navi-plugin-broker
cargo test -p navi-plugin-runtime
cargo test -p navi-plugin-registry
cargo test --test plugin_redteam
```
