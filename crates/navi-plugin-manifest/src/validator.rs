use crate::error::{ManifestError, ValidationError};
use crate::types::{Capability, FsAccess, PluginManifest, RuntimeKind, TrustLevel};
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

static ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9\-_]{1,63}$").unwrap());

static CAP_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9\-_]*$").unwrap());

static TOOL_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9\-_]*$").unwrap());

/// Validate a manifest against all rules.
/// Trust level determines which rules apply.
pub fn validate(manifest: &PluginManifest, trust_level: TrustLevel) -> Result<(), ManifestError> {
    let mut errors = Vec::new();

    validate_plugin_meta(manifest, trust_level, &mut errors);
    validate_capabilities(manifest, trust_level, &mut errors);
    validate_tools(manifest, &mut errors);
    validate_cross_references(manifest, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ManifestError::Validation { errors })
    }
}

fn validate_plugin_meta(
    manifest: &PluginManifest,
    trust_level: TrustLevel,
    errors: &mut Vec<ValidationError>,
) {
    let meta = &manifest.plugin;

    // REQ-MANIFEST-004: id format
    if !ID_RE.is_match(&meta.id) {
        errors.push(ValidationError {
            field: "plugin.id".into(),
            message: format!("must match [a-z0-9][a-z0-9-_]{{1,63}}, got '{}'", meta.id),
        });
    }

    // Required fields are enforced by serde deserialization.
    // Additional checks:

    // REQ-MANIFEST-005: runtime must be wasm-component for community
    if trust_level == TrustLevel::Community && meta.runtime != RuntimeKind::WasmComponent {
        errors.push(ValidationError {
            field: "plugin.runtime".into(),
            message: "community plugins must use runtime = 'wasm-component'".into(),
        });
    }

    // wasm_hash format
    if !meta.wasm_hash.starts_with("sha256:") {
        errors.push(ValidationError {
            field: "plugin.wasm_hash".into(),
            message: "must start with 'sha256:'".into(),
        });
    }
    let hash_hex = meta.wasm_hash.strip_prefix("sha256:").unwrap_or("");
    if hash_hex.len() != 64 || !hash_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        errors.push(ValidationError {
            field: "plugin.wasm_hash".into(),
            message: "must be sha256:<64 hex chars>".into(),
        });
    }

    // signature format
    if !meta.signature.starts_with("ed25519:") {
        errors.push(ValidationError {
            field: "plugin.signature".into(),
            message: "must start with 'ed25519:'".into(),
        });
    }

    if trust_level == TrustLevel::Community {
        match &meta.public_key {
            None => errors.push(ValidationError {
                field: "plugin.public_key".into(),
                message: "required for community plugins (ed25519:<base64>)".into(),
            }),
            Some(key) if !key.starts_with("ed25519:") => errors.push(ValidationError {
                field: "plugin.public_key".into(),
                message: "must start with 'ed25519:'".into(),
            }),
            Some(_) => {}
        }
    }
}

fn validate_capabilities(
    manifest: &PluginManifest,
    trust_level: TrustLevel,
    errors: &mut Vec<ValidationError>,
) {
    let mut cap_ids = HashSet::new();

    for cap in &manifest.capabilities {
        // REQ-MANIFEST-007: capability IDs unique
        if !cap_ids.insert(cap.id()) {
            errors.push(ValidationError {
                field: format!("capabilities[{}]", cap.id()),
                message: "duplicate capability ID".into(),
            });
        }

        // Capability ID format
        if !CAP_ID_RE.is_match(cap.id()) {
            errors.push(ValidationError {
                field: format!("capabilities[{}].id", cap.id()),
                message: "must match [a-z0-9][a-z0-9-_]+".into(),
            });
        }

        match cap {
            Capability::Filesystem { access, .. } => {
                // REQ-CAP-013: community plugins MUST NOT use read-write
                if trust_level == TrustLevel::Community && *access == FsAccess::ReadWrite {
                    errors.push(ValidationError {
                        field: format!("capabilities[{}].access", cap.id()),
                        message: "community plugins must not use read-write filesystem".into(),
                    });
                }
            }
            Capability::Network { hosts, methods, .. } => {
                // REQ-MANIFEST-014: hosts must not be empty
                if hosts.is_empty() {
                    errors.push(ValidationError {
                        field: format!("capabilities[{}].hosts", cap.id()),
                        message: "must not be empty for network capability".into(),
                    });
                }
                // REQ-MANIFEST-015: methods must not be empty
                if methods.is_empty() {
                    errors.push(ValidationError {
                        field: format!("capabilities[{}].methods", cap.id()),
                        message: "must not be empty for network capability".into(),
                    });
                }
            }
            Capability::Tui { .. } => {
                // REQ-CAP-012: community plugins MUST NOT declare tui capabilities
                if trust_level == TrustLevel::Community {
                    errors.push(ValidationError {
                        field: format!("capabilities[{}]", cap.id()),
                        message: "community plugins must not declare tui capabilities".into(),
                    });
                }
            }
        }
    }
}

fn validate_tools(manifest: &PluginManifest, errors: &mut Vec<ValidationError>) {
    let mut tool_ids = HashSet::new();

    for tool in &manifest.tools {
        // REQ-MANIFEST-006: tool IDs unique
        if !tool_ids.insert(&tool.id) {
            errors.push(ValidationError {
                field: format!("tools[{}]", tool.id),
                message: "duplicate tool ID".into(),
            });
        }

        // Tool ID format
        if !TOOL_ID_RE.is_match(&tool.id) {
            errors.push(ValidationError {
                field: format!("tools[{}].id", tool.id),
                message: "must match [a-z0-9][a-z0-9-_]+".into(),
            });
        }

        // Validate input_schema if it's a string
        if let Some(schema) = &tool.input_schema
            && schema.is_string()
        {
            let s = schema.as_str().unwrap_or("");
            if serde_json::from_str::<serde_json::Value>(s).is_err() {
                errors.push(ValidationError {
                    field: format!("tools[{}].input_schema", tool.id),
                    message: "must be valid JSON".into(),
                });
            }
        }
    }
}

fn validate_cross_references(manifest: &PluginManifest, errors: &mut Vec<ValidationError>) {
    let cap_ids: HashSet<&str> = manifest.capabilities.iter().map(|c| c.id()).collect();

    for tool in &manifest.tools {
        for cap_ref in &tool.capabilities {
            // REQ-MANIFEST-008: tool capabilities must reference existing capabilities
            if !cap_ids.contains(cap_ref.as_str()) {
                errors.push(ValidationError {
                    field: format!("tools[{}].capabilities", tool.id),
                    message: format!("references non-existent capability '{}'", cap_ref),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_manifest;

    fn valid_manifest_toml() -> String {
        r#"
[plugin]
id = "test-plugin"
name = "Test Plugin"
version = "1.0.0"
publisher = "gh:test"
runtime = "wasm-component"
entry = "plugin.wasm"
wasm_hash = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
signature = "ed25519:base64sig"
public_key = "ed25519:YWJj"
minimum_navi = "0.1.0"

[[tools]]
id = "greet"
summary = "Returns a greeting."
risk = "read_only"
capabilities = []
"#
        .to_string()
    }

    #[test]
    fn valid_manifest_passes() {
        let manifest = parse_manifest(&valid_manifest_toml()).unwrap();
        assert!(validate(&manifest, TrustLevel::Community).is_ok());
    }

    #[test]
    fn invalid_id_fails() {
        let toml = valid_manifest_toml().replace("id = \"test-plugin\"", "id = \"BAD ID!\"");
        let manifest = parse_manifest(&toml).unwrap();
        let err = validate(&manifest, TrustLevel::Community).unwrap_err();
        match err {
            ManifestError::Validation { errors } => {
                assert!(errors.iter().any(|e| e.field == "plugin.id"));
            }
            _ => panic!("expected validation error"),
        }
    }

    #[test]
    fn duplicate_tool_id_fails() {
        let toml = r#"
[plugin]
id = "test"
name = "Test"
version = "1.0.0"
publisher = "gh:test"
runtime = "wasm-component"
entry = "plugin.wasm"
wasm_hash = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
signature = "ed25519:base64sig"
public_key = "ed25519:YWJj"
minimum_navi = "0.1.0"

[[tools]]
id = "dup"
summary = "First"
risk = "read_only"
capabilities = []

[[tools]]
id = "dup"
summary = "Second"
risk = "read_only"
capabilities = []
"#
        .to_string();
        let manifest = parse_manifest(&toml).unwrap();
        let err = validate(&manifest, TrustLevel::Community).unwrap_err();
        match err {
            ManifestError::Validation { errors } => {
                assert!(errors.iter().any(|e| e.field == "tools[dup]"));
            }
            _ => panic!("expected validation error"),
        }
    }

    #[test]
    fn unknown_capability_reference_fails() {
        let toml = r#"
[plugin]
id = "test"
name = "Test"
version = "1.0.0"
publisher = "gh:test"
runtime = "wasm-component"
entry = "plugin.wasm"
wasm_hash = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
signature = "ed25519:base64sig"
public_key = "ed25519:YWJj"
minimum_navi = "0.1.0"

[[tools]]
id = "search"
summary = "Search"
risk = "read_only"
capabilities = ["nonexistent"]
"#
        .to_string();
        let manifest = parse_manifest(&toml).unwrap();
        let err = validate(&manifest, TrustLevel::Community).unwrap_err();
        match err {
            ManifestError::Validation { errors } => {
                assert!(errors.iter().any(|e| e.message.contains("nonexistent")));
            }
            _ => panic!("expected validation error"),
        }
    }

    #[test]
    fn community_read_write_fails() {
        let toml = r#"
[plugin]
id = "sneaky"
name = "Sneaky"
version = "1.0.0"
publisher = "gh:bad"
runtime = "wasm-component"
entry = "plugin.wasm"
wasm_hash = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
signature = "ed25519:base64sig"
public_key = "ed25519:YWJj"
minimum_navi = "0.1.0"

[[capabilities]]
id = "write-all"
kind = "filesystem"
scope = "project"
access = "read-write"
paths = ["/"]
reason = "Needs write."

[[tools]]
id = "overwrite"
summary = "Overwrite files."
risk = "write"
capabilities = ["write-all"]
"#
        .to_string();
        let manifest = parse_manifest(&toml).unwrap();
        let err = validate(&manifest, TrustLevel::Community).unwrap_err();
        match err {
            ManifestError::Validation { errors } => {
                assert!(errors.iter().any(|e| e.message.contains("read-write")));
            }
            _ => panic!("expected validation error"),
        }
    }

    #[test]
    fn community_tui_fails() {
        let toml = r#"
[plugin]
id = "ui-plugin"
name = "UI Plugin"
version = "1.0.0"
publisher = "gh:test"
runtime = "wasm-component"
entry = "plugin.wasm"
wasm_hash = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
signature = "ed25519:base64sig"
public_key = "ed25519:YWJj"
minimum_navi = "0.1.0"

[[capabilities]]
id = "ui"
kind = "tui"
components = ["panel"]
reason = "Render panel."

[[tools]]
id = "render"
summary = "Render UI."
risk = "read_only"
capabilities = ["ui"]
"#
        .to_string();
        let manifest = parse_manifest(&toml).unwrap();
        let err = validate(&manifest, TrustLevel::Community).unwrap_err();
        match err {
            ManifestError::Validation { errors } => {
                assert!(errors.iter().any(|e| e.message.contains("tui")));
            }
            _ => panic!("expected validation error"),
        }
    }

    #[test]
    fn empty_network_hosts_fails() {
        let toml = r#"
[plugin]
id = "test"
name = "Test"
version = "1.0.0"
publisher = "gh:test"
runtime = "wasm-component"
entry = "plugin.wasm"
wasm_hash = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
signature = "ed25519:base64sig"
public_key = "ed25519:YWJj"
minimum_navi = "0.1.0"

[[capabilities]]
id = "net"
kind = "network"
hosts = []
methods = ["GET"]
reason = "API access."

[[tools]]
id = "call"
summary = "Call API."
risk = "network_read"
capabilities = ["net"]
"#
        .to_string();
        let manifest = parse_manifest(&toml).unwrap();
        let err = validate(&manifest, TrustLevel::Community).unwrap_err();
        match err {
            ManifestError::Validation { errors } => {
                assert!(errors.iter().any(|e| e.message.contains("empty")));
            }
            _ => panic!("expected validation error"),
        }
    }

    #[test]
    fn invalid_wasm_hash_format_fails() {
        let toml = valid_manifest_toml().replace(
            "wasm_hash = \"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\"",
            "wasm_hash = \"not-a-hash\"",
        );
        let manifest = parse_manifest(&toml).unwrap();
        let err = validate(&manifest, TrustLevel::Community).unwrap_err();
        match err {
            ManifestError::Validation { errors } => {
                assert!(errors.iter().any(|e| e.field == "plugin.wasm_hash"));
            }
            _ => panic!("expected validation error"),
        }
    }

    #[test]
    fn duplicate_capability_id_fails() {
        let toml = r#"
[plugin]
id = "test"
name = "Test"
version = "1.0.0"
publisher = "gh:test"
runtime = "wasm-component"
entry = "plugin.wasm"
wasm_hash = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
signature = "ed25519:base64sig"
public_key = "ed25519:YWJj"
minimum_navi = "0.1.0"

[[capabilities]]
id = "dup"
kind = "filesystem"
scope = "project"
access = "read-only"
paths = ["src/"]
reason = "Read source."

[[capabilities]]
id = "dup"
kind = "filesystem"
scope = "project"
access = "read-only"
paths = ["lib/"]
reason = "Read lib."

[[tools]]
id = "read"
summary = "Read files."
risk = "read_only"
capabilities = ["dup"]
"#
        .to_string();
        let manifest = parse_manifest(&toml).unwrap();
        let err = validate(&manifest, TrustLevel::Community).unwrap_err();
        match err {
            ManifestError::Validation { errors } => {
                assert!(errors.iter().any(|e| e.field == "capabilities[dup]"));
            }
            _ => panic!("expected validation error"),
        }
    }
}
