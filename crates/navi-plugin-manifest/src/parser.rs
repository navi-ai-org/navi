use crate::error::ManifestError;
use crate::types::PluginManifest;

/// Parse a manifest from TOML string content.
pub fn parse_manifest(content: &str) -> Result<PluginManifest, ManifestError> {
    let manifest: PluginManifest = toml::from_str(content)?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_minimal_manifest() {
        let toml = r#"
[plugin]
id = "hello-world"
name = "Hello World"
version = "1.0.0"
publisher = "gh:example"
runtime = "wasm-component"
entry = "plugin.wasm"
wasm_hash = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
signature = "ed25519:base64sig"
minimum_navi = "0.1.0"

[[tools]]
id = "greet"
summary = "Returns a greeting."
risk = "read_only"
capabilities = []
"#;
        let manifest = parse_manifest(toml).expect("should parse");
        assert_eq!(manifest.plugin.id, "hello-world");
        assert_eq!(manifest.tools.len(), 1);
        assert_eq!(manifest.capabilities.len(), 0);
    }

    #[test]
    fn parse_manifest_with_capabilities() {
        let toml = r#"
[plugin]
id = "code-search"
name = "Code Search"
version = "2.1.0"
publisher = "gh:author"
runtime = "wasm-component"
entry = "code_search.wasm"
wasm_hash = "sha256:abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
signature = "ed25519:base64sig"
minimum_navi = "0.1.0"

[[capabilities]]
id = "read-src"
kind = "filesystem"
scope = "project"
access = "read-only"
paths = ["src/", "lib/"]
reason = "Read source files."

[[capabilities]]
id = "call-api"
kind = "network"
hosts = ["api.example.com"]
methods = ["GET", "POST"]
https_only = true
reason = "Call external API."

[capabilities.call-api.auth]
binding = "API_KEY"
inject_as = "Authorization: Bearer {secret}"

[[tools]]
id = "search"
summary = "Search code."
risk = "network_read"
capabilities = ["read-src", "call-api"]
"#;
        let manifest = parse_manifest(toml).expect("should parse");
        assert_eq!(manifest.capabilities.len(), 2);
        assert_eq!(manifest.tools[0].capabilities.len(), 2);
    }

    #[test]
    fn parse_invalid_toml_fails() {
        let toml = "this is not valid toml [[[";
        assert!(parse_manifest(toml).is_err());
    }

    #[test]
    fn parse_missing_required_field_fails() {
        let toml = r#"
[plugin]
id = "broken"
name = "Broken"
"#;
        assert!(parse_manifest(toml).is_err());
    }
}
